use super::*;
use crate::ext::vm_actor::{VmActorHandle, VmActorState};
use std::sync::atomic::AtomicU8;

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
