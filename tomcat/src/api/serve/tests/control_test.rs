use super::*;
use std::sync::Arc;
use std::sync::atomic::Ordering;
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
async fn serve_initialize_control_request_sets_ready_state() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, _slot) = build_initialized_state_with_streams(vec![]).await;
    state.initialized.store(false, Ordering::SeqCst);

    let handled = handle_control_or_interrupt(
        Arc::clone(&state),
        ServeCommand::ControlRequest {
            request_id: "init-1".to_string(),
            subtype: "initialize".to_string(),
            session_id: None,
            payload: serde_json::Value::Null,
        },
    )
    .await
    .unwrap();
    assert!(handled);
    assert!(state.initialized.load(Ordering::SeqCst));

    let lines = wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("control_response")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| line.get("type").and_then(serde_json::Value::as_str) == Some("control_response"))
        .unwrap();
    assert_eq!(
        response.get("requestId").and_then(serde_json::Value::as_str),
        Some("init-1")
    );
    let payload = response.get("payload").unwrap();
    assert_eq!(
        payload
            .get("protocolVersion")
            .and_then(serde_json::Value::as_i64),
        Some(1)
    );
    let capabilities = payload["capabilities"].as_array().expect("capabilities array");
    let capability_names = capabilities
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect::<Vec<_>>();
    for expected in [
        "prompt",
        "steer",
        "follow_up",
        "new_session",
        "interrupt",
        "ask_question",
    ] {
        assert!(
            capability_names.contains(&expected),
            "missing capability {expected:?} in {capability_names:?}"
        );
    }
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_not_initialized_returns_error_response() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;
    state.initialized.store(false, Ordering::SeqCst);

    let allowed = ensure_initialized_or_error(
        &state,
        &ServeCommand::Prompt {
            id: Some("prompt-1".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "hello".to_string(),
            params: ServeMessageParams::default(),
        },
    )
    .unwrap();
    assert!(!allowed);

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("prompt-1")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("prompt-1"))
        .unwrap();
    assert_eq!(
        response.get("error").and_then(serde_json::Value::as_str),
        Some("not_initialized")
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_interrupt_cancels_target_session() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;

    let handled = handle_control_or_interrupt(
        Arc::clone(&state),
        ServeCommand::Interrupt {
            id: Some("interrupt-1".to_string()),
            session_id: Some(slot.session_id.clone()),
        },
    )
    .await
    .unwrap();
    assert!(handled);
    assert!(slot.ctx.session_runtime.cancel_token.lock().is_cancelled());

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("interrupt-1")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("interrupt-1"))
        .unwrap();
    assert_eq!(
        response.get("success").and_then(serde_json::Value::as_bool),
        Some(true)
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_interrupt_unknown_session_returns_error_response() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, _slot) = build_initialized_state_with_streams(vec![]).await;

    let handled = handle_control_or_interrupt(
        Arc::clone(&state),
        ServeCommand::Interrupt {
            id: Some("interrupt-missing".to_string()),
            session_id: Some("missing-session".to_string()),
        },
    )
    .await
    .unwrap();
    assert!(handled);

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("interrupt-missing")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("interrupt-missing"))
        .unwrap();
    assert_eq!(response.get("success").and_then(serde_json::Value::as_bool), Some(false));
    assert_eq!(
        response.get("error").and_then(serde_json::Value::as_str),
        Some("unknown_session")
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_unknown_control_subtype_returns_unknown_command_error() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, _slot) = build_initialized_state_with_streams(vec![]).await;

    let handled = handle_control_or_interrupt(
        Arc::clone(&state),
        ServeCommand::ControlRequest {
            request_id: "weird-1".to_string(),
            subtype: "mystery".to_string(),
            session_id: None,
            payload: serde_json::json!({}),
        },
    )
    .await
    .unwrap();
    assert!(handled);

    let lines = wait_for_line(&buffer, |line| {
        line.get("error").and_then(serde_json::Value::as_str)
            == Some("unknown_command: control_request/mystery")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| {
            line.get("error").and_then(serde_json::Value::as_str)
                == Some("unknown_command: control_request/mystery")
        })
        .unwrap();
    assert_eq!(response.get("success").and_then(serde_json::Value::as_bool), Some(false));
}
