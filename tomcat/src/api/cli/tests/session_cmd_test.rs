//! # `tomcat session ...` 子命令
//!
//! 直接传入隔离过的 `AppConfig`（`storage.work_dir` 指向临时目录），覆盖
//! `list` / `new` / `switch` / `delete` / `archive` / `search` 全部分支，
//! 包括对未存在 key 的容错（switch/delete 都视作 noop 不应报错）。

use super::super::*;
use super::mocks::test_config;
use std::io::{Read, Write};
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

#[test]
fn run_session_list_empty_returns_ok() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    crate::ensure_work_dir_structure(&cfg).unwrap();
    let r = run_session(SessionSub::List, &cfg);
    assert!(r.is_ok());
}

#[test]
fn run_session_new_returns_ok() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    crate::ensure_work_dir_structure(&cfg).unwrap();
    let r = run_session(SessionSub::New, &cfg);
    assert!(r.is_ok());
}

#[test]
fn run_session_list_after_new_returns_ok() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    crate::ensure_work_dir_structure(&cfg).unwrap();
    let _ = run_session(SessionSub::New, &cfg);
    let r = run_session(SessionSub::List, &cfg);
    assert!(r.is_ok());
}

#[test]
fn run_session_switch_nonexistent_returns_ok() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    crate::ensure_work_dir_structure(&cfg).unwrap();
    let r = run_session(
        SessionSub::Switch {
            key: "nonexistent".to_string(),
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
    let _ = run_session(SessionSub::New, &cfg);
    let r = run_session(
        SessionSub::Switch {
            key: crate::DEFAULT_SESSION_KEY.to_string(),
        },
        &cfg,
    );
    assert!(r.is_ok());
}

#[test]
fn run_session_delete_returns_ok() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    crate::ensure_work_dir_structure(&cfg).unwrap();
    let _ = run_session(SessionSub::New, &cfg);
    let r = run_session(
        SessionSub::Delete {
            key: crate::DEFAULT_SESSION_KEY.to_string(),
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
    let _ = run_session(SessionSub::New, &cfg);
    let r = run_session(
        SessionSub::Archive {
            key: crate::DEFAULT_SESSION_KEY.to_string(),
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
    // SAFETY: 测试内部临时注入 key。
    unsafe { std::env::set_var("TOMCAT_SESSION_CLEANUP_TEST_KEY", "stub") };
    crate::ensure_work_dir_structure(&cfg).unwrap();
    let _ = run_session(SessionSub::New, &cfg);

    let sessions_path = crate::resolve_sessions_dir(&cfg).unwrap();
    let registry = crate::core::llm::openai_files::OpenAiFilesRuntime::registry_path_for_session(
        sessions_path.as_path(),
        crate::DEFAULT_SESSION_KEY,
    );
    std::fs::write(
        &registry,
        r#"{"files":[{"file_id":"file-cli-cleanup","bytes":1,"created_at":1,"reason":"test"}]}"#,
    )
    .unwrap();
    assert!(registry.exists());

    let r = run_session(
        SessionSub::Delete {
            key: crate::DEFAULT_SESSION_KEY.to_string(),
        },
        &cfg,
    );
    assert!(r.is_ok(), "session delete should still succeed with cleanup");
    assert!(
        !registry.exists(),
        "cleanup 成功后应移除 registry 文件（404 视成功）"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 1, "应发起 1 次 DELETE 请求");
    handle.join().unwrap();
    // SAFETY: 清理测试环境变量。
    unsafe { std::env::remove_var("TOMCAT_SESSION_CLEANUP_TEST_KEY") };
}

#[test]
fn run_session_search_empty_returns_ok() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    crate::ensure_work_dir_structure(&cfg).unwrap();
    let r = run_session(SessionSub::Search { query: None }, &cfg);
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
        },
        &cfg,
    );
    assert!(r.is_ok());
}
