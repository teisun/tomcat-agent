//! `serve` 控制通道处理。
//!
//! 当前覆盖：
//! - `initialize` 握手
//! - `interrupt` 软中断
//! - `control_response` / `control_cancel` 回注 `ask_question` 回环

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use crate::AppError;

use super::cleanup_session_slot;
use super::types::{ControlFrame, OutFrame, ResponseFrame, ServeCommand};
use super::ServeState;

/// Keep first paint and the initial session/history fetch ahead of disk-heavy,
/// best-effort cleanup. This runs only after the initialize response is queued.
const STARTUP_HOUSEKEEPING_DELAY: Duration = Duration::from_secs(2);

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
                        "protocolVersion": 2,
                        "serverVersion": env!("CARGO_PKG_VERSION"),
                        "capabilities": [
                            "prompt",
                            "steer",
                            "follow_up",
                            "resume",
                            "retry",
                            "get_state",
                            "compact",
                            "set_plan_mode",
                            "set_model",
                            "set_thinking_level",
                            "set_context_window",
                            "list_models",
                            "upsert_model",
                            "remove_model",
                            "set_provider_key",
                            "list_provider_keys",
                            "list_connectors",
                            "list_connector_tools",
                            "add_connector",
                            "remove_connector",
                            "set_connector_trust",
                            "test_connector",
                            "reload_connector",                            "login_connector",
                            "cancel_login_connector",
                            "logout_connector",

                            "set_connector_tool_filter",
                            "new_session",
                            "switch_session",
                            "get_messages",
                            "list_checkpoints",
                            "ingest_attachment",
                            "retain_attachment_leases",
                            "cache_attachment_thumbnail",
                            "discard_detached_session",
                            "close_session",
                            "list_sessions",
                            "interrupt",
                            "ask_question",
                            "confirmation"
                        ],
                        "sessionId": state.registry.active_session_id(),
                        // 见 `serve::attachment_root`：宿主必须在渲染 webview 之前拿到它。
                        "attachmentRoot": super::attachment_root(&state)
                            .map(|path| path.to_string_lossy().into_owned()),
                    }),
                );
                state.writer.send(OutFrame::Control(response))?;
                let housekeeping_state = Arc::clone(&state);
                tokio::spawn(async move {
                    tokio::time::sleep(STARTUP_HOUSEKEEPING_DELAY).await;
                    let _ = tokio::task::spawn_blocking(move || {
                        super::run_attachment_housekeeping(&housekeeping_state);
                    })
                    .await;
                });
                if let Some(session_id) = state.registry.active_session_id() {
                    if let Some(slot) = state.registry.get(&session_id) {
                        super::resume_pending_ask_question(&state, &slot);
                    }
                }
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
            if !state.confirmation.handle_control_response(&frame)? {
                state.ask_question.handle_control_response(&frame)?;
            }
            Ok(true)
        }
        ServeCommand::ControlCancel {
            request_id,
            session_id,
            payload,
        } => {
            let frame = ControlFrame::cancel(request_id, session_id, payload);
            if !state.confirmation.handle_control_cancel(&frame)? {
                state.ask_question.handle_control_cancel(&frame)?;
            }
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
            state
                .ask_question
                .cancel_live_session(&resolved, "interrupt");
            state
                .confirmation
                .cancel_live_session(&resolved, "interrupt");
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
