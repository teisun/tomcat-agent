use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

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
    previous: PathBuf,
}

impl CurrentDirGuard {
    fn set(path: &Path) -> Self {
        let previous = std::env::current_dir().expect("current_dir");
        std::env::set_current_dir(path).expect("set_current_dir");
        Self { previous }
    }
}

impl Drop for CurrentDirGuard {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.previous);
    }
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
