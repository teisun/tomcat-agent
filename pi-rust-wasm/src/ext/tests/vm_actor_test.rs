use super::super::vm_actor::*;
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
        cmd_tx: tx,
        state: state.clone(),
    };
    assert_eq!(handle.current_state(), VmActorState::Created);
    state.store(VmActorState::Running as u8, Ordering::Relaxed);
    assert_eq!(handle.current_state(), VmActorState::Running);
}
