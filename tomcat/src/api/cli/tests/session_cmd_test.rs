//! # `tomcat session ...` 子命令
//!
//! 直接传入隔离过的 `AppConfig`（`storage.work_dir` 指向临时目录），覆盖
//! `list` / `new` / `switch` / `delete` / `archive` / `search` 全部分支，
//! 包括对未存在 key 的容错（switch/delete 都视作 noop 不应报错）。

use super::super::*;
use super::mocks::test_config;
use crate::SessionMode;
use serde_json::json;
use std::ffi::OsString;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

fn spawn_delete_404_server() -> (String, Arc<AtomicUsize>, std::thread::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_clone = Arc::clone(&hits);
    let handle = std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            hits_clone.fetch_add(1, Ordering::SeqCst);
            let body = r#"{"error":"not found"}"#;
            let resp = format!(
                "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes());
        }
    });
    (format!("http://{}", addr), hits, handle)
}

struct EnvGuard {
    key: &'static str,
    prev: Option<OsString>,
}

impl EnvGuard {
    fn set(key: &'static str, value: impl Into<OsString>) -> Self {
        let prev = std::env::var_os(key);
        // SAFETY: test-scoped env mutation guarded by the test lock.
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
    _lock: crate::test_support::TestLockGuard<'static>,
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

fn write_session_plugin_fixture(workspace: &Path, plugin_id: &str, activation: &str) {
    let plugin_dir = workspace.join(".tomcat").join("plugins").join(plugin_id);
    fs::create_dir_all(&plugin_dir).expect("create plugin fixture dir");
    let manifest = json!({
        "id": plugin_id,
        "name": plugin_id,
        "version": "0.1.0",
        "description": format!("fixture {plugin_id}"),
        "author": "tests",
        "main": "main.js",
        "requiredPermissions": [],
        "requiredApiVersion": "1.0",
        "tags": [],
        "tools": [],
        "events": ["session_start"],
        "activation": activation
    });
    fs::write(
        plugin_dir.join("plugin.json"),
        serde_json::to_string_pretty(&manifest).expect("serialize plugin manifest"),
    )
    .expect("write plugin manifest");
    fs::write(
        plugin_dir.join("main.js"),
        r#"
pi.on("session_start", function () {});
__pi_start_event_loop();
"#,
    )
    .expect("write plugin main");
}

#[test]
fn run_session_list_empty_returns_ok() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    crate::ensure_work_dir_structure(&cfg).unwrap();
    let r = run_session(
        SessionSub::List {
            scope: Some(SessionScopeArg::Claw),
        },
        &cfg,
    );
    assert!(r.is_ok());
}

#[test]
fn run_session_new_returns_ok() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    crate::ensure_work_dir_structure(&cfg).unwrap();
    let r = run_session(
        SessionSub::New {
            scope: Some(SessionScopeArg::Claw),
        },
        &cfg,
    );
    assert!(r.is_ok());
}

#[test]
fn run_session_list_after_new_returns_ok() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    crate::ensure_work_dir_structure(&cfg).unwrap();
    let _ = run_session(
        SessionSub::New {
            scope: Some(SessionScopeArg::Claw),
        },
        &cfg,
    );
    let r = run_session(
        SessionSub::List {
            scope: Some(SessionScopeArg::Claw),
        },
        &cfg,
    );
    assert!(r.is_ok());
}

#[test]
fn run_session_switch_nonexistent_returns_ok() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    crate::ensure_work_dir_structure(&cfg).unwrap();
    let r = run_session(
        SessionSub::Switch {
            session_id: "nonexistent".to_string(),
            scope: Some(SessionScopeArg::Claw),
        },
        &cfg,
    );
    assert!(r.is_ok());
}

#[test]
fn run_session_switch_existing_returns_ok() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    crate::ensure_work_dir_structure(&cfg).unwrap();
    let _ = run_session(
        SessionSub::New {
            scope: Some(SessionScopeArg::Claw),
        },
        &cfg,
    );
    let mgr = crate::SessionManager::new(crate::resolve_sessions_dir(&cfg).unwrap());
    let first = mgr.current_session_id().unwrap().expect("first session id");
    let _ = run_session(
        SessionSub::New {
            scope: Some(SessionScopeArg::Claw),
        },
        &cfg,
    );
    let second = mgr
        .current_session_id()
        .unwrap()
        .expect("second session id");
    assert_ne!(first, second, "第二次 new 应生成新的 session_id");
    let r = run_session(
        SessionSub::Switch {
            session_id: first.clone(),
            scope: Some(SessionScopeArg::Claw),
        },
        &cfg,
    );
    assert!(r.is_ok());
    assert_eq!(
        mgr.current_session_id().unwrap().as_deref(),
        Some(first.as_str()),
        "switch 后 current 应指向目标 session_id"
    );
}

#[test]
fn run_session_delete_returns_ok() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    crate::ensure_work_dir_structure(&cfg).unwrap();
    let _ = run_session(
        SessionSub::New {
            scope: Some(SessionScopeArg::Claw),
        },
        &cfg,
    );
    let mgr = crate::SessionManager::new(crate::resolve_sessions_dir(&cfg).unwrap());
    let current_id = mgr
        .current_session_id()
        .unwrap()
        .expect("current session id");
    let r = run_session(
        SessionSub::Delete {
            session_id: current_id,
            scope: Some(SessionScopeArg::Claw),
        },
        &cfg,
    );
    assert!(r.is_ok(), "run_session(Delete) failed: {:?}", r);
}

#[test]
fn run_session_archive_returns_ok() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    crate::ensure_work_dir_structure(&cfg).unwrap();
    let _ = run_session(
        SessionSub::New {
            scope: Some(SessionScopeArg::Claw),
        },
        &cfg,
    );
    let mgr = crate::SessionManager::new(crate::resolve_sessions_dir(&cfg).unwrap());
    let current_id = mgr
        .current_session_id()
        .unwrap()
        .expect("current session id");
    let r = run_session(
        SessionSub::Archive {
            session_id: current_id,
            scope: Some(SessionScopeArg::Claw),
        },
        &cfg,
    );
    assert!(r.is_ok());
}

#[test]
fn run_session_delete_triggers_openai_files_cleanup_registry() {
    let (base_url, hits, handle) = spawn_delete_404_server();
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = test_config(dir.path());
    cfg.llm.api_base = Some(base_url);
    cfg.llm.api_key_env = Some("TOMCAT_SESSION_CLEANUP_TEST_KEY".to_string());
    let old_no_proxy = std::env::var("NO_PROXY").ok();
    let old_no_proxy_lower = std::env::var("no_proxy").ok();
    // SAFETY: 测试作用域内强制本地 127.0.0.1 请求绕过代理，避免 mock 请求被外部代理劫持。
    unsafe {
        std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
        std::env::set_var("no_proxy", "127.0.0.1,localhost");
    }
    // SAFETY: 测试内部临时注入 key。
    unsafe { std::env::set_var("TOMCAT_SESSION_CLEANUP_TEST_KEY", "stub") };
    crate::ensure_work_dir_structure(&cfg).unwrap();
    let _ = run_session(
        SessionSub::New {
            scope: Some(SessionScopeArg::Claw),
        },
        &cfg,
    );

    let sessions_path = crate::resolve_sessions_dir(&cfg).unwrap();
    let mgr = crate::SessionManager::new(sessions_path.clone());
    let current_id = mgr
        .current_session_id()
        .unwrap()
        .expect("current session id");
    let registry = crate::core::llm::openai_files::OpenAiFilesRuntime::registry_path_for_session(
        sessions_path.as_path(),
        &current_id,
    );
    std::fs::write(
        &registry,
        r#"{"files":[{"file_id":"file-cli-cleanup","bytes":1,"created_at":1,"reason":"test"}]}"#,
    )
    .unwrap();
    assert!(registry.exists());

    let r = run_session(
        SessionSub::Delete {
            session_id: current_id,
            scope: Some(SessionScopeArg::Claw),
        },
        &cfg,
    );
    assert!(
        r.is_ok(),
        "session delete should still succeed with cleanup"
    );
    assert!(
        !registry.exists(),
        "cleanup 成功后应移除 registry 文件（404 视成功）"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 1, "应发起 1 次 DELETE 请求");
    handle.join().unwrap();
    // SAFETY: 清理测试环境变量。
    unsafe {
        std::env::remove_var("TOMCAT_SESSION_CLEANUP_TEST_KEY");
        match old_no_proxy {
            Some(v) => std::env::set_var("NO_PROXY", v),
            None => std::env::remove_var("NO_PROXY"),
        }
        match old_no_proxy_lower {
            Some(v) => std::env::set_var("no_proxy", v),
            None => std::env::remove_var("no_proxy"),
        }
    };
}

fn assert_session_subcommand_cleans_plugin_vm(build_sub: impl FnOnce(String) -> SessionSub) {
    const API_ENV: &str = "TOMCAT_SESSION_PLUGIN_CLEANUP_TEST_KEY";
    const PLUGIN_ID: &str = "session-cmd-cleanup-plugin";

    let _home_lock = crate::test_support::home_env_lock().lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let work_dir = tempfile::tempdir().unwrap();
    let _home_guard = EnvGuard::set("HOME", home.path().as_os_str().to_os_string());
    let _api_guard = EnvGuard::set(API_ENV, "stub");
    let _cwd_guard = CurrentDirGuard::set(workspace.path());

    write_session_plugin_fixture(workspace.path(), PLUGIN_ID, "session");

    let mut cfg = test_config(work_dir.path());
    cfg.llm.api_key_env = Some(API_ENV.to_string());
    cfg.plugin.auto_load = vec![PLUGIN_ID.to_string()];
    crate::ensure_work_dir_structure(&cfg).unwrap();

    let (rt, ctx) =
        crate::api::cli::build_runtime_and_context(&cfg, SessionMode::Code).expect("build ctx");
    let plugin_manager = ctx
        .global_services
        .plugin_manager
        .as_ref()
        .expect("plugin manager");
    let session_id = ctx
        .session_runtime
        .session
        .current_session_id()
        .unwrap()
        .expect("current session id");
    if !plugin_manager.has_session_vm(&session_id, PLUGIN_ID) {
        rt.block_on(plugin_manager.start_session_vm(&session_id, PLUGIN_ID))
            .expect("start session vm");
    }

    let instance_id = format!("{session_id}/{PLUGIN_ID}");
    assert!(
        plugin_manager.has_session_vm(&session_id, PLUGIN_ID),
        "fixture should have an active session VM before running session command"
    );
    assert!(
        ctx.scope_services
            .scope_container
            .dispatcher
            .get_event_sender(&instance_id)
            .is_some(),
        "session VM should register an event channel before running session command"
    );

    let result = run_session(build_sub(session_id.clone()), &cfg);
    assert!(
        result.is_ok(),
        "session subcommand should succeed while cleaning plugin VM: {result:?}"
    );
    assert!(
        !plugin_manager.has_session_vm(&session_id, PLUGIN_ID),
        "session subcommand should end the target session VM"
    );
    assert!(
        ctx.scope_services
            .scope_container
            .dispatcher
            .get_event_sender(&instance_id)
            .is_none(),
        "session subcommand should remove the target session event channel"
    );
}

#[test]
fn run_session_delete_cleans_plugin_vm_in_current_process() {
    assert_session_subcommand_cleans_plugin_vm(|session_id| SessionSub::Delete {
        session_id,
        scope: Some(SessionScopeArg::Code),
    });
}

#[test]
fn run_session_archive_cleans_plugin_vm_in_current_process() {
    assert_session_subcommand_cleans_plugin_vm(|session_id| SessionSub::Archive {
        session_id,
        scope: Some(SessionScopeArg::Code),
    });
}

#[test]
fn run_session_search_empty_returns_ok() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    crate::ensure_work_dir_structure(&cfg).unwrap();
    let r = run_session(
        SessionSub::Search {
            query: None,
            scope: Some(SessionScopeArg::Claw),
        },
        &cfg,
    );
    assert!(r.is_ok());
}

#[test]
fn run_session_search_with_query_returns_ok() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    crate::ensure_work_dir_structure(&cfg).unwrap();
    let r = run_session(
        SessionSub::Search {
            query: Some("q".to_string()),
            scope: Some(SessionScopeArg::Claw),
        },
        &cfg,
    );
    assert!(r.is_ok());
}
