//! 并发集成测试：覆盖 session store RMW 持锁与 transcript per-file 串行。

mod common;

use std::collections::HashSet;
use std::sync::{Arc, Barrier};

use tempfile::TempDir;
use tomcat::{SessionManager, TranscriptEntry};

/// A separate process exercises OS file locks rather than a second Rust mutex in this process.
#[test]
fn session_process_worker() {
    let Ok(mode) = std::env::var("TOMCAT_SESSION_LOCK_TEST_MODE") else {
        return;
    };
    let root = std::path::PathBuf::from(std::env::var_os("TOMCAT_SESSION_LOCK_TEST_ROOT").unwrap());
    let manager = SessionManager::new(root);
    if mode == "hold" {
        let id = std::env::var("TOMCAT_SESSION_LOCK_TEST_ID").unwrap();
        manager.pin_session(&id).unwrap();
        let ready = std::env::var_os("TOMCAT_SESSION_LOCK_TEST_READY").unwrap();
        std::fs::write(&ready, b"ready").unwrap();
        loop {
            let mut line = String::new();
            std::io::stdin().read_line(&mut line).unwrap();
            let Some(next) = line.trim().strip_prefix("switch ") else {
                break;
            };
            manager.switch_current_to_session_id(next).unwrap();
            std::fs::write(&ready, next).unwrap();
        }
        manager.release_session_usage();
    } else {
        for _ in 0..25 {
            manager.new_current_session(None).unwrap();
        }
    }
}

struct SessionWorker(std::process::Child);
impl Drop for SessionWorker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn spawn_session_worker(
    root: &std::path::Path,
    mode: &str,
    id: &str,
    ready: &std::path::Path,
) -> SessionWorker {
    SessionWorker(
        std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "session_process_worker", "--nocapture"])
            .env("TOMCAT_SESSION_LOCK_TEST_MODE", mode)
            .env("TOMCAT_SESSION_LOCK_TEST_ROOT", root)
            .env("TOMCAT_SESSION_LOCK_TEST_ID", id)
            .env("TOMCAT_SESSION_LOCK_TEST_READY", ready)
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap(),
    )
}

#[test]
fn independent_process_blocks_delete_and_process_exit_releases_occupancy() {
    let temp = TempDir::new().unwrap();
    let manager = SessionManager::new(temp.path().to_path_buf());
    let entry = manager.new_current_session(None).unwrap();
    let ready = temp.path().join("ready");
    let mut child = spawn_session_worker(temp.path(), "hold", &entry.session_id, &ready);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while !ready.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "child did not acquire occupancy"
        );
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "child exited before acquiring occupancy"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(manager
        .delete_session(&entry.session_id)
        .unwrap_err()
        .to_string()
        .contains("session_in_use"));
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    manager.delete_session(&entry.session_id).unwrap();
    assert!(manager
        .get_session_by_id(&entry.session_id)
        .unwrap()
        .is_none());
}

#[test]
fn independent_process_rebinding_moves_occupancy_without_following_disk_current() {
    use std::io::Write;
    let temp = TempDir::new().unwrap();
    let manager = SessionManager::new(temp.path().into());
    let a = manager.new_current_session(None).unwrap();
    let b = manager.new_current_session(None).unwrap();
    let ready = temp.path().join("ready-rebind");
    let mut child = spawn_session_worker(temp.path(), "hold", &a.session_id, &ready);
    let wait = |expected: &str| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        while std::fs::read_to_string(&ready).ok().as_deref() != Some(expected) {
            assert!(
                std::time::Instant::now() < deadline,
                "child did not publish {expected}"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    };
    wait("ready");
    assert!(manager.delete_session(&a.session_id).is_err());
    writeln!(child.0.stdin.as_mut().unwrap(), "switch {}", b.session_id).unwrap();
    wait(&b.session_id);
    manager.delete_session(&a.session_id).unwrap();
    assert!(manager.get_session_by_id(&a.session_id).unwrap().is_none());
    assert!(manager.delete_session(&b.session_id).is_err());
    writeln!(child.0.stdin.as_mut().unwrap(), "release").unwrap();
    assert!(child.0.wait().unwrap().success());
    manager.delete_session(&b.session_id).unwrap();
    assert!(manager.get_session_by_id(&b.session_id).unwrap().is_none());
}

#[test]
fn independent_process_store_rmw_keeps_all_sessions_across_atomic_renames() {
    let temp = TempDir::new().unwrap();
    let ready = temp.path().join("unused");
    let mut first = spawn_session_worker(temp.path(), "create", "", &ready);
    let mut second = spawn_session_worker(temp.path(), "create", "", &ready);
    assert!(first.0.wait().unwrap().success());
    assert!(second.0.wait().unwrap().success());
    let manager = SessionManager::new(temp.path().into());
    assert_eq!(manager.list_sessions().unwrap().len(), 50);
}

#[test]
fn parallel_create_session_preserves_all_scope_entries() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let tmp = TempDir::new()?;
    let mgr = SessionManager::new(tmp.path().to_path_buf());
    let key = mgr.current_session_key().to_string();
    let barrier = Arc::new(Barrier::new(17));

    let mut handles = Vec::new();
    for idx in 0..16 {
        let mgr = mgr.clone();
        let key = key.clone();
        let barrier = barrier.clone();
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            mgr.create_session(&key, Some(format!("/tmp/concurrency-{idx}")))
                .expect("create_session")
                .session_id
        }));
    }

    barrier.wait();
    let mut ids = HashSet::new();
    for handle in handles {
        ids.insert(handle.join().expect("thread join"));
    }

    let listed = mgr.list_sessions()?;
    let listed_ids: HashSet<String> = listed
        .into_iter()
        .map(|(_, entry)| entry.session_id)
        .collect();
    assert_eq!(ids.len(), 16, "每个线程都应创建唯一 session_id");
    assert_eq!(listed_ids.len(), 16, "scope 内不应丢 session entry");
    assert_eq!(listed_ids, ids, "最终落盘应包含全部并发创建结果");
    Ok(())
}

#[test]
fn parallel_append_custom_entry_keeps_transcript_parseable(
) -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let tmp = TempDir::new()?;
    let mgr = SessionManager::new(tmp.path().to_path_buf());
    let key = mgr.current_session_key().to_string();
    mgr.create_session(&key, None)?;

    let barrier = Arc::new(Barrier::new(17));
    let payload = "x".repeat(16 * 1024);
    let mut handles = Vec::new();
    for idx in 0..16 {
        let mgr = mgr.clone();
        let barrier = barrier.clone();
        let payload = payload.clone();
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            mgr.append_custom_entry(serde_json::json!({
                "customType": "concurrency.test",
                "idx": idx,
                "payload": payload,
            }))
            .expect("append_custom_entry");
        }));
    }

    barrier.wait();
    for handle in handles {
        handle.join().expect("thread join");
    }

    let entries = mgr.get_entries(64)?;
    let custom_entries = entries
        .into_iter()
        .filter(|entry| matches!(entry, TranscriptEntry::Custom(_)))
        .count();
    assert_eq!(
        custom_entries, 16,
        "per-file transcript lock 应保证所有 custom entry 都可被完整解析"
    );
    Ok(())
}

#[test]
fn parallel_update_session_preserves_disjoint_fields() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let tmp = TempDir::new()?;
    let mgr = SessionManager::new(tmp.path().to_path_buf());
    let key = mgr.current_session_key().to_string();
    mgr.create_session(&key, None)?;

    let barrier = Arc::new(Barrier::new(3));
    let mgr_a = mgr.clone();
    let key_a = key.clone();
    let barrier_a = barrier.clone();
    let handle_a = std::thread::spawn(move || {
        barrier_a.wait();
        mgr_a
            .update_session(&key_a, |entry| {
                entry.cwd = Some("/tmp/cwd-a".to_string());
            })
            .expect("update cwd");
    });

    let mgr_b = mgr.clone();
    let key_b = key.clone();
    let barrier_b = barrier.clone();
    let handle_b = std::thread::spawn(move || {
        barrier_b.wait();
        mgr_b
            .update_session(&key_b, |entry| {
                entry.model_override = Some("gpt-5.4".to_string());
            })
            .expect("update model");
    });

    barrier.wait();
    handle_a.join().expect("thread a");
    handle_b.join().expect("thread b");

    let entry = mgr.get_session(&key)?.expect("current session entry");
    assert_eq!(entry.cwd.as_deref(), Some("/tmp/cwd-a"));
    assert_eq!(entry.model_override.as_deref(), Some("gpt-5.4"));
    Ok(())
}

#[test]
fn same_scope_switch_keeps_current_pointer_and_history_consistent(
) -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let tmp = TempDir::new()?;
    let mgr = SessionManager::new(tmp.path().to_path_buf());

    let first = mgr.new_current_session(Some("/tmp/one".to_string()))?;
    let second = mgr.new_current_session(Some("/tmp/two".to_string()))?;
    assert_eq!(
        mgr.current_session_id()?.as_deref(),
        Some(second.session_id.as_str()),
        "new_current_session should repoint current to the newest session"
    );

    mgr.switch_current_to_session_id(&first.session_id)?;
    assert_eq!(
        mgr.current_session_id()?.as_deref(),
        Some(first.session_id.as_str()),
        "switch should repoint current within the same scope"
    );

    let listed_ids: Vec<String> = mgr
        .list_sessions()?
        .into_iter()
        .map(|(_, entry)| entry.session_id)
        .collect();
    assert_eq!(
        listed_ids.len(),
        2,
        "switch should not drop historical sessions"
    );
    assert!(listed_ids.contains(&first.session_id));
    assert!(listed_ids.contains(&second.session_id));
    Ok(())
}
