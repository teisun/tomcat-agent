use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::AppError;

use super::ask_question::ServeAskQuestionBridge;
use super::types::{ControlFrame, OutFrame, ResponseFrame, ServeCommand};
use super::ServeState;

pub(crate) async fn handle_control_or_interrupt(
    state: Arc<ServeState>,
    command: ServeCommand,
) -> Result<bool, AppError> {
    match command {
        ServeCommand::ControlRequest {
            request_id,
            subtype,
            session_id,
            payload: _,
        } => {
            if subtype == "initialize" {
                state.initialized.store(true, Ordering::SeqCst);
                let response = ControlFrame::response(
                    request_id,
                    session_id.or_else(|| state.registry.active_session_id()),
                    serde_json::json!({
                        "protocolVersion": 1,
                        "capabilities": [
                            "prompt",
                            "steer",
                            "follow_up",
                            "get_state",
                            "set_model",
                            "new_session",
                            "switch_session",
                            "get_messages",
                            "close_session",
                            "list_sessions",
                            "interrupt",
                            "ask_question"
                        ],
                        "sessionId": state.registry.active_session_id(),
                    }),
                );
                state.writer.send(OutFrame::Control(response))?;
                return Ok(true);
            }
            state.writer.send(OutFrame::Response(ResponseFrame::error(
                None,
                None,
                format!("unknown_command: control_request/{subtype}"),
            )))?;
            Ok(true)
        }
        ServeCommand::ControlResponse {
            request_id,
            session_id,
            payload,
        } => {
            let frame = ControlFrame::response(request_id, session_id, payload);
            state.ask_question.handle_control_response(&frame)?;
            Ok(true)
        }
        ServeCommand::ControlCancel {
            request_id,
            session_id,
            payload,
        } => {
            let frame = ControlFrame::cancel(request_id, session_id, payload);
            state.ask_question.handle_control_cancel(&frame)?;
            Ok(true)
        }
        ServeCommand::Interrupt { id, session_id } => {
            if !state.initialized.load(Ordering::SeqCst) {
                state.writer.send(OutFrame::Response(ResponseFrame::error(
                    id,
                    session_id,
                    "not_initialized",
                )))?;
                return Ok(true);
            }
            let resolved = state.registry.resolve_session_id(session_id.as_deref())?;
            let slot = state
                .registry
                .get(&resolved)
                .ok_or_else(|| AppError::Config("unknown_session".to_string()))?;
            slot.ctx.session_runtime.cancel_token.lock().cancel();
            slot.ctx.agent_registry.cascade_abort(&resolved);
            state.writer.send(OutFrame::Response(ResponseFrame::ok(
                id,
                Some(resolved),
                Some(serde_json::json!({ "interrupted": true })),
            )))?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

pub(crate) async fn handle_stdin_eof(state: Arc<ServeState>) -> Result<(), AppError> {
    for summary in state.registry.list() {
        if let Some(slot) = state.registry.get(&summary.session_id) {
            slot.ctx.session_runtime.cancel_token.lock().cancel();
            slot.ctx.agent_registry.cascade_abort(&slot.session_id);
        }
    }
    for summary in state.registry.list() {
        if let Some(slot) = state.registry.get(&summary.session_id) {
            let handle = { slot.run_task.lock().take() };
            if let Some(handle) = handle {
                let _ = handle.await;
            }
        }
    }
    Ok(())
}

pub(crate) fn ensure_initialized(state: &ServeState, command: &ServeCommand) -> bool {
    state.initialized.load(Ordering::SeqCst) || !command.requires_initialized()
}

pub(crate) fn ensure_initialized_or_error(
    state: &ServeState,
    command: &ServeCommand,
) -> Result<bool, AppError> {
    if ensure_initialized(state, command) {
        return Ok(true);
    }
    state.writer.send(OutFrame::Response(ResponseFrame::error(
        command.command_id().map(ToOwned::to_owned),
        command.session_id().map(ToOwned::to_owned),
        "not_initialized",
    )))?;
    Ok(false)
}

pub(crate) fn ask_bridge(state: &ServeState) -> &ServeAskQuestionBridge {
    &state.ask_question
}

#[cfg(test)]
mod tests {
    use super::{ensure_initialized_or_error, handle_control_or_interrupt};
    use std::sync::atomic::Ordering;
    use std::sync::Arc;
    use std::time::Duration;

    use serial_test::serial;

    use crate::api::serve::test_support::{
        build_initialized_state_with_streams, install_test_api_key, read_ndjson_lines,
    };
    use crate::api::serve::types::ServeCommand;

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
            .find(|line| {
                line.get("type").and_then(serde_json::Value::as_str) == Some("control_response")
            })
            .unwrap();
        assert_eq!(
            response
                .get("requestId")
                .and_then(serde_json::Value::as_str),
            Some("init-1")
        );
        let payload = response.get("payload").unwrap();
        assert_eq!(
            payload
                .get("protocolVersion")
                .and_then(serde_json::Value::as_i64),
            Some(1)
        );
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
                params: serde_json::Map::new(),
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
}
