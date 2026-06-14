use super::super::runtime_manager::*;
use crate::ext::vm_actor::{VmActorHandle, VmActorState};
use std::sync::atomic::AtomicU8;
use std::sync::Arc;
use std::time::Duration;

fn make_stub_handle() -> VmActorHandle {
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    VmActorHandle {
        cmd_tx: tx,
        state: Arc::new(AtomicU8::new(VmActorState::Created as u8)),
    }
}

#[test]
fn insert_and_get() {
    let mgr = RuntimeManager::new();
    let key = VmRuntimeKey::new("s1", "p1");
    mgr.insert(key.clone(), make_stub_handle());
    assert!(mgr.get(&key).is_some());
}

#[test]
fn remove_returns_handle() {
    let mgr = RuntimeManager::new();
    let key = VmRuntimeKey::new("s1", "p1");
    mgr.insert(key.clone(), make_stub_handle());
    assert!(mgr.remove(&key).is_some());
    assert!(mgr.get(&key).is_none());
}

#[test]
fn remove_session_clears_all_keys_for_session() {
    let mgr = RuntimeManager::new();
    mgr.insert(VmRuntimeKey::new("s1", "p1"), make_stub_handle());
    mgr.insert(VmRuntimeKey::new("s1", "p2"), make_stub_handle());
    mgr.insert(VmRuntimeKey::new("s2", "p1"), make_stub_handle());

    let removed = mgr.remove_session("s1");
    assert_eq!(removed.len(), 2);
    assert_eq!(mgr.len(), 1);
    assert!(mgr.get(&VmRuntimeKey::new("s2", "p1")).is_some());
}

#[test]
fn remove_plugin_clears_all_keys_for_plugin() {
    let mgr = RuntimeManager::new();
    mgr.insert(VmRuntimeKey::new("s1", "p1"), make_stub_handle());
    mgr.insert(VmRuntimeKey::new("s2", "p1"), make_stub_handle());
    mgr.insert(VmRuntimeKey::new("s1", "p2"), make_stub_handle());

    let removed = mgr.remove_plugin("p1");
    assert_eq!(removed.len(), 2);
    assert_eq!(mgr.len(), 1);
    assert!(mgr.get(&VmRuntimeKey::new("s1", "p2")).is_some());
}

#[test]
fn same_plugin_two_sessions_isolated_instances() {
    let mgr = RuntimeManager::new();
    let first = VmRuntimeKey::new("session-a", "shared-plugin");
    let second = VmRuntimeKey::new("session-b", "shared-plugin");
    mgr.insert(first.clone(), make_stub_handle());
    mgr.insert(second.clone(), make_stub_handle());

    assert!(
        mgr.get(&first).is_some(),
        "first session should resolve its VM"
    );
    assert!(
        mgr.get(&second).is_some(),
        "second session should resolve its own VM"
    );

    let removed = mgr.remove_session("session-a");
    assert_eq!(removed.len(), 1);
    assert!(
        mgr.get(&first).is_none(),
        "session-a VM should be removed in isolation"
    );
    assert!(
        mgr.get(&second).is_some(),
        "session-b VM should remain registered after neighbor cleanup"
    );
}

#[test]
fn idle_vm_reclaimed_after_ttl() {
    let mgr = RuntimeManager::new();
    let stale = VmRuntimeKey::new("session-ttl", "plugin-ttl");
    mgr.insert(stale.clone(), make_stub_handle());

    std::thread::sleep(Duration::from_millis(20));
    let reaped = mgr.reap_idle(Duration::from_millis(5));
    assert_eq!(reaped.len(), 1);
    assert_eq!(reaped[0].0, stale);
    assert!(
        mgr.is_empty(),
        "reap_idle should remove expired runtime entries"
    );
}

#[test]
fn concurrent_insert_get() {
    use std::thread;

    let mgr = Arc::new(RuntimeManager::new());
    let mut handles = vec![];

    for i in 0..10 {
        let m = mgr.clone();
        handles.push(thread::spawn(move || {
            let key = VmRuntimeKey::new("sess", format!("p{i}"));
            m.insert(key.clone(), make_stub_handle());
            assert!(m.get(&key).is_some());
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(mgr.len(), 10);
}
