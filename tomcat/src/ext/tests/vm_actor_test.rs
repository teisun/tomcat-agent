use super::super::vm_actor::*;
use crate::ext::PluginEngine;
use crate::infra::wire;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

#[test]
fn state_roundtrip() {
    for s in [
        VmActorState::Created,
        VmActorState::Running,
        VmActorState::Idle,
        VmActorState::ShuttingDown,
        VmActorState::Stopped,
        VmActorState::Error,
    ] {
        assert_eq!(VmActorState::from_u8(s as u8), s);
    }
}

#[test]
fn event_envelope_serialize() {
    let env = EventEnvelope {
        event_type: wire::vm::WIRE_SESSION_START.into(),
        data: serde_json::json!({"key": "val"}),
        context: serde_json::json!({}),
    };
    let json = serde_json::to_string(&env).unwrap();
    let needle = format!("\"type\":\"{}\"", wire::vm::WIRE_SESSION_START);
    assert!(json.contains(&needle));
}

#[test]
fn handle_state_check() {
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    let state = Arc::new(AtomicU8::new(VmActorState::Created as u8));
    let handle = VmActorHandle {
        instance_id: "state-check".to_string(),
        cmd_tx: tx,
        state: state.clone(),
    };
    assert_eq!(handle.current_state(), VmActorState::Created);
    state.store(VmActorState::Running as u8, Ordering::Relaxed);
    assert_eq!(handle.current_state(), VmActorState::Running);
}

/// Component-level command transport test; real actor lifecycle is exercised
/// through PluginManager in long_lived_vm_tests, without exposing handle identity.
#[tokio::test]
async fn test_vm_actor_handle_dispatch_event_command() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    let handle = VmActorHandle {
        instance_id: "dispatch-commands".into(),
        cmd_tx: tx,
        state: Arc::new(AtomicU8::new(VmActorState::Running as u8)),
    };
    handle.dispatch(VmCommand::Init).await.unwrap();
    handle
        .dispatch(VmCommand::DispatchEvent {
            event_type: "test_event".into(),
            data: serde_json::json!({"seq": 1}),
            context: serde_json::json!({}),
        })
        .await
        .unwrap();
    handle.shutdown().await.unwrap();
    assert!(matches!(rx.try_recv().unwrap(), VmCommand::Init));
    assert!(matches!(
        rx.try_recv().unwrap(),
        VmCommand::DispatchEvent { .. }
    ));
    assert!(matches!(rx.try_recv().unwrap(), VmCommand::Shutdown));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actor_panic_is_caught_and_marked_error() {
    let engine = PluginEngine::global(None).expect("create quickjs engine");
    let mut instance = engine
        .create_instance("panic-actor-test")
        .expect("create quickjs instance");
    instance
        .register_host_binding(
            |_request_json| -> Result<String, crate::infra::error::AppError> {
                panic!("intentional host binding panic");
            },
        )
        .expect("register host binding");

    let temp = tempfile::tempdir().expect("create tempdir for vm_actor test");
    let script_path = temp.path().join("main.js");
    std::fs::write(&script_path, "pi.log('trigger panic');\n")
        .expect("write vm_actor panic test script");

    let handle = VmActor::spawn(instance, script_path, 8);
    handle
        .dispatch(VmCommand::Init)
        .await
        .expect("send init command");

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        if handle.current_state() == VmActorState::Error {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "VmActor should enter Error after host binding panic"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}
