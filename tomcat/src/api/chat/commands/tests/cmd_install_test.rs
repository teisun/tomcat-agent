use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;

use super::super::cmd_install::{run, InstallCommand, InstallTarget};
use super::super::parse::ChatCommandOutcome;
use crate::api::chat::panels::{Answer, AskQuestionPanel, AskQuestionResult, MockAskQuestionPanel};
use crate::api::chat::{ChatContext, ChatContextOverrides};
use crate::core::SessionMode;
use crate::AppConfig;
use serial_test::serial;

struct EnvGuard {
    key: &'static str,
    old: Option<OsString>,
}

impl EnvGuard {
    fn set(key: &'static str, value: impl Into<OsString>) -> Self {
        let old = std::env::var_os(key);
        unsafe { std::env::set_var(key, value.into()) };
        Self { key, old }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.old {
            Some(value) => unsafe { std::env::set_var(self.key, value) },
            None => unsafe { std::env::remove_var(self.key) },
        }
    }
}

struct CurrentDirGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    previous: std::path::PathBuf,
}

impl CurrentDirGuard {
    fn set(path: &Path) -> Self {
        let lock = crate::test_support::cwd_lock().lock().unwrap();
        let previous = std::env::current_dir().expect("current_dir");
        std::env::set_current_dir(path).expect("set_current_dir");
        Self {
            _lock: lock,
            previous,
        }
    }
}

impl Drop for CurrentDirGuard {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.previous);
    }
}

fn write_plugin(dir: &Path, id: &str, tool_name: &str, main_body: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("plugin.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "id": id,
            "name": id,
            "version": "1.0.0",
            "description": "test plugin",
            "author": "tests",
            "main": "main.js",
            "requiredPermissions": [],
            "requiredApiVersion": "1.0",
            "tags": [],
            "tools": [{
                "name": tool_name,
                "description": "tool description",
                "parameters": {
                    "type": "object",
                    "properties": {}
                }
            }]
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(dir.join("main.js"), main_body).unwrap();
}

fn write_skill(dir: &Path, name: &str, description: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n# Body\n"),
    )
    .unwrap();
}

fn write_package_manifest(root: &Path, plugins: &[&str], skills: &[&str]) {
    std::fs::write(
        root.join("package.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "name": "chat-install-package",
            "version": "2.0.0",
            "tomcat": {
                "plugins": plugins,
                "skills": skills
            }
        }))
        .unwrap(),
    )
    .unwrap();
}

fn build_ctx(work_dir: &Path, panel: Arc<dyn AskQuestionPanel>) -> ChatContext {
    const API_ENV: &str = "TOMCAT_CMD_INSTALL_TEST_KEY";
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(work_dir.to_string_lossy().to_string());
    cfg.llm.api_key_env = Some(API_ENV.to_string());
    ChatContext::from_config_with_mode_and_overrides(
        cfg,
        SessionMode::Code,
        ChatContextOverrides::default().with_ask_question_panel(panel),
    )
    .expect("chat context should be created")
}

#[tokio::test]
#[serial(env_lock)]
async fn run_install_refreshes_current_session_inventory() {
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    let _home_guard = EnvGuard::set("HOME", home.path().as_os_str().to_os_string());
    let _api_guard = EnvGuard::set("TOMCAT_CMD_INSTALL_TEST_KEY", "stub");
    let _cwd_guard = CurrentDirGuard::set(workspace.path());

    write_plugin(
        &source.path().join("plugins/release-plugin"),
        "release-plugin",
        "release_tool",
        "export default 1;\n",
    );
    write_skill(
        &source.path().join("skills/commit"),
        "commit",
        "Create a commit.",
    );
    write_package_manifest(
        source.path(),
        &["plugins/release-plugin"],
        &["skills/commit"],
    );

    let panel: Arc<dyn AskQuestionPanel> =
        Arc::new(MockAskQuestionPanel::new(vec![AskQuestionResult {
            answers: vec![Answer {
                question_id: "install-target".to_string(),
                option_ids: vec!["scope".to_string()],
                custom_text: None,
                skipped: false,
                picked_recommended: true,
            }],
            cancelled: false,
        }]));
    let ctx = build_ctx(work.path(), panel);

    let outcome = run(
        &ctx,
        InstallCommand {
            source: source.path().to_string_lossy().to_string(),
            target: None,
        },
    )
    .await;
    assert!(matches!(outcome, ChatCommandOutcome::Handled));
    assert!(ctx.skill_set_snapshot().resolve_any("commit").is_some());
    let tools = ctx
        .global_services
        .tool_registry
        .list_tools(Some("release-plugin"))
        .await
        .expect("list tools after refresh");
    assert!(
        tools.iter().any(|tool| tool.name == "release_tool"),
        "plugin static tool should become visible immediately"
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn run_install_cancelled_has_no_side_effects() {
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    let _home_guard = EnvGuard::set("HOME", home.path().as_os_str().to_os_string());
    let _api_guard = EnvGuard::set("TOMCAT_CMD_INSTALL_TEST_KEY", "stub");
    let _cwd_guard = CurrentDirGuard::set(workspace.path());

    write_skill(source.path(), "commit", "Create a commit.");

    let panel: Arc<dyn AskQuestionPanel> =
        Arc::new(MockAskQuestionPanel::new(vec![AskQuestionResult {
            answers: vec![],
            cancelled: true,
        }]));
    let ctx = build_ctx(work.path(), panel);

    let outcome = run(
        &ctx,
        InstallCommand {
            source: source.path().to_string_lossy().to_string(),
            target: None,
        },
    )
    .await;
    assert!(matches!(outcome, ChatCommandOutcome::Handled));
    assert!(
        !workspace
            .path()
            .join(".tomcat/packages/registry.json")
            .exists(),
        "cancelled chooser should not write any package registry"
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn install_live_refresh_does_not_execute_plugin() {
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    let _home_guard = EnvGuard::set("HOME", home.path().as_os_str().to_os_string());
    let _api_guard = EnvGuard::set("TOMCAT_CMD_INSTALL_TEST_KEY", "stub");
    let _cwd_guard = CurrentDirGuard::set(workspace.path());

    write_plugin(
        source.path(),
        "syntax-bomb-plugin",
        "bomb_tool",
        "this is definitely not valid javascript !!!\n",
    );

    let panel: Arc<dyn AskQuestionPanel> = Arc::new(MockAskQuestionPanel::new(vec![]));
    let ctx = build_ctx(work.path(), panel);

    let outcome = run(
        &ctx,
        InstallCommand {
            source: source.path().to_string_lossy().to_string(),
            target: Some(InstallTarget::CurrentProject),
        },
    )
    .await;
    assert!(matches!(outcome, ChatCommandOutcome::Handled));

    let tools = ctx
        .global_services
        .tool_registry
        .list_tools(Some("syntax-bomb-plugin"))
        .await
        .expect("list tools after install");
    assert!(
        tools.iter().any(|tool| tool.name == "bomb_tool"),
        "static tool should still register from manifest-only refresh"
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn install_keeps_active_session_plugin_catalog_stable() {
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    let _home_guard = EnvGuard::set("HOME", home.path().as_os_str().to_os_string());
    let _api_guard = EnvGuard::set("TOMCAT_CMD_INSTALL_TEST_KEY", "stub");
    let _cwd_guard = CurrentDirGuard::set(workspace.path());

    write_plugin(
        &work.path().join("plugins/loaded-plugin"),
        "loaded-plugin",
        "loaded_old",
        r#"
pi.registerTool({
  name: "loaded_old",
  description: "old tool",
  parameters: { type: "object", properties: {} },
  execute: function () { return { ok: true }; }
});
"#,
    );
    write_plugin(
        source.path(),
        "loaded-plugin",
        "loaded_new",
        r#"
pi.registerTool({
  name: "loaded_new",
  description: "new tool",
  parameters: { type: "object", properties: {} },
  execute: function () { return { ok: true }; }
});
"#,
    );

    let panel: Arc<dyn AskQuestionPanel> = Arc::new(MockAskQuestionPanel::new(vec![]));
    let ctx = build_ctx(work.path(), panel);
    let plugin_manager = ctx
        .global_services
        .plugin_manager
        .as_ref()
        .expect("plugin manager")
        .clone();
    let session_id = ctx
        .session_runtime
        .session
        .current_session_id()
        .expect("current_session_id")
        .expect("session id");

    plugin_manager
        .start_session_vm(&session_id, "loaded-plugin")
        .await
        .expect("start session vm for loaded plugin");
    assert!(
        plugin_manager.has_session_vm(&session_id, "loaded-plugin"),
        "fixture should have an active session VM before /install"
    );

    let tools_before = ctx
        .global_services
        .tool_registry
        .list_tools(Some("loaded-plugin"))
        .await
        .expect("list tools before refresh");
    assert!(
        tools_before.iter().any(|tool| tool.name == "loaded_old"),
        "old static tool should be visible before /install"
    );
    assert!(
        !tools_before.iter().any(|tool| tool.name == "loaded_new"),
        "new static tool should not be visible before /install"
    );

    let outcome = run(
        &ctx,
        InstallCommand {
            source: source.path().to_string_lossy().to_string(),
            target: Some(InstallTarget::CurrentProject),
        },
    )
    .await;
    assert!(matches!(outcome, ChatCommandOutcome::Handled));

    let tools_after = ctx
        .global_services
        .tool_registry
        .list_tools(Some("loaded-plugin"))
        .await
        .expect("list tools after refresh");
    assert!(
        tools_after.iter().any(|tool| tool.name == "loaded_old"),
        "active session plugin should retain old static tool entry"
    );
    assert!(
        !tools_after.iter().any(|tool| tool.name == "loaded_new"),
        "active session VM should block new manifest tools from surfacing mid-session"
    );
    assert!(
        plugin_manager.has_session_vm(&session_id, "loaded-plugin"),
        "current session VM should stay attached after /install"
    );
}
