use super::*;
use std::sync::Arc;
use std::time::Duration;

use serial_test::serial;

async fn wait_for_line(
    buffer: &crate::api::serve::test_support::SharedWriterBuffer,
    predicate: impl Fn(&serde_json::Value) -> bool,
) -> Vec<serde_json::Value> {
    for _ in 0..50 {
        let lines = read_ndjson_lines(buffer);
        if lines.iter().any(&predicate) {
            return lines;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    read_ndjson_lines(buffer)
}

#[tokio::test]
#[serial(env_lock)]
async fn dispatch_command_returns_error_frame_and_keeps_loop_alive_after_handler_error() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;
    state.registry.remove(&slot.session_id);

    dispatch_command(
        Arc::clone(&state),
        ServeCommand::GetState {
            id: Some("get-state-missing".to_string()),
            session_id: None,
        },
    )
    .await
    .unwrap();

    let after_error = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("get-state-missing")
    })
    .await;
    let error_response = after_error
        .iter()
        .find(|line| {
            line.get("id").and_then(serde_json::Value::as_str) == Some("get-state-missing")
        })
        .expect("error response");
    assert_eq!(error_response["success"].as_bool(), Some(false));
    assert_eq!(error_response["error"].as_str(), Some("unknown_session"));

    dispatch_command(
        Arc::clone(&state),
        ServeCommand::NewSession {
            id: Some("new-session-after-error".to_string()),
            params: NewSessionParams::default(),
        },
    )
    .await
    .unwrap();

    let after_success = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("new-session-after-error")
    })
    .await;
    let success_response = after_success
        .iter()
        .find(|line| {
            line.get("id").and_then(serde_json::Value::as_str) == Some("new-session-after-error")
        })
        .expect("success response");
    assert_eq!(success_response["success"].as_bool(), Some(true));
}
