use super::*;
use std::sync::Arc;

use serial_test::serial;

#[tokio::test]
#[serial(env_lock)]
async fn new_session_registers_slot_in_registry() {
    let _api_key = install_test_api_key();
    let (state, _buffer, _temp, initial_slot) = build_initialized_state_with_streams(vec![]).await;
    let initial_len = state.registry.len();
    let new_slot = create_session_slot(Arc::clone(&state), NewSessionParams::default(), true)
        .await
        .expect("new session slot");

    state
        .registry
        .insert(Arc::clone(&new_slot))
        .expect("insert new session slot");

    assert_eq!(state.registry.len(), initial_len + 1);
    assert!(state.registry.get(&new_slot.session_id).is_some());

    let system_text = new_slot
        .turn_state
        .lock()
        .as_ref()
        .expect("new session turn state")
        .system_text
        .clone();
    assert!(system_text.contains("Agent workspace directory"));
    assert!(!system_text.contains("Current date and time"));
    assert!(!system_text.contains("{now}"));

    assert!(state
        .registry
        .list()
        .iter()
        .any(|session| session.session_id == new_slot.session_id));
    assert_eq!(
        state.registry.active_session_id().as_deref(),
        Some(initial_slot.session_id.as_str()),
        "insert alone should not implicitly steal active session"
    );
}
