#![allow(clippy::await_holding_lock)]

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use serial_test::serial;

use crate::{api::chat::ChatContext, AppConfig, SessionMode};

struct EnvGuard {
    key: &'static str,
    prev: Option<OsString>,
}

impl EnvGuard {
    fn set(key: &'static str, value: impl Into<OsString>) -> Self {
        let prev = std::env::var_os(key);
        // SAFETY: test-scoped env mutation guarded by serial + home_env_lock.
        unsafe { std::env::set_var(key, value.into()) };
        Self { key, prev }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match self.prev.take() {
            Some(prev) => {
                // SAFETY: restore original env during test teardown.
                unsafe { std::env::set_var(self.key, prev) };
            }
            None => {
                // SAFETY: clear test-only env during teardown.
                unsafe { std::env::remove_var(self.key) };
            }
        }
    }
}

struct CurrentDirGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    previous: PathBuf,
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

fn make_config(work_dir: &Path, api_env: &str) -> AppConfig {
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(work_dir.to_string_lossy().to_string());
    cfg.llm.api_key_env = Some(api_env.to_string());
    cfg
}

async fn assert_plugin_runtime_disabled_for_env_value(raw: &str) {
    const API_ENV: &str = "TOMCAT_PLUGIN_DISABLE_TEST_KEY";

    let _home_lock = crate::test_support::home_env_lock().lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let work_dir = tempfile::tempdir().unwrap();
    let _home_guard = EnvGuard::set("HOME", home.path().as_os_str().to_os_string());
    let _api_guard = EnvGuard::set(API_ENV, "stub");
    let _disable_guard = EnvGuard::set("PI_PLUGIN_DISABLE", raw);
    let _cwd_guard = CurrentDirGuard::set(workspace.path());
    write_plugin_fixture(
        workspace.path(),
        "disabled-plugin",
        "lazy",
        &["disabled_tool"],
        r#"
pi.registerTool({
  name: "disabled_tool",
  description: "should not load",
  parameters: { type: "object", properties: {} },
  execute: function () { return { ok: true }; }
});
"#,
    );

    let ctx = ChatContext::from_config(make_config(work_dir.path(), API_ENV)).expect("ctx");
    assert!(
        ctx.global_services.plugin_manager.is_none(),
        "PI_PLUGIN_DISABLE={raw} 时不应初始化 PluginManager"
    );
    assert!(
        list_tool_names(&ctx).await.is_empty(),
        "禁用插件运行时后不应物化任何插件工具"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(env_lock)]
async fn plugin_runtime_can_be_disabled_via_env() {
    for raw in ["1", "true", "yes", "on"] {
        assert_plugin_runtime_disabled_for_env_value(raw).await;
    }
}

#[test]
#[serial(env_lock)]
fn plugin_runtime_uses_plugin_config_values() {
    const API_ENV: &str = "TOMCAT_PLUGIN_CONFIG_TEST_KEY";

    let _home_lock = crate::test_support::home_env_lock().lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let work_dir = tempfile::tempdir().unwrap();
    let _home_guard = EnvGuard::set("HOME", home.path().as_os_str().to_os_string());
    let _api_guard = EnvGuard::set(API_ENV, "stub");
    let _cwd_guard = CurrentDirGuard::set(workspace.path());

    let mut cfg = make_config(work_dir.path(), API_ENV);
    cfg.plugin.js_heap_mb = 8;
    cfg.plugin.call_timeout_ms = 1_234;
    cfg.plugin.interrupt_budget = 9_876;
    cfg.plugin.event_channel_capacity = 7;
    cfg.plugin.idle_ttl_ms = 4_321;

    let ctx = ChatContext::from_config(cfg).expect("ctx");
    let plugin_manager = ctx
        .global_services
        .plugin_manager
        .as_ref()
        .expect("plugin manager");
    let engine_cfg = plugin_manager
        .configured_engine_config()
        .expect("configured engine");
    assert_eq!(engine_cfg.quickjs_heap_mb, 8);
    assert_eq!(engine_cfg.call_timeout_ms, 1_234);
    assert_eq!(engine_cfg.interrupt_budget, 9_876);
    assert_eq!(engine_cfg.idle_ttl_ms, 4_321);
    assert_eq!(plugin_manager.configured_event_channel_capacity(), 7);
    assert_eq!(
        plugin_manager.configured_idle_ttl(),
        Some(Duration::from_millis(4_321))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(env_lock)]
async fn configured_js_heap_mb_enforces_memory_limit() {
    const API_ENV: &str = "TOMCAT_HEAP_LIMIT_TEST_KEY";

    let _home_lock = crate::test_support::home_env_lock().lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let work_dir = tempfile::tempdir().unwrap();
    let _home_guard = EnvGuard::set("HOME", home.path().as_os_str().to_os_string());
    let _api_guard = EnvGuard::set(API_ENV, "stub");
    let _cwd_guard = CurrentDirGuard::set(workspace.path());
    write_plugin_fixture(
        workspace.path(),
        "heap-limit-plugin",
        "lazy",
        &["alloc_echo"],
        r#"
pi.registerTool({
  name: "alloc_echo",
  description: "allocate a large buffer",
  parameters: { type: "object", properties: {} },
  execute: function () {
    globalThis.__hold = new Uint8Array(4 * 1024 * 1024);
    return { allocated: globalThis.__hold.length };
  }
});
"#,
    );

    let mut cfg = make_config(work_dir.path(), API_ENV);
    cfg.plugin.js_heap_mb = 1;

    let ctx = ChatContext::from_config(cfg).expect("ctx");
    let session_id = current_session_id(&ctx);
    let err = tokio::time::timeout(
        Duration::from_secs(5),
        ctx.global_services.tool_registry.call_tool(
            "alloc_echo",
            json!({}),
            "__test__",
            Some(&session_id),
        ),
    )
    .await
    .expect("heap-limited tool call should not hang")
    .expect_err("1MB heap should reject a 4MB allocation");

    let message = err.to_string();
    assert!(
        message.contains("QuickJS")
            || message.contains("JS执行错误")
            || message.to_ascii_lowercase().contains("memory"),
        "heap limit failure should surface as a QuickJS-side error, got: {err}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(env_lock)]
async fn configured_js_heap_zero_disables_memory_limit() {
    const API_ENV: &str = "TOMCAT_HEAP_UNLIMITED_TEST_KEY";

    let _home_lock = crate::test_support::home_env_lock().lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let work_dir = tempfile::tempdir().unwrap();
    let _home_guard = EnvGuard::set("HOME", home.path().as_os_str().to_os_string());
    let _api_guard = EnvGuard::set(API_ENV, "stub");
    let _cwd_guard = CurrentDirGuard::set(workspace.path());
    write_plugin_fixture(
        workspace.path(),
        "heap-unlimited-plugin",
        "lazy",
        &["alloc_echo"],
        r#"
pi.registerTool({
  name: "alloc_echo",
  description: "allocate a large buffer",
  parameters: { type: "object", properties: {} },
  execute: function () {
    globalThis.__hold = new Uint8Array(4 * 1024 * 1024);
    return { allocated: globalThis.__hold.length };
  }
});
"#,
    );

    let mut cfg = make_config(work_dir.path(), API_ENV);
    cfg.plugin.js_heap_mb = 0;

    let ctx = ChatContext::from_config(cfg).expect("ctx");
    let session_id = current_session_id(&ctx);
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        ctx.global_services.tool_registry.call_tool(
            "alloc_echo",
            json!({}),
            "__test__",
            Some(&session_id),
        ),
    )
    .await
    .expect("unbounded heap tool call should not hang")
    .expect("heap=0 should disable the memory limit");

    assert_eq!(
        result
            .get("content")
            .and_then(|value| value.get("allocated"))
            .and_then(|value| value.as_u64()),
        Some(4 * 1024 * 1024),
        "heap=0 should allow the full allocation to succeed"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(env_lock)]
async fn chat_context_discovers_workspace_catalog_plugins() {
    const API_ENV: &str = "TOMCAT_CATALOG_DISCOVERY_TEST_KEY";

    let _home_lock = crate::test_support::home_env_lock().lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let work_dir = tempfile::tempdir().unwrap();
    let _home_guard = EnvGuard::set("HOME", home.path().as_os_str().to_os_string());
    let _api_guard = EnvGuard::set(API_ENV, "stub");
    let _cwd_guard = CurrentDirGuard::set(workspace.path());
    write_plugin_fixture(
        workspace.path(),
        "catalog-plugin",
        "lazy",
        &["catalog_echo"],
        r#"
pi.registerTool({
  name: "catalog_echo",
  description: "catalog tool",
  parameters: { type: "object", properties: { text: { type: "string" } }, required: ["text"] },
  execute: function (_callId, params) {
    return { plugin: "catalog-plugin", echo: params.text };
  }
});
"#,
    );

    let ctx = ChatContext::from_config(make_config(work_dir.path(), API_ENV)).expect("ctx");
    let pm = ctx
        .global_services
        .plugin_manager
        .as_ref()
        .expect("plugin manager");
    let mut loaded = pm.list_loaded();
    loaded.sort();

    assert_eq!(loaded, vec!["catalog-plugin".to_string()]);
    assert_eq!(
        list_tool_names(&ctx).await,
        vec!["catalog_echo".to_string()]
    );
    let info = pm
        .get_plugin("catalog-plugin")
        .expect("catalog plugin info");
    assert_eq!(
        info.loaded_at, 0,
        "catalog discovery should register a stub only"
    );
}

fn current_session_id(ctx: &ChatContext) -> String {
    ctx.session_runtime
        .session
        .current_session_id()
        .expect("read current session id")
        .expect("session id should exist")
}

async fn list_tool_names(ctx: &ChatContext) -> Vec<String> {
    let mut names = ctx
        .global_services
        .tool_registry
        .list_tools(None)
        .await
        .expect("list tools")
        .into_iter()
        .map(|tool| tool.name)
        .collect::<Vec<_>>();
    names.sort();
    names
}

fn write_plugin_fixture(
    workspace: &Path,
    plugin_id: &str,
    activation: &str,
    tools: &[&str],
    script: &str,
) {
    write_plugin_fixture_with_permissions_and_events(
        workspace,
        plugin_id,
        activation,
        tools,
        &[],
        &[],
        script,
    );
}

fn write_plugin_fixture_with_events(
    workspace: &Path,
    plugin_id: &str,
    activation: &str,
    tools: &[&str],
    events: &[&str],
    script: &str,
) {
    write_plugin_fixture_with_permissions_and_events(
        workspace,
        plugin_id,
        activation,
        tools,
        &[],
        events,
        script,
    );
}

fn write_plugin_fixture_with_permissions(
    workspace: &Path,
    plugin_id: &str,
    activation: &str,
    tools: &[&str],
    required_permissions: &[&str],
    script: &str,
) {
    write_plugin_fixture_with_permissions_and_events(
        workspace,
        plugin_id,
        activation,
        tools,
        required_permissions,
        &[],
        script,
    );
}

fn write_plugin_fixture_with_permissions_and_events(
    workspace: &Path,
    plugin_id: &str,
    activation: &str,
    tools: &[&str],
    required_permissions: &[&str],
    events: &[&str],
    script: &str,
) {
    let plugin_dir = workspace.join(".tomcat").join("plugins").join(plugin_id);
    fs::create_dir_all(&plugin_dir).expect("create plugin fixture dir");
    let tool_defs = tools
        .iter()
        .map(|tool_name| {
            json!({
                "name": tool_name,
                "description": format!("{plugin_id}::{tool_name}"),
                "parameters": {
                    "type": "object",
                    "properties": {
                        "text": { "type": "string" }
                    },
                    "required": ["text"]
                }
            })
        })
        .collect::<Vec<_>>();
    let manifest = json!({
        "id": plugin_id,
        "name": plugin_id,
        "version": "0.1.0",
        "description": format!("fixture {plugin_id}"),
        "author": "tests",
        "main": "main.js",
        "requiredPermissions": required_permissions,
        "requiredApiVersion": "1.0",
        "tags": [],
        "tools": tool_defs,
        "events": events,
        "activation": activation
    });
    fs::write(
        plugin_dir.join("plugin.json"),
        serde_json::to_string_pretty(&manifest).expect("serialize manifest"),
    )
    .expect("write plugin manifest");
    fs::write(plugin_dir.join("main.js"), script).expect("write plugin main");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(env_lock)]
async fn chat_context_loads_auto_load_plugin_with_permissions_by_default() {
    const API_ENV: &str = "TOMCAT_PERMISSION_AUTOLOAD_TEST_KEY";

    let _home_lock = crate::test_support::home_env_lock().lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let work_dir = tempfile::tempdir().unwrap();
    let _home_guard = EnvGuard::set("HOME", home.path().as_os_str().to_os_string());
    let _api_guard = EnvGuard::set(API_ENV, "stub");
    let _cwd_guard = CurrentDirGuard::set(workspace.path());
    write_plugin_fixture_with_permissions(
        workspace.path(),
        "permission-auto-load",
        "lazy",
        &[],
        &["read", "bash"],
        r#"
pi.registerTool({
  name: "permission_echo",
  description: "autoload permission-gated tool",
  parameters: { type: "object", properties: { text: { type: "string" } }, required: ["text"] },
  execute: function (_callId, params) {
    return { plugin: "permission-auto-load", echo: params.text };
  }
});
"#,
    );

    let mut cfg = make_config(work_dir.path(), API_ENV);
    cfg.plugin.auto_load = vec!["permission-auto-load".to_string()];

    let ctx = ChatContext::from_config(cfg).expect("ctx");
    let pm = ctx
        .global_services
        .plugin_manager
        .as_ref()
        .expect("plugin manager");

    assert_eq!(
        list_tool_names(&ctx).await,
        vec!["permission_echo".to_string()]
    );
    let info = pm
        .get_plugin("permission-auto-load")
        .expect("auto-loaded permission plugin");
    assert!(
        info.loaded_at > 0,
        "auto-load should execute the plugin even when requiredPermissions is non-empty"
    );
}

#[test]
#[serial(env_lock)]
fn from_config_reuses_scope_services_and_isolates_session_runtime_state() {
    const API_ENV: &str = "TOMCAT_RUNTIME_SPLIT_TEST_KEY";

    let _home_lock = crate::test_support::home_env_lock().lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let work_dir = tempfile::tempdir().unwrap();
    let _home_guard = EnvGuard::set("HOME", home.path().as_os_str().to_os_string());
    let _api_guard = EnvGuard::set(API_ENV, "stub");
    let _cwd_guard = CurrentDirGuard::set(workspace.path());

    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(work_dir.path().to_string_lossy().to_string());
    cfg.llm.api_key_env = Some(API_ENV.to_string());

    let ctx1 = ChatContext::from_config(cfg.clone()).expect("ctx1");
    let ctx2 = ChatContext::from_config(cfg).expect("ctx2");

    assert!(
        Arc::ptr_eq(
            &ctx1.scope_services.checkpoint_switcher,
            &ctx2.scope_services.checkpoint_switcher
        ),
        "同一 work_tree 下应复用 checkpoint store"
    );
    assert!(
        Arc::ptr_eq(
            &ctx1.global_services.tool_registry,
            &ctx2.global_services.tool_registry
        ),
        "同一 work_tree 下应复用插件 ToolRegistry"
    );
    assert!(
        Arc::ptr_eq(
            &ctx1.global_services.event_bus,
            &ctx2.global_services.event_bus
        ),
        "同一 work_tree 下应复用 scope 事件总线"
    );
    let pm1 = ctx1
        .global_services
        .plugin_manager
        .as_ref()
        .expect("ctx1 plugin manager");
    let pm2 = ctx2
        .global_services
        .plugin_manager
        .as_ref()
        .expect("ctx2 plugin manager");
    assert!(
        Arc::ptr_eq(pm1, pm2),
        "同一 work_tree 下应复用 PluginManager"
    );
    assert!(
        Arc::ptr_eq(
            &ctx1.scope_services.skill_set,
            &ctx2.scope_services.skill_set
        ),
        "同一 work_tree 下应复用 scope 级 skill_set"
    );
    assert!(
        !Arc::ptr_eq(
            &ctx1.session_runtime.read_file_state,
            &ctx2.session_runtime.read_file_state
        ),
        "不同 ChatContext/session runtime 不应共享 read_file_state"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(env_lock)]
async fn sessions_in_same_project_share_scope_container() {
    const API_ENV: &str = "TOMCAT_SCOPE_SHARE_TEST_KEY";

    let _home_lock = crate::test_support::home_env_lock().lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let work_dir = tempfile::tempdir().unwrap();
    let _home_guard = EnvGuard::set("HOME", home.path().as_os_str().to_os_string());
    let _api_guard = EnvGuard::set(API_ENV, "stub");
    let _cwd_guard = CurrentDirGuard::set(workspace.path());
    write_plugin_fixture(
        workspace.path(),
        "shared-scope-plugin",
        "lazy",
        &["scope_echo"],
        r#"
pi.registerTool({
  name: "scope_echo",
  description: "shared scope echo",
  parameters: { type: "object", properties: { text: { type: "string" } }, required: ["text"] },
  execute: function (_callId, params) {
    return { plugin: "shared-scope-plugin", echo: params.text };
  }
});
"#,
    );

    let cfg = make_config(work_dir.path(), API_ENV);
    let ctx1 = ChatContext::from_config(cfg.clone()).expect("ctx1");
    let session1 = current_session_id(&ctx1);
    let next = ctx1
        .session_runtime
        .session
        .new_current_session(Some(workspace.path().to_string_lossy().to_string()))
        .expect("create second session");
    let ctx2 = ChatContext::from_config(cfg).expect("ctx2");
    let session2 = current_session_id(&ctx2);
    assert_ne!(
        session1, session2,
        "same project test should use distinct sessions"
    );
    assert_eq!(session2, next.session_id);

    assert!(
        Arc::ptr_eq(
            &ctx1.global_services.tool_registry,
            &ctx2.global_services.tool_registry
        ),
        "同一 project 的不同 session 应复用 ToolRegistry"
    );
    assert!(
        Arc::ptr_eq(
            &ctx1.global_services.event_bus,
            &ctx2.global_services.event_bus
        ),
        "同一 project 的不同 session 应复用事件总线"
    );
    let pm1 = ctx1
        .global_services
        .plugin_manager
        .as_ref()
        .expect("ctx1 plugin manager");
    let pm2 = ctx2
        .global_services
        .plugin_manager
        .as_ref()
        .expect("ctx2 plugin manager");
    assert!(
        Arc::ptr_eq(pm1, pm2),
        "同一 project 的不同 session 应复用 PluginManager"
    );
    assert_eq!(
        list_tool_names(&ctx1).await,
        list_tool_names(&ctx2).await,
        "共享 scope 容器时工具视图应完全一致"
    );
    assert!(
        !Arc::ptr_eq(
            &ctx1.session_runtime.read_file_state,
            &ctx2.session_runtime.read_file_state
        ),
        "session 级运行态仍应隔离"
    );
}

#[test]
#[serial(env_lock)]
fn from_config_prefers_session_cwd_when_reopening_existing_session() {
    const API_ENV: &str = "TOMCAT_RUNTIME_WORKSPACE_CWD_TEST_KEY";

    let _home_lock = crate::test_support::home_env_lock().lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let project_a = tempfile::tempdir().unwrap();
    let project_b = tempfile::tempdir().unwrap();
    let work_dir = tempfile::tempdir().unwrap();
    let _home_guard = EnvGuard::set("HOME", home.path().as_os_str().to_os_string());
    let _api_guard = EnvGuard::set(API_ENV, "stub");

    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(work_dir.path().to_string_lossy().to_string());
    cfg.llm.api_key_env = Some(API_ENV.to_string());
    let expected_a = std::fs::canonicalize(project_a.path()).expect("canonicalize project_a path");

    {
        let _cwd_guard = CurrentDirGuard::set(project_a.path());
        let ctx = ChatContext::from_config(cfg.clone()).expect("ctx from project a");
        assert_eq!(
            ctx.scope_services.agent_workspace_dir, expected_a,
            "首次创建会话时应记录 project_a 作为 workspace"
        );
    }

    {
        let _cwd_guard = CurrentDirGuard::set(project_b.path());
        let reopened = ChatContext::from_config(cfg).expect("reopened ctx");
        assert_eq!(
            reopened.scope_services.agent_workspace_dir, expected_a,
            "重进已有会话时应优先沿用 session.cwd，而不是当前进程 cwd"
        );
    }
}

#[test]
#[serial(env_lock)]
fn from_config_with_code_mode_isolates_scope_runtime_between_projects() {
    const API_ENV: &str = "TOMCAT_RUNTIME_SCOPE_ISOLATION_TEST_KEY";

    let _home_lock = crate::test_support::home_env_lock().lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let project_a = tempfile::tempdir().unwrap();
    let project_b = tempfile::tempdir().unwrap();
    let work_dir = tempfile::tempdir().unwrap();
    let _home_guard = EnvGuard::set("HOME", home.path().as_os_str().to_os_string());
    let _api_guard = EnvGuard::set(API_ENV, "stub");

    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(work_dir.path().to_string_lossy().to_string());
    cfg.llm.api_key_env = Some(API_ENV.to_string());

    let ctx_a = {
        let _cwd_guard = CurrentDirGuard::set(project_a.path());
        ChatContext::from_config_with_mode(cfg.clone(), SessionMode::Code).expect("ctx_a")
    };
    let ctx_b = {
        let _cwd_guard = CurrentDirGuard::set(project_b.path());
        ChatContext::from_config_with_mode(cfg, SessionMode::Code).expect("ctx_b")
    };

    assert!(
        !Arc::ptr_eq(
            &ctx_a.global_services.tool_registry,
            &ctx_b.global_services.tool_registry
        ),
        "不同 project scope 不应共享 ToolRegistry"
    );
    assert!(
        !Arc::ptr_eq(
            &ctx_a.global_services.event_bus,
            &ctx_b.global_services.event_bus
        ),
        "不同 project scope 不应共享事件总线"
    );
    assert!(
        !Arc::ptr_eq(
            &ctx_a.scope_services.skill_set,
            &ctx_b.scope_services.skill_set
        ),
        "不同 project scope 不应共享 skill_set"
    );
    let pm_a = ctx_a
        .global_services
        .plugin_manager
        .as_ref()
        .expect("ctx_a plugin manager");
    let pm_b = ctx_b
        .global_services
        .plugin_manager
        .as_ref()
        .expect("ctx_b plugin manager");
    assert!(
        !Arc::ptr_eq(pm_a, pm_b),
        "不同 project scope 不应共享 PluginManager"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(env_lock)]
async fn different_projects_get_distinct_scope_containers() {
    const API_ENV: &str = "TOMCAT_SCOPE_DISTINCT_TEST_KEY";

    let _home_lock = crate::test_support::home_env_lock().lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let project_a = tempfile::tempdir().unwrap();
    let project_b = tempfile::tempdir().unwrap();
    let work_dir = tempfile::tempdir().unwrap();
    let _home_guard = EnvGuard::set("HOME", home.path().as_os_str().to_os_string());
    let _api_guard = EnvGuard::set(API_ENV, "stub");
    write_plugin_fixture(
        project_a.path(),
        "project-a-plugin",
        "lazy",
        &["tool_a"],
        r#"
pi.registerTool({
  name: "tool_a",
  description: "project a tool",
  parameters: { type: "object", properties: { text: { type: "string" } }, required: ["text"] },
  execute: function (_callId, params) {
    return { plugin: "project-a-plugin", echo: params.text };
  }
});
"#,
    );
    write_plugin_fixture(
        project_b.path(),
        "project-b-plugin",
        "lazy",
        &["tool_b"],
        r#"
pi.registerTool({
  name: "tool_b",
  description: "project b tool",
  parameters: { type: "object", properties: { text: { type: "string" } }, required: ["text"] },
  execute: function (_callId, params) {
    return { plugin: "project-b-plugin", echo: params.text };
  }
});
"#,
    );

    let cfg = make_config(work_dir.path(), API_ENV);
    let ctx_a = {
        let _cwd_guard = CurrentDirGuard::set(project_a.path());
        ChatContext::from_config_with_mode(cfg.clone(), SessionMode::Code).expect("ctx_a")
    };
    let ctx_b = {
        let _cwd_guard = CurrentDirGuard::set(project_b.path());
        ChatContext::from_config_with_mode(cfg, SessionMode::Code).expect("ctx_b")
    };

    assert!(
        !Arc::ptr_eq(
            &ctx_a.global_services.tool_registry,
            &ctx_b.global_services.tool_registry
        ),
        "不同 project scope 不应共享 ToolRegistry"
    );
    assert!(
        !Arc::ptr_eq(
            &ctx_a.global_services.event_bus,
            &ctx_b.global_services.event_bus
        ),
        "不同 project scope 不应共享事件总线"
    );
    let pm_a = ctx_a
        .global_services
        .plugin_manager
        .as_ref()
        .expect("ctx_a plugin manager");
    let pm_b = ctx_b
        .global_services
        .plugin_manager
        .as_ref()
        .expect("ctx_b plugin manager");
    assert!(
        !Arc::ptr_eq(pm_a, pm_b),
        "不同 project scope 不应共享 PluginManager"
    );
    assert_eq!(list_tool_names(&ctx_a).await, vec!["tool_a".to_string()]);
    assert_eq!(list_tool_names(&ctx_b).await, vec!["tool_b".to_string()]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(env_lock)]
async fn registry_shared_across_sessions_same_scope() {
    const API_ENV: &str = "TOMCAT_SCOPE_REGISTRY_SHARE_TEST_KEY";

    let _home_lock = crate::test_support::home_env_lock().lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let work_dir = tempfile::tempdir().unwrap();
    let _home_guard = EnvGuard::set("HOME", home.path().as_os_str().to_os_string());
    let _api_guard = EnvGuard::set(API_ENV, "stub");
    let _cwd_guard = CurrentDirGuard::set(workspace.path());
    write_plugin_fixture(
        workspace.path(),
        "registry-shared-plugin",
        "lazy",
        &["shared_registry_tool"],
        r#"
pi.registerTool({
  name: "shared_registry_tool",
  description: "shared registry tool",
  parameters: { type: "object", properties: { text: { type: "string" } }, required: ["text"] },
  execute: function (_callId, params) {
    return { plugin: "registry-shared-plugin", echo: params.text };
  }
});
"#,
    );

    let cfg = make_config(work_dir.path(), API_ENV);
    let ctx1 = ChatContext::from_config(cfg.clone()).expect("ctx1");
    let session1 = current_session_id(&ctx1);
    ctx1.session_runtime
        .session
        .new_current_session(Some(workspace.path().to_string_lossy().to_string()))
        .expect("create second session");
    let ctx2 = ChatContext::from_config(cfg).expect("ctx2");
    let session2 = current_session_id(&ctx2);
    let tools1 = list_tool_names(&ctx1).await;
    let tools2 = list_tool_names(&ctx2).await;
    assert_eq!(tools1, vec!["shared_registry_tool".to_string()]);
    assert_eq!(tools1, tools2);

    let pm = ctx1
        .global_services
        .plugin_manager
        .as_ref()
        .expect("shared plugin manager");
    pm.end_session(&session1).await.expect("end session1");
    pm.end_session(&session2).await.expect("end session2");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(env_lock)]
async fn registry_isolated_across_project_scopes() {
    const API_ENV: &str = "TOMCAT_SCOPE_REGISTRY_ISOLATION_TEST_KEY";

    let _home_lock = crate::test_support::home_env_lock().lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let project_a = tempfile::tempdir().unwrap();
    let project_b = tempfile::tempdir().unwrap();
    let work_dir = tempfile::tempdir().unwrap();
    let _home_guard = EnvGuard::set("HOME", home.path().as_os_str().to_os_string());
    let _api_guard = EnvGuard::set(API_ENV, "stub");
    write_plugin_fixture(
        project_a.path(),
        "registry-a",
        "lazy",
        &["scope_only_a"],
        r#"
pi.registerTool({
  name: "scope_only_a",
  description: "scope a tool",
  parameters: { type: "object", properties: { text: { type: "string" } }, required: ["text"] },
  execute: function (_callId, params) {
    return { plugin: "registry-a", echo: params.text };
  }
});
"#,
    );
    write_plugin_fixture(
        project_b.path(),
        "registry-b",
        "lazy",
        &["scope_only_b"],
        r#"
pi.registerTool({
  name: "scope_only_b",
  description: "scope b tool",
  parameters: { type: "object", properties: { text: { type: "string" } }, required: ["text"] },
  execute: function (_callId, params) {
    return { plugin: "registry-b", echo: params.text };
  }
});
"#,
    );

    let cfg = make_config(work_dir.path(), API_ENV);
    let ctx_a = {
        let _cwd_guard = CurrentDirGuard::set(project_a.path());
        ChatContext::from_config_with_mode(cfg.clone(), SessionMode::Code).expect("ctx_a")
    };
    let ctx_b = {
        let _cwd_guard = CurrentDirGuard::set(project_b.path());
        ChatContext::from_config_with_mode(cfg, SessionMode::Code).expect("ctx_b")
    };

    assert_eq!(
        list_tool_names(&ctx_a).await,
        vec!["scope_only_a".to_string()]
    );
    assert_eq!(
        list_tool_names(&ctx_b).await,
        vec!["scope_only_b".to_string()]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(env_lock)]
async fn manifest_static_tools_visible_without_vm() {
    const API_ENV: &str = "TOMCAT_STATIC_TOOL_VISIBILITY_TEST_KEY";

    let _home_lock = crate::test_support::home_env_lock().lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let work_dir = tempfile::tempdir().unwrap();
    let _home_guard = EnvGuard::set("HOME", home.path().as_os_str().to_os_string());
    let _api_guard = EnvGuard::set(API_ENV, "stub");
    let _cwd_guard = CurrentDirGuard::set(workspace.path());
    write_plugin_fixture(
        workspace.path(),
        "manifest-static-plugin",
        "lazy",
        &["static_echo"],
        r#"
pi.registerTool({
  name: "static_echo",
  description: "static manifest echo",
  parameters: { type: "object", properties: { text: { type: "string" } }, required: ["text"] },
  execute: function (_callId, params) {
    return { plugin: "manifest-static-plugin", echo: params.text };
  }
});
"#,
    );

    let ctx = ChatContext::from_config(make_config(work_dir.path(), API_ENV)).expect("ctx");
    let session_id = current_session_id(&ctx);
    assert_eq!(list_tool_names(&ctx).await, vec!["static_echo".to_string()]);

    let pm = ctx
        .global_services
        .plugin_manager
        .as_ref()
        .expect("plugin manager");
    assert!(
        !pm.has_session_vm(&session_id, "manifest-static-plugin"),
        "manifest-declared static tools should surface before any VM exists"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(env_lock)]
async fn vm_instance_lazy_created_on_first_use() {
    const API_ENV: &str = "TOMCAT_LAZY_VM_CREATION_TEST_KEY";

    let _home_lock = crate::test_support::home_env_lock().lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let work_dir = tempfile::tempdir().unwrap();
    let _home_guard = EnvGuard::set("HOME", home.path().as_os_str().to_os_string());
    let _api_guard = EnvGuard::set(API_ENV, "stub");
    let _cwd_guard = CurrentDirGuard::set(workspace.path());
    write_plugin_fixture(
        workspace.path(),
        "lazy-tool-plugin",
        "lazy",
        &["lazy_echo"],
        r#"
pi.registerTool({
  name: "lazy_echo",
  description: "lazy echo",
  parameters: { type: "object", properties: { text: { type: "string" } }, required: ["text"] },
  execute: function (_callId, params) {
    return { plugin: "lazy-tool-plugin", echo: params.text };
  }
});
"#,
    );

    let ctx = ChatContext::from_config(make_config(work_dir.path(), API_ENV)).expect("ctx");
    let session_id = current_session_id(&ctx);
    let pm = ctx
        .global_services
        .plugin_manager
        .as_ref()
        .expect("plugin manager");
    let had_vm_before = pm.has_session_vm(&session_id, "lazy-tool-plugin");

    let tool_registry = ctx.global_services.tool_registry.clone();
    let call_outcome = tokio::time::timeout(
        Duration::from_secs(5),
        tool_registry.call_tool(
            "lazy_echo",
            json!({ "text": "hello" }),
            "__test__",
            Some(&session_id),
        ),
    )
    .await;
    let has_vm_after = pm.has_session_vm(&session_id, "lazy-tool-plugin");
    pm.end_session(&session_id).await.expect("end lazy session");

    assert!(!had_vm_before, "lazy static tool should not prestart a VM");
    let result = call_outcome
        .expect("plugin tool call should not hang")
        .expect("call plugin tool through shared registry");
    assert_eq!(
        result
            .get("content")
            .and_then(|value| value.get("plugin"))
            .and_then(|value| value.as_str()),
        Some("lazy-tool-plugin")
    );
    assert!(
        has_vm_after,
        "first tool call should lazily create the plugin VM"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(env_lock)]
async fn session_activation_delivers_session_start_before_first_tool_call() {
    const API_ENV: &str = "TOMCAT_SESSION_START_DELIVERY_TEST_KEY";

    let _home_lock = crate::test_support::home_env_lock().lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let work_dir = tempfile::tempdir().unwrap();
    let _home_guard = EnvGuard::set("HOME", home.path().as_os_str().to_os_string());
    let _api_guard = EnvGuard::set(API_ENV, "stub");
    let _cwd_guard = CurrentDirGuard::set(workspace.path());
    write_plugin_fixture_with_events(
        workspace.path(),
        "session-start-probe-plugin",
        "session",
        &["session_probe"],
        &["session_start"],
        r#"
let started = false;

pi.on("session_start", function () {
  started = true;
});

pi.registerTool({
  name: "session_probe",
  description: "session_start delivery probe",
  parameters: { type: "object", properties: { text: { type: "string" } }, required: ["text"] },
  execute: function (_callId, params) {
    return { plugin: "session-start-probe-plugin", echo: params.text, started: started };
  }
});
"#,
    );

    let ctx = ChatContext::from_config(make_config(work_dir.path(), API_ENV)).expect("ctx");
    let session_id = current_session_id(&ctx);
    let pm = ctx
        .global_services
        .plugin_manager
        .as_ref()
        .expect("plugin manager");
    assert!(
        pm.has_session_vm(&session_id, "session-start-probe-plugin"),
        "activation=session plugin should be prestarted during scope activation"
    );

    let call_outcome = tokio::time::timeout(
        Duration::from_secs(5),
        ctx.global_services.tool_registry.call_tool(
            "session_probe",
            json!({ "text": "probe" }),
            "__test__",
            Some(&session_id),
        ),
    )
    .await;
    pm.end_session(&session_id)
        .await
        .expect("end session_start probe session");

    let result = call_outcome
        .expect("session-start probe tool call should not hang")
        .expect("call session_start probe tool");
    assert_eq!(
        result
            .get("content")
            .and_then(|value| value.get("plugin"))
            .and_then(|value| value.as_str()),
        Some("session-start-probe-plugin")
    );
    assert_eq!(
        result
            .get("content")
            .and_then(|value| value.get("started"))
            .and_then(|value| value.as_bool()),
        Some(true),
        "session_start should flip module state before the first tool call observes it"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(env_lock)]
async fn scope_conflicts_and_activation_quadrants_route_correctly() {
    const API_ENV: &str = "TOMCAT_SCOPE_CONFLICTS_AND_QUADRANTS_TEST_KEY";

    let _home_lock = crate::test_support::home_env_lock().lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let work_dir = tempfile::tempdir().unwrap();
    let _home_guard = EnvGuard::set("HOME", home.path().as_os_str().to_os_string());
    let _api_guard = EnvGuard::set(API_ENV, "stub");
    let _cwd_guard = CurrentDirGuard::set(workspace.path());
    write_plugin_fixture(
        workspace.path(),
        "alpha-plugin",
        "lazy",
        &["shared_tool"],
        r#"
pi.registerTool({
  name: "shared_tool",
  description: "alpha tool",
  parameters: { type: "object", properties: { text: { type: "string" } }, required: ["text"] },
  execute: function (_callId, params) {
    return { plugin: "alpha-plugin", echo: params.text };
  }
});
"#,
    );
    write_plugin_fixture(
        workspace.path(),
        "beta-plugin",
        "lazy",
        &["shared_tool"],
        r#"
pi.registerTool({
  name: "shared_tool",
  description: "beta tool",
  parameters: { type: "object", properties: { text: { type: "string" } }, required: ["text"] },
  execute: function (_callId, params) {
    return { plugin: "beta-plugin", echo: params.text };
  }
});
"#,
    );
    write_plugin_fixture(
        workspace.path(),
        "legacy-lazy-plugin",
        "lazy",
        &[],
        r#"
pi.registerTool({
  name: "legacy_lazy_echo",
  description: "legacy lazy tool",
  parameters: { type: "object", properties: { text: { type: "string" } }, required: ["text"] },
  execute: function (_callId, params) {
    return { plugin: "legacy-lazy-plugin", echo: params.text };
  }
});
"#,
    );
    write_plugin_fixture(
        workspace.path(),
        "legacy-session-plugin",
        "session",
        &[],
        r#"
pi.registerTool({
  name: "legacy_session_echo",
  description: "legacy session tool",
  parameters: { type: "object", properties: { text: { type: "string" } }, required: ["text"] },
  execute: function (_callId, params) {
    return { plugin: "legacy-session-plugin", echo: params.text };
  }
});
"#,
    );
    write_plugin_fixture(
        workspace.path(),
        "static-session-plugin",
        "session",
        &["static_session_echo"],
        r#"
pi.registerTool({
  name: "static_session_echo",
  description: "static session tool",
  parameters: { type: "object", properties: { text: { type: "string" } }, required: ["text"] },
  execute: function (_callId, params) {
    return { plugin: "static-session-plugin", echo: params.text };
  }
});
"#,
    );

    let ctx = ChatContext::from_config(make_config(work_dir.path(), API_ENV)).expect("ctx");
    let session_id = current_session_id(&ctx);
    let pm = ctx
        .global_services
        .plugin_manager
        .as_ref()
        .expect("plugin manager");
    let tool_names = list_tool_names(&ctx).await;
    let has_shared_tool = tool_names.contains(&"shared_tool".to_string());
    let shared_tool_count = tool_names
        .iter()
        .filter(|name| name.as_str() == "shared_tool")
        .count();
    let has_legacy_lazy_tool = tool_names.contains(&"legacy_lazy_echo".to_string());
    let has_legacy_session_tool = tool_names.contains(&"legacy_session_echo".to_string());
    let has_static_session_tool = tool_names.contains(&"static_session_echo".to_string());
    let alpha_started_before = pm.has_session_vm(&session_id, "alpha-plugin");
    let beta_started_before = pm.has_session_vm(&session_id, "beta-plugin");
    let legacy_lazy_started_before = pm.has_session_vm(&session_id, "legacy-lazy-plugin");
    let legacy_session_started_before = pm.has_session_vm(&session_id, "legacy-session-plugin");
    let static_session_started_before = pm.has_session_vm(&session_id, "static-session-plugin");

    let routed_outcome = tokio::time::timeout(
        Duration::from_secs(5),
        ctx.global_services.tool_registry.call_tool(
            "shared_tool",
            json!({ "text": "routed" }),
            "__test__",
            Some(&session_id),
        ),
    )
    .await;
    let alpha_started_after = pm.has_session_vm(&session_id, "alpha-plugin");
    let beta_started_after = pm.has_session_vm(&session_id, "beta-plugin");
    pm.end_session(&session_id)
        .await
        .expect("end routed session");

    assert!(
        has_shared_tool,
        "the first conflicting tool should still register"
    );
    assert_eq!(shared_tool_count, 1, "scope 内重名工具应拒绝后续注册");
    assert!(
        has_legacy_lazy_tool,
        "legacy lazy plugin should preload its dynamic tool during scope activation"
    );
    assert!(
        has_legacy_session_tool,
        "session-activated plugin should expose its tool during prestart"
    );
    assert!(
        has_static_session_tool,
        "static session plugin should expose manifest tool immediately"
    );
    assert!(
        !alpha_started_before,
        "lazy static plugin should wait until first tool call"
    );
    assert!(
        !beta_started_before,
        "rejected conflicting plugin should not be started by someone else's tool route"
    );
    assert!(
        !legacy_lazy_started_before,
        "legacy lazy preload should not leave a session VM behind"
    );
    assert!(
        legacy_session_started_before,
        "activation=session plugin should prestart its session VM"
    );
    assert!(
        static_session_started_before,
        "activation=session with static tools should also prestart its VM"
    );

    let routed = routed_outcome
        .expect("routed plugin tool call should not hang")
        .expect("call routed tool");
    assert_eq!(
        routed
            .get("content")
            .and_then(|value| value.get("plugin"))
            .and_then(|value| value.as_str()),
        Some("alpha-plugin"),
        "toolName should resolve to the first surviving plugin registration"
    );
    assert!(alpha_started_after, "lazy winner should start on first use");
    assert!(
        !beta_started_after,
        "conflicting loser should remain untouched after routed execution"
    );
}
