//! # `tomcat session ...` 子命令
//!
//! 直接传入隔离过的 `AppConfig`（`storage.work_dir` 指向临时目录），覆盖
//! `list` / `new` / `switch` / `delete` / `archive` / `search` 全部分支，
//! 包括对未存在 key 的容错（switch/delete 都视作 noop 不应报错）。

use super::super::*;
use super::mocks::test_config;

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
