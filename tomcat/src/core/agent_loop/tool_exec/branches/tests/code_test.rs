use super::{run_code, truncate_code_result_text, MAX_CODE_RESULT_TEXT_BYTES};
use crate::core::connector::mcp::manager::{McpCallContext, McpManager, McpToolView, ServerState};
use crate::ext::PluginEngineConfig;
use crate::infra::config::get_work_dir;
use crate::AppConfig;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::{sync::CancellationToken, task::TaskTracker};

pub(in super::super) async fn vm_fixture(
    timeout_ms: u64,
) -> (
    tempfile::TempDir,
    AppConfig,
    Arc<McpManager>,
    std::path::PathBuf,
) {
    let temp = tempfile::tempdir().unwrap();
    let events = temp.path().join("events.jsonl");
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
    cfg.connector.mcp.call_timeout_ms = timeout_ms;
    let config_path = get_work_dir(&cfg).unwrap().join("mcp.json");
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/mcp/fake_stdio_server.mjs");
    std::fs::write(
        config_path,
        serde_json::json!({"mcpServers":{"fake":{
            "command":"node", "args":[fixture,"--events",events]
        }}})
        .to_string(),
    )
    .unwrap();
    let manager = crate::core::connector::ConnectorRegistry::new(&cfg, None)
        .unwrap()
        .mcp_manager();
    manager.connect_server("fake").await.unwrap();
    (temp, cfg, manager, events)
}

async fn events(path: &std::path::Path) -> Vec<serde_json::Value> {
    tokio::fs::read_to_string(path)
        .await
        .unwrap_or_default()
        .lines()
        .filter_map(|row| serde_json::from_str(row).ok())
        .collect()
}

pub(in super::super) async fn seen(path: &std::path::Path, kind: &str, label: &str) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !events(path)
            .await
            .iter()
            .any(|event| event["kind"] == kind && event["label"] == label)
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("fixture event must arrive");
}

pub(in super::super) fn vm_limits() -> PluginEngineConfig {
    // Allow debug-build JS/marshalling work while remaining below the 300ms MCP
    // idle deadline and 900ms host wait. These tests check isolation, not JS speed.
    PluginEngineConfig {
        call_timeout_ms: 250,
        interrupt_budget: 0,
        ..Default::default()
    }
}

#[tokio::test]
async fn code_vm_fanout_uses_independent_progress_windows_not_vm_wall_time() {
    let (temp, cfg, manager, log) = vm_fixture(300).await;
    let gate = temp.path().join("release");
    let a = serde_json::json!({"label":"A", "gateFile":gate, "delayMs":900, "progressMs":40});
    let b = serde_json::json!({"label":"B", "gateFile":gate, "delayMs":900, "progressMs":40});
    let code = format!("return await Promise.all([callTool('mcp__fake__capture', {a}), callTool('mcp__fake__capture', {b})]);");
    let limits = vm_limits();
    let vm_budget = Duration::from_millis(limits.call_timeout_ms);
    let started = std::time::Instant::now();
    let vm = tokio::spawn(run_code(
        limits,
        McpToolView::new(manager.clone(), None),
        code,
        McpCallContext::default(),
    ));
    seen(&log, "started", "A").await;
    seen(&log, "started", "B").await;
    assert!(
        !vm.is_finished(),
        "VM ended before opening the gate: {:?}; fixture events: {:?}",
        vm.await,
        events(&log).await
    );
    tokio::fs::write(gate, "release").await.unwrap();
    let value = vm.await.unwrap().unwrap();
    assert!(started.elapsed() > vm_budget);
    assert_eq!(value[0]["content"][0]["text"], "fixture result: A");
    assert_eq!(value[1]["content"][0]["text"], "fixture result: B");
    assert_eq!(
        events(&log)
            .await
            .iter()
            .filter(|e| e["kind"] == "started")
            .count(),
        2
    );
    assert_eq!(manager.call_debug_counts("fake"), Some((0, 16, 32)));
    manager.remove_configured_server("fake", &cfg).unwrap();
}

#[tokio::test]
async fn code_vm_polling_and_sibling_progress_cannot_renew_a_wrong_token_call() {
    let (_temp, cfg, manager, log) = vm_fixture(300).await;
    let code = r#"
const settled = await Promise.allSettled([
  callTool('mcp__fake__capture', {label:'A', delayMs:900, progressMs:40}),
  callTool('mcp__fake__status', {label:'B', delayMs:900, progressMs:40, wrongToken:true})
]);
return settled.map(r => r.status === 'fulfilled'
  ? {status:r.status, text:r.value.content[0].text}
  : {status:r.status, error:String(r.reason)});
"#
    .to_string();
    let vm = tokio::spawn(run_code(
        vm_limits(),
        McpToolView::new(manager.clone(), None),
        code,
        McpCallContext::default(),
    ));
    seen(&log, "exited", "B").await;
    assert!(!vm.is_finished());
    let value = vm.await.unwrap().unwrap();
    assert_eq!(value[0]["text"], "fixture result: A");
    assert_eq!(value[1]["status"], "rejected");
    assert!(value[1]["error"]
        .as_str()
        .unwrap()
        .contains("idle wait timed out"));
    assert_eq!(
        events(&log)
            .await
            .iter()
            .filter(|e| e["kind"] == "started")
            .count(),
        2
    );
    assert_eq!(manager.call_debug_counts("fake"), Some((0, 16, 32)));
    manager.remove_configured_server("fake", &cfg).unwrap();
}

#[tokio::test]
async fn equal_host_ids_in_two_vms_remain_isolated_on_stop_and_drop() {
    let (temp, cfg, manager, log) = vm_fixture(300).await;
    for drop_caller in [false, true] {
        for index in 0..10 {
            let label_a = format!("A-{drop_caller}-{index}");
            let label_b = format!("B-{drop_caller}-{index}");
            let gate = temp.path().join(&label_b);
            let args_a = serde_json::json!({"label":label_a, "gateFile":temp.path().join(&label_a), "progressMs":40});
            let args_b = serde_json::json!({"label":label_b, "gateFile":gate, "progressMs":40});
            let cancel = CancellationToken::new();
            let owner = TaskTracker::new();
            let mut context_a = McpCallContext::new("session-A", "same-outer-id", &cancel);
            context_a.owner_tasks = Some(owner.clone());
            let a = tokio::spawn(run_code(
                vm_limits(),
                McpToolView::new(manager.clone(), None),
                format!(
                    "Date.now = () => 0; return await callTool('mcp__fake__capture', {args_a});"
                ),
                context_a,
            ));
            let b = tokio::spawn(run_code(
                vm_limits(),
                McpToolView::new(manager.clone(), None),
                format!(
                    "Date.now = () => 0; return await callTool('mcp__fake__capture', {args_b});"
                ),
                McpCallContext::new("session-B", "same-outer-id", &CancellationToken::new()),
            ));
            seen(&log, "started", &label_a).await;
            seen(&log, "started", &label_b).await;
            owner.close();
            if drop_caller {
                a.abort();
                let _ = a.await;
            } else {
                cancel.cancel();
                assert!(a.await.unwrap().is_err());
            }
            tokio::time::timeout(Duration::from_secs(5), owner.wait())
                .await
                .unwrap();
            assert!(!b.is_finished(), "stopping A must not finish B");
            tokio::fs::write(gate, "release").await.unwrap();
            assert_eq!(
                b.await.unwrap().unwrap()["content"][0]["text"],
                format!("fixture result: {label_b}")
            );
            seen(&log, "exited", &label_a).await;
            assert_eq!(manager.call_debug_counts("fake"), Some((0, 16, 32)));
            assert_eq!(manager.statuses()[0].state, ServerState::Ready);
        }
    }
    let started: Vec<_> = events(&log)
        .await
        .into_iter()
        .filter(|e| e["kind"] == "started")
        .collect();
    assert_eq!(started.len(), 40);
    let ids: std::collections::HashSet<_> = started.iter().map(|e| e["id"].to_string()).collect();
    assert_eq!(ids.len(), 40);
    manager.remove_configured_server("fake", &cfg).unwrap();
}

#[tokio::test]
async fn returning_throwing_and_busy_vm_drop_reap_unawaited_children() {
    let (temp, cfg, manager, log) = vm_fixture(300).await;
    for tail in [
        "return 7;",
        "throw new Error('test throw');",
        "while (true) {}",
    ] {
        for index in 0..10 {
            let label = format!("child-{tail}-{index}");
            let barrier = format!("barrier-{tail}-{index}");
            let gate = temp.path().join(&barrier);
            let child = serde_json::json!({"label":label,"gateFile":temp.path().join(&label),"progressMs":40});
            let second = serde_json::json!({"label":barrier,"gateFile":gate,"progressMs":40});
            let mut context = McpCallContext::default();
            let owner = TaskTracker::new();
            context.owner_tasks = Some(owner.clone());
            // Give the busy-loop case a long local budget: cancellation,
            // not that budget, must end an already running blocking worker.
            let limits = if tail.starts_with("while") {
                PluginEngineConfig {
                    call_timeout_ms: 5000,
                    interrupt_budget: 0,
                    ..Default::default()
                }
            } else {
                vm_limits()
            };
            let vm = tokio::spawn(run_code(limits, McpToolView::new(manager.clone(), None),
                    format!("callTool('mcp__fake__capture', {child}); await callTool('mcp__fake__status', {second}); {tail}"), context));
            seen(&log, "started", &label).await;
            seen(&log, "started", &barrier).await;
            tokio::fs::write(gate, "release").await.unwrap();
            seen(&log, "exited", &barrier).await;
            owner.close();
            if tail.starts_with("while") {
                tokio::time::sleep(Duration::from_millis(75)).await;
                vm.abort();
                let _ = vm.await;
            } else {
                let result = vm.await.unwrap();
                if tail.starts_with("return") {
                    assert_eq!(result.unwrap(), 7);
                } else {
                    assert!(result.unwrap_err().to_string().contains("test throw"));
                }
            }
            tokio::time::timeout(Duration::from_secs(5), owner.wait())
                .await
                .unwrap();
            seen(&log, "exited", &label).await;
            assert_eq!(manager.call_debug_counts("fake"), Some((0, 16, 32)));
            assert_eq!(manager.statuses()[0].state, ServerState::Ready);
        }
    }
    manager.remove_configured_server("fake", &cfg).unwrap();
}

#[test]
fn truncation_preserves_utf8_and_explains_the_remedy() {
    let text = "图".repeat(MAX_CODE_RESULT_TEXT_BYTES);
    let truncated = truncate_code_result_text(&text);
    assert!(truncated.len() <= MAX_CODE_RESULT_TEXT_BYTES);
    assert!(truncated.contains("Output truncated"));
    assert!(truncated.is_char_boundary(truncated.len()));
}

#[tokio::test]
async fn code_vm_calls_deferred_mcp_without_tool_registry() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
    let config_path = get_work_dir(&cfg).expect("work dir").join("mcp.json");
    std::fs::create_dir_all(config_path.parent().expect("config directory"))
        .expect("config directory");
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/mcp/fake_stdio_server.mjs");
    std::fs::write(
        config_path,
        serde_json::json!({
            "mcpServers": {
                "fake": {
                    "command": "node",
                    "args": [fixture],
                }
            }
        })
        .to_string(),
    )
    .expect("write MCP config");
    let manager = McpManager::new(&cfg, Some(&workspace)).expect("construct manager");
    manager
        .connect_server("fake")
        .await
        .expect("connect fake MCP");

    let result = run_code(
        PluginEngineConfig::default(),
        McpToolView::new(manager.clone(), None),
        r#"return await callTool("mcp__fake__capture", {});"#.to_string(),
        McpCallContext::default(),
    )
    .await
    .expect("call deferred MCP through the code VM");
    assert_eq!(manager.call_debug_counts("fake"), Some((0, 16, 32)));
    assert_eq!(result["content"][0]["text"], "fake capture complete");
}

#[tokio::test]
async fn code_vm_cannot_bypass_untrusted_project_connector() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let workspace = temp.path().join("workspace");
    let config_path = workspace.join(".agents/mcp.json");
    std::fs::create_dir_all(config_path.parent().expect("project config directory"))
        .expect("project config directory");
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/mcp/fake_stdio_server.mjs");
    std::fs::write(
        config_path,
        serde_json::json!({
            "mcpServers": {
                "project-fake": {
                    "command": "node",
                    "args": [fixture],
                }
            }
        })
        .to_string(),
    )
    .expect("write untrusted project MCP config");

    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
    let manager = McpManager::new(&cfg, Some(&workspace)).expect("construct manager");
    manager
        .connect_server("project-fake")
        .await
        .expect("untrusted source records a confirmation requirement");
    assert!(matches!(
        manager.statuses().pop().expect("project status").state,
        ServerState::AwaitingProjectTrust
    ));

    let error = run_code(
        PluginEngineConfig::default(),
        McpToolView::new(manager, None),
        r#"return await callTool("mcp__project-fake__capture", {});"#.to_string(),
        McpCallContext::default(),
    )
    .await
    .expect_err("agent code must not bypass project connector confirmation");
    assert!(
        error
            .to_string()
            .contains("unknown or not-ready deferred tool"),
        "unapproved connector must remain unavailable to agent code: {error}"
    );
}
