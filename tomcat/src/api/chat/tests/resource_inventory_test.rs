#![allow(clippy::await_holding_lock)]
use crate::api::chat::{
    build_prompt_snapshot, refresh_prompt_snapshot, ChatContext, ChatContextOverrides,
};
use crate::{AppConfig, SessionMode};
use serde_json::json;
use serial_test::serial;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Manual timing evidence; no performance threshold runs in normal CI.
/// "cold" means the first measured reconciliation, not a flushed OS page cache.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial(env_lock)]
#[ignore = "manual resource inventory timing matrix"]
async fn sync_timing_matrix() {
    let _lock = crate::test_support::home_env_lock().lock().unwrap();
    const STAGES: [&str; 14] = [
        "lock_wait",
        "discover_skills",
        "discover_plugins",
        "diff",
        "retire",
        "register",
        "stop_vms",
        "activate",
        "total",
        "prompt_rebuild",
        "lazy_first_call",
        "lazy_hot_call",
        "vm_exit_upper_bound",
        "total_with_prompt",
    ];
    println!("All values are ms. cold=first measured sync (OS caches not flushed). vm_exit_upper_bound=sync start to observed Stopped, NOT exact shutdown-to-exit latency.");
    for (label, skill_count, plugin_count, changed) in [
        ("S1", 30, 5, false),
        ("S2", 1000, 20, false),
        ("S3", 30, 5, true),
        ("S4", 1000, 20, true),
    ] {
        let fixture = Fixture::new();
        let ctx = fixture.ctx(SessionMode::Code, false);
        ctx.spawn_skill_discovery_if_needed().await;
        ctx.await_skill_discovery().await;
        let builtins = ctx.skill_set_snapshot().by_name.len();
        for i in 0..skill_count - builtins {
            fixture.skill(&format!("matrix-skill-{i:04}"), "timing fixture");
        }
        let sid = ctx
            .session_runtime
            .session
            .current_session_id()
            .unwrap()
            .unwrap();
        let pm = ctx.global_services.plugin_manager.as_ref().unwrap();
        let session_count = if plugin_count == 5 { 2 } else { 10 };
        let mut samples: Vec<[Duration; 14]> = Vec::new();
        let budget = crate::infra::config::compute_context_budget_chars(&ctx.config.context);
        for iteration in 0..11 {
            // Restore the baseline outside the timed slice so every changed
            // sample performs a real transition rather than ten no-op repeats.
            if changed || iteration == 0 {
                fixture.skill("matrix-skill-0000", "timing fixture");
                let extra = fixture.workspace.path().join(".agents/plugins/matrix-new");
                if extra.exists() {
                    std::fs::remove_dir_all(extra).unwrap();
                }
                for i in 0..plugin_count {
                    let id = format!("matrix-{i:02}");
                    let mut body = script(&id, &format!("baseline-{iteration}"));
                    if plugin_count == 20 && i >= 18 {
                        body.push_str(&format!("\n/*{}*/", "x".repeat(1024 * 1024)));
                    }
                    fixture.plugin(
                        &id,
                        if i < session_count { "session" } else { "lazy" },
                        &body,
                    );
                }
                ctx.sync_resource_inventory().await.unwrap();
                assert_eq!(ctx.skill_set_snapshot().by_name.len(), skill_count);
            }
            let mut old_handles = Vec::new();
            if changed {
                let updates = if plugin_count == 5 { 1 } else { 10 };
                for i in 0..updates {
                    let id = format!("matrix-{i:02}");
                    old_handles.push(pm.start_session_vm(&sid, &id).await.unwrap());
                    std::fs::write(
                        fixture
                            .workspace
                            .path()
                            .join(".agents/plugins")
                            .join(&id)
                            .join("main.js"),
                        script(&id, &format!("updated-{iteration}")),
                    )
                    .unwrap();
                }
                if plugin_count == 5 {
                    let removed = fixture.workspace.path().join(".agents/plugins/matrix-02");
                    old_handles.push(pm.start_session_vm(&sid, "matrix-02").await.unwrap());
                    std::fs::remove_dir_all(removed).unwrap();
                    std::fs::remove_dir_all(
                        fixture
                            .workspace
                            .path()
                            .join(".agents/skills/matrix-skill-0000"),
                    )
                    .unwrap();
                    fixture.plugin("matrix-new", "session", &script("matrix-new", "added"));
                }
            }
            let mut prompt = build_prompt_snapshot(&ctx, budget).await;
            // The snapshot is built before reconciliation; only disk has changed.
            let start = Instant::now();
            let report = ctx.sync_resource_inventory().await.unwrap();
            assert_eq!(report.has_changes(), changed, "{label}: {report:?}");
            let prompt_start = Instant::now();
            let rebuilt = refresh_prompt_snapshot(&ctx, budget, &mut prompt).await;
            let prompt_time = prompt_start.elapsed();
            if !changed {
                assert!(!rebuilt);
            }
            let sync_with_prompt = start.elapsed();
            let mut lazy_first = Duration::ZERO;
            let mut lazy_hot = Duration::ZERO;
            if label == "S3" {
                let call = Instant::now();
                ctx.global_services
                    .tool_registry
                    .call_tool("matrix-03_tool", json!({}), "timing", Some(&sid))
                    .await
                    .unwrap();
                lazy_first = call.elapsed();
                let call = Instant::now();
                ctx.global_services
                    .tool_registry
                    .call_tool("matrix-03_tool", json!({}), "timing", Some(&sid))
                    .await
                    .unwrap();
                lazy_hot = call.elapsed();
            }
            let vm_exit = if old_handles.is_empty() {
                Duration::ZERO
            } else {
                tokio::time::timeout(Duration::from_secs(3), async {
                    while old_handles
                        .iter()
                        .any(|handle| handle.current_state() != crate::ext::VmActorState::Stopped)
                    {
                        tokio::time::sleep(Duration::from_millis(1)).await;
                    }
                })
                .await
                .unwrap();
                start.elapsed()
            };
            let t = report.timings;
            let row = [
                t.lock_wait,
                t.discover_skills,
                t.discover_plugins,
                t.diff,
                t.retire,
                t.register,
                t.stop_vms,
                t.activate,
                t.total,
                prompt_time,
                lazy_first,
                lazy_hot,
                vm_exit,
                sync_with_prompt,
            ];
            samples.push(row);
        }
        println!(
            "scenario {label}: {skill_count} skills, {plugin_count} plugins, changed={changed}"
        );
        for (index, stage) in STAGES.iter().enumerate() {
            let mut hot = samples[1..]
                .iter()
                .map(|sample| sample[index])
                .collect::<Vec<_>>();
            hot.sort();
            let median = (hot[4] + hot[5]) / 2;
            println!(
                "{label} {stage:22} cold={:9.3} median={:9.3} max={:9.3}",
                samples[0][index].as_secs_f64() * 1000.0,
                median.as_secs_f64() * 1000.0,
                hot[9].as_secs_f64() * 1000.0
            );
        }
        stop(&ctx).await;
    }
}

struct EnvGuard(&'static str, Option<OsString>);
impl EnvGuard {
    fn set(key: &'static str, value: impl Into<OsString>) -> Self {
        let old = std::env::var_os(key);
        unsafe {
            std::env::set_var(key, value.into());
        }
        Self(key, old)
    }
}
impl Drop for EnvGuard {
    fn drop(&mut self) {
        unsafe {
            match self.1.take() {
                Some(value) => std::env::set_var(self.0, value),
                None => std::env::remove_var(self.0),
            }
        }
    }
}
struct Fixture {
    workspace: tempfile::TempDir,
    work: tempfile::TempDir,
    _home: tempfile::TempDir,
    _home_env: EnvGuard,
    _key: EnvGuard,
}
impl Fixture {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let home_env = EnvGuard::set("HOME", home.path().as_os_str());
        let key = EnvGuard::set("TOMCAT_INVENTORY_TEST_KEY", "stub");
        Self {
            workspace: tempfile::tempdir().unwrap(),
            work: tempfile::tempdir().unwrap(),
            _home: home,
            _home_env: home_env,
            _key: key,
        }
    }
    fn ctx(&self, mode: SessionMode, isolated: bool) -> ChatContext {
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(self.work.path().to_string_lossy().into_owned());
        cfg.llm.api_key_env = Some("TOMCAT_INVENTORY_TEST_KEY".into());
        crate::test_support::write_models_override(
            self.work.path(),
            &[
                crate::test_support::TestModelOverride::gpt54_openai_responses(
                    "TOMCAT_INVENTORY_TEST_KEY",
                ),
            ],
        );
        let mut overrides = ChatContextOverrides::default()
            .with_session_cwd_override(self.workspace.path().to_path_buf());
        if isolated {
            overrides = overrides.with_fetch_http_client(reqwest::Client::new());
        }
        ChatContext::from_config_with_mode_and_overrides(cfg, mode, overrides).unwrap()
    }
    fn skill(&self, name: &str, description: &str) -> PathBuf {
        let root = self.workspace.path().join(".agents/skills").join(name);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\nbody\n"),
        )
        .unwrap();
        root
    }
    fn plugin(&self, id: &str, activation: &str, script: &str) -> PathBuf {
        let root = self.workspace.path().join(".agents/plugins").join(id);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("plugin.json"), json!({
            "id":id,"name":id,"version":"1.0.0","author":"test","description":"test","main":"main.js",
            "requiredPermissions":[],"requiredApiVersion":"1.0","tags":[],"activation":activation,
            "tools":[{"name":format!("{id}_tool"),"description":"echo","parameters":{"type":"object","properties":{}}}]
        }).to_string()).unwrap();
        std::fs::write(root.join("main.js"), script).unwrap();
        root
    }
}
fn script(id: &str, version: &str) -> String {
    format!("pi.registerTool({{name:'{id}_tool',description:'echo',parameters:{{type:'object'}},execute:function(){{return {{version:'{version}'}};}}}});")
}
async fn names(ctx: &ChatContext) -> Vec<String> {
    ctx.global_services
        .tool_registry
        .list_tools(None)
        .await
        .unwrap()
        .into_iter()
        .map(|tool| tool.name)
        .collect()
}
async fn stop(ctx: &ChatContext) {
    let pm = ctx.global_services.plugin_manager.as_ref().unwrap();
    let sid = ctx
        .session_runtime
        .session
        .current_session_id()
        .unwrap()
        .unwrap();
    let mut handles = Vec::new();
    for id in pm.list_loaded() {
        if pm.has_session_vm(&sid, &id) {
            handles.push(pm.start_session_vm(&sid, &id).await.unwrap());
        }
    }
    pm.end_session(&sid).await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while handles
            .iter()
            .any(|handle| handle.current_state() != crate::ext::VmActorState::Stopped)
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial(env_lock)]
async fn resource_inventory_diff_noop_description_update_and_removal() {
    let _lock = crate::test_support::home_env_lock().lock().unwrap();
    let fixture = Fixture::new();
    let removed_skill = fixture.skill("removed", "old");
    fixture.skill("keep", "unchanged");
    let plugin = fixture.plugin("echo", "lazy", &script("echo", "old"));
    fixture.plugin("unrelated", "lazy", &script("unrelated", "keep"));
    let ctx = fixture.ctx(SessionMode::Code, false);
    ctx.spawn_skill_discovery_if_needed().await;
    ctx.await_skill_discovery().await;
    let sid = ctx
        .session_runtime
        .session
        .current_session_id()
        .unwrap()
        .unwrap();
    let pm = ctx.global_services.plugin_manager.as_ref().unwrap();
    let old = pm.start_session_vm(&sid, "echo").await.unwrap();
    let budget = crate::infra::config::compute_context_budget_chars(&ctx.config.context);
    let mut prompt = build_prompt_snapshot(&ctx, budget).await;
    let epoch = super::super::current_resource_inventory_epoch();
    for _ in 0..2 {
        let report = ctx.sync_resource_inventory().await.unwrap();
        assert!(!report.has_changes(), "{report:?}");
        assert_eq!(report.timings.retire, Duration::ZERO);
        assert_eq!(report.timings.register, Duration::ZERO);
        assert_eq!(report.timings.activate, Duration::ZERO);
        assert!(
            report.timings.total
                >= report.timings.discover_plugins
                    + report.timings.discover_skills
                    + report.timings.diff
        );
        assert!(!refresh_prompt_snapshot(&ctx, budget, &mut prompt).await);
        assert_eq!(super::super::current_resource_inventory_epoch(), epoch);
        assert!(Arc::ptr_eq(
            &old.state,
            &pm.start_session_vm(&sid, "echo").await.unwrap().state
        ));
    }
    fixture.skill("removed", "new-description");
    let report = ctx.sync_resource_inventory().await.unwrap();
    assert_eq!(report.skills_changed, ["removed"]);
    assert!(refresh_prompt_snapshot(&ctx, budget, &mut prompt).await);
    assert!(prompt.system_text().contains("new-description"));
    assert_eq!(super::super::current_resource_inventory_epoch(), epoch + 1);
    std::fs::write(plugin.join("main.js"), script("echo", "new")).unwrap();
    let report = ctx.sync_resource_inventory().await.unwrap();
    assert_eq!(report.plugins_changed, ["echo"]);
    assert!(!pm.has_session_vm(&sid, "echo"));
    let value = ctx
        .global_services
        .tool_registry
        .call_tool("echo_tool", json!({}), "test", Some(&sid))
        .await
        .unwrap();
    assert!(value.to_string().contains("new"), "{value}");
    std::fs::remove_dir_all(plugin).unwrap();
    std::fs::remove_dir_all(removed_skill).unwrap();
    let report = ctx.sync_resource_inventory().await.unwrap();
    assert_eq!(report.plugins_removed, ["echo"]);
    assert_eq!(report.skills_removed, ["removed"]);
    assert!(!pm.has_session_vm(&sid, "echo"));
    assert_eq!(names(&ctx).await, ["unrelated_tool"]);
    assert!(ctx.skill_set_snapshot().by_name.contains_key("keep"));
    stop(&ctx).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial(env_lock)]
async fn resource_inventory_file_read_failure_preserves_last_good_category() {
    let _lock = crate::test_support::home_env_lock().lock().unwrap();
    let fixture = Fixture::new();
    let skill = fixture.skill("keep-file", "last good description");
    let plugin = fixture.plugin("keep-plugin", "lazy", &script("keep-plugin", "old"));
    let ctx = fixture.ctx(SessionMode::Code, false);
    ctx.spawn_skill_discovery_if_needed().await;
    ctx.await_skill_discovery().await;
    let before = ctx.skill_set_snapshot();
    // Invalid UTF-8 fails the disk reader, independently of process UID/chmod.
    std::fs::write(skill.join("SKILL.md"), [0xff, 0xfe]).unwrap();
    // Manifest stays valid; reading its script as bytes fails (IsADirectory).
    std::fs::remove_file(plugin.join("main.js")).unwrap();
    std::fs::create_dir(plugin.join("main.js")).unwrap();
    let report = ctx.sync_resource_inventory().await.unwrap();
    assert!(
        !report.has_changes(),
        "read failure is not uninstall: {report:?}"
    );
    assert_eq!(ctx.skill_set_snapshot(), before);
    assert_eq!(names(&ctx).await, ["keep-plugin_tool"]);
    assert!(report
        .warnings
        .iter()
        .any(|warning| warning.starts_with("skills_scan_unreadable:")));
    assert!(report
        .warnings
        .iter()
        .any(|warning| warning.contains("plugin catalog ignored")));
    stop(&ctx).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial(env_lock)]
async fn resource_inventory_bad_registry_preserves_last_good_plugins() {
    let _lock = crate::test_support::home_env_lock().lock().unwrap();
    let fixture = Fixture::new();
    fixture.plugin("echo", "lazy", &script("echo", "old"));
    let ctx = fixture.ctx(SessionMode::Code, false);
    ctx.spawn_skill_discovery_if_needed().await;
    ctx.await_skill_discovery().await;
    let before = names(&ctx).await;
    let registry = fixture
        .workspace
        .path()
        .join(".agents/plugins/registry.json");
    std::fs::write(&registry, "broken JSON").unwrap();
    let report = ctx.sync_resource_inventory().await.unwrap();
    assert!(!report.has_changes(), "{report:?}");
    assert!(!report.warnings.is_empty());
    assert_eq!(names(&ctx).await, before);
    std::fs::write(&registry, "{\"plugins\":[]}").unwrap();
    assert_eq!(
        ctx.sync_resource_inventory().await.unwrap().plugins_removed,
        ["echo"]
    );
    stop(&ctx).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial(env_lock)]
async fn resource_inventory_epoch_is_the_only_turn_gate_and_session_activation_matches_new_context()
{
    let _lock = crate::test_support::home_env_lock().lock().unwrap();
    let fixture = Fixture::new();
    let skill = fixture.skill("external", "old");
    let a = fixture.ctx(SessionMode::Code, false);
    a.spawn_skill_discovery_if_needed().await;
    a.await_skill_discovery().await;
    let b = fixture.ctx(SessionMode::Claw, false);
    assert!(Arc::ptr_eq(
        &a.scope_services.scope_container,
        &b.scope_services.scope_container
    ));
    std::fs::remove_dir_all(skill).unwrap();
    a.sync_resource_inventory_before_turn().await.unwrap();
    assert!(
        a.skill_set_snapshot().by_name.contains_key("external"),
        "external edits require /reload"
    );
    fixture.plugin("session-echo", "session", &script("session-echo", "new"));
    let report = a.sync_resource_inventory().await.unwrap();
    assert_eq!(report.skills_removed, ["external"]);
    assert_eq!(report.plugins_added, ["session-echo"]);
    let epoch = super::super::current_resource_inventory_epoch();
    b.sync_resource_inventory_before_turn().await.unwrap();
    assert_eq!(
        super::super::current_resource_inventory_epoch(),
        epoch,
        "B must not republish A's change"
    );
    for ctx in [&a, &b] {
        let sid = ctx
            .session_runtime
            .session
            .current_session_id()
            .unwrap()
            .unwrap();
        assert!(ctx
            .global_services
            .plugin_manager
            .as_ref()
            .unwrap()
            .has_session_vm(&sid, "session-echo"));
    }
    let cold = fixture.ctx(SessionMode::Code, true);
    cold.spawn_skill_discovery_if_needed().await;
    cold.await_skill_discovery().await;
    assert!(!Arc::ptr_eq(
        &a.scope_services.scope_container,
        &cold.scope_services.scope_container
    ));
    assert_eq!(names(&a).await, names(&cold).await);
    assert_eq!(
        a.skill_set_snapshot().by_name.keys().collect::<Vec<_>>(),
        cold.skill_set_snapshot().by_name.keys().collect::<Vec<_>>()
    );
    stop(&a).await;
    stop(&b).await;
    stop(&cold).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial(env_lock)]
async fn resource_inventory_busy_peer_drains_then_switches_at_next_turn() {
    let _lock = crate::test_support::home_env_lock().lock().unwrap();
    for deleted in [false, true] {
        let fixture = Fixture::new();
        let plugin = fixture.plugin("echo", "lazy", r#"
pi.registerTool({name:'echo_tool',description:'echo',parameters:{type:'object'}, execute:function(_id,p){
 if(p.block){
   pi.emit('blocked');
   var event = JSON.parse(__pi_host_call(JSON.stringify({module:'__session',method:'waitForEvent',params:{}})));
   if (!event.ok || event.data.type !== 'release') throw new Error('unexpected release event');
 }
 return {version:'old'};
}});
"#);
        let a = fixture.ctx(SessionMode::Code, false);
        a.spawn_skill_discovery_if_needed().await;
        a.await_skill_discovery().await;
        let b = fixture.ctx(SessionMode::Claw, false);
        let sid_a = a
            .session_runtime
            .session
            .current_session_id()
            .unwrap()
            .unwrap();
        let sid_b = b
            .session_runtime
            .session
            .current_session_id()
            .unwrap()
            .unwrap();
        let pm = a.global_services.plugin_manager.as_ref().unwrap();
        let old_a = pm.start_session_vm(&sid_a, "echo").await.unwrap();
        let old_b = pm.start_session_vm(&sid_b, "echo").await.unwrap();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let sender = std::sync::Mutex::new(Some(started_tx));
        a.global_services.event_bus.on(
            "blocked",
            Box::new(move |_| {
                if let Some(sender) = sender.lock().unwrap().take() {
                    let _ = sender.send(());
                }
                Ok(())
            }),
        );
        let registry = b.global_services.tool_registry.clone();
        let peer = sid_b.clone();
        let inflight = tokio::spawn(async move {
            registry
                .call_tool("echo_tool", json!({"block":true}), "test", Some(&peer))
                .await
        });
        tokio::time::timeout(Duration::from_secs(3), started_rx)
            .await
            .unwrap()
            .unwrap();
        if deleted {
            std::fs::remove_dir_all(&plugin).unwrap();
        } else {
            std::fs::write(plugin.join("main.js"), script("echo", "new")).unwrap();
        }
        a.sync_resource_inventory().await.unwrap();
        assert!(!pm.has_session_vm(&sid_a, "echo"));
        assert!(pm.has_session_vm(&sid_b, "echo"));
        assert!(
            !inflight.is_finished(),
            "peer must still be in flight after A's reload"
        );
        pm.dispatch_session_event(&sid_b, "echo", "release", json!({}), json!({}))
            .unwrap();
        let result = tokio::time::timeout(Duration::from_secs(3), inflight)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(result.to_string().contains("old"));
        if deleted {
            let now = Instant::now();
            assert!(b
                .global_services
                .tool_registry
                .call_tool("echo_tool", json!({}), "test", Some(&sid_b))
                .await
                .is_err());
            assert!(now.elapsed() < Duration::from_secs(1));
            let response = b.scope_services.scope_container.dispatcher.dispatch_async(&format!("{sid_b}/echo"), crate::ext::HostRequest {
                module:"tools".into(), method:"registerTool".into(), params:json!({"name":"zombie", "description":"revoked", "parameters":{"type":"object"}}), call_id:None
            }).await.unwrap();
            assert!(!response.ok);
            assert!(!names(&b).await.iter().any(|name| name == "zombie"));
        } else {
            let result = b
                .global_services
                .tool_registry
                .call_tool("echo_tool", json!({}), "test", Some(&sid_b))
                .await
                .unwrap();
            assert!(result.to_string().contains("old"));
        }
        b.sync_resource_inventory_before_turn().await.unwrap();
        assert!(!pm.has_session_vm(&sid_b, "echo"));
        tokio::time::timeout(Duration::from_secs(3), async {
            while old_a.current_state() != crate::ext::VmActorState::Stopped
                || old_b.current_state() != crate::ext::VmActorState::Stopped
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        if !deleted {
            let result = b
                .global_services
                .tool_registry
                .call_tool("echo_tool", json!({}), "test", Some(&sid_b))
                .await
                .unwrap();
            assert!(result.to_string().contains("new"));
        }
        stop(&a).await;
        stop(&b).await;
    }
}
