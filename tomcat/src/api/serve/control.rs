//! `serve` 控制通道处理。
//!
//! 当前覆盖：
//! - `initialize` 握手
//! - `interrupt` 软中断
//! - `control_response` / `control_cancel` 回注 `ask_question` 回环

use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::AppError;

use super::cleanup_session_slot;
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
                            "set_plan_mode",
                            "set_model",
                        "set_thinking_level",
                            "list_models",
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
            let resolved = match state.registry.resolve_session_id(session_id.as_deref()) {
                Ok(resolved) => resolved,
                Err(error) if is_config_error(&error, "unknown_session") => {
                    state.writer.send(OutFrame::Response(ResponseFrame::error(
                        id,
                        session_id,
                        "unknown_session",
                    )))?;
                    return Ok(true);
                }
                Err(error) => return Err(error),
            };
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

pub(crate) async fn shutdown_all_sessions(state: Arc<ServeState>) -> Result<(), AppError> {
    let slots = state
        .registry
        .list()
        .into_iter()
        .filter_map(|summary| state.registry.get(&summary.session_id))
        .collect::<Vec<_>>();
    for slot in slots {
        cleanup_session_slot(&state, &slot, false, "serve_stdio_shutdown").await?;
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

fn is_config_error(error: &AppError, expected: &str) -> bool {
    matches!(error, AppError::Config(message) if message == expected)
}
