//! `serve` 命令分发层。
//!
//! 负责把 `ServeCommand` 翻译为：
//! - 会话路由
//! - turn 启动 / 排队
//! - 响应帧与错误帧
//! - `ChatMessage` 多模态装配

use std::sync::Arc;

use futures_util::FutureExt;

use crate::core::llm::{ChatMessage, ChatMessageContentPart};
use crate::AppError;

use super::control;
use super::event_pump;
use super::types::{
    OutFrame, ResponseFrame, ServeAttachment, ServeAttachmentKind, ServeCommand,
    ServeMessageParams,
};
use super::{create_session_slot, register_slot_hooks, run_slot_turn, ServeState};

pub(crate) async fn handle_command(
    state: Arc<ServeState>,
    command: ServeCommand,
) -> Result<(), AppError> {
    if control::handle_control_or_interrupt(Arc::clone(&state), command.clone()).await? {
        return Ok(());
    }
    if !control::ensure_initialized_or_error(&state, &command)? {
        return Ok(());
    }

    match command {
        ServeCommand::Prompt {
            id,
            session_id,
            text,
            params,
        } => {
            let Some(slot) = resolve_slot_or_error(&state, id.clone(), session_id.clone()).await?
            else {
                return Ok(());
            };
            if slot.is_busy() {
                send_error(&state, id, session_id, "busy")?;
                return Ok(());
            }
            let input_message = match build_user_message(text, &params) {
                Ok(message) => message,
                Err(error) => {
                    send_error(&state, id, Some(slot.session_id.clone()), error)?;
                    return Ok(());
                }
            };
            start_turn(state, slot, id, input_message).await?;
        }
        ServeCommand::Steer {
            id,
            session_id,
            text,
            params: _,
        } => {
            let Some(slot) = resolve_slot_or_error(&state, id.clone(), session_id.clone()).await?
            else {
                return Ok(());
            };
            if slot.is_busy() {
                slot.ctx
                    .session_runtime
                    .steering_queue
                    .lock()
                    .push(ChatMessage::steering(text));
                state.writer.send(OutFrame::Response(ResponseFrame::ok(
                    id,
                    Some(slot.session_id.clone()),
                    Some(serde_json::json!({ "queued": true })),
                )))?;
                return Ok(());
            }
            start_turn(state, slot, id, ChatMessage::steering(text)).await?;
        }
        ServeCommand::FollowUp {
            id,
            session_id,
            text,
            params,
        } => {
            let Some(slot) = resolve_slot_or_error(&state, id.clone(), session_id.clone()).await?
            else {
                return Ok(());
            };
            let input_message = match build_user_message(text, &params) {
                Ok(message) => message,
                Err(error) => {
                    send_error(&state, id, Some(slot.session_id.clone()), error)?;
                    return Ok(());
                }
            };
            if slot.is_busy() {
                slot.ctx
                    .session_runtime
                    .follow_up_queue
                    .lock()
                    .push(input_message);
                state.writer.send(OutFrame::Response(ResponseFrame::ok(
                    id,
                    Some(slot.session_id.clone()),
                    Some(serde_json::json!({ "queued": true })),
                )))?;
                return Ok(());
            }
            start_turn(state, slot, id, input_message).await?;
        }
        ServeCommand::NewSession { id, params } => {
            if state.registry.len() >= state.registry.max_sessions() {
                send_error(&state, id, None, "too_many_sessions")?;
                return Ok(());
            }
            match create_session_slot(Arc::clone(&state), params, true).await {
                Ok(slot) => {
                    let session_id = slot.session_id.clone();
                    match state.registry.insert(Arc::clone(&slot)) {
                        Ok(()) => {}
                        Err(error) if is_config_error(&error, "too_many_sessions") => {
                            rollback_created_session(&slot)?;
                            send_error(&state, id, None, "too_many_sessions")?;
                            return Ok(());
                        }
                        Err(error) => {
                            rollback_created_session(&slot)?;
                            return Err(error);
                        }
                    }
                    register_slot_hooks(&state, &slot);
                    state.registry.set_active_session(&session_id)?;
                    state.writer.send(OutFrame::Response(ResponseFrame::ok(
                        id,
                        Some(session_id.clone()),
                        Some(serde_json::json!({ "sessionId": session_id })),
                    )))?;
                }
                Err(error) if is_config_error(&error, "too_many_sessions") => {
                    send_error(&state, id, None, "too_many_sessions")?;
                }
                Err(error) => return Err(error),
            }
        }
        ServeCommand::SwitchSession { id, session_id } => {
            if let Err(error) = state.registry.set_active_session(&session_id) {
                if is_config_error(&error, "unknown_session") {
                    send_error(&state, id, Some(session_id), "unknown_session")?;
                    return Ok(());
                }
                return Err(error);
            }
            state.writer.send(OutFrame::Response(ResponseFrame::ok(
                id,
                Some(session_id.clone()),
                Some(serde_json::json!({ "activeSessionId": session_id })),
            )))?;
        }
        ServeCommand::GetMessages {
            id,
            session_id,
            params,
        } => {
            let Some(slot) = resolve_slot_or_error(&state, id.clone(), session_id.clone()).await?
            else {
                return Ok(());
            };
            let cap = params
                .limit
                .or_else(|| params.last_n_turns.map(|turns| turns.saturating_mul(32)))
                .unwrap_or(128);
            let header = slot
                .ctx
                .session_runtime
                .session
                .read_session_header()
                .map_err(|error| {
                    AppError::Config(format!("read session header failed: {error}"))
                })?;
            let entries = slot
                .ctx
                .session_runtime
                .session
                .get_entries(cap)
                .map_err(|error| {
                    AppError::Config(format!("read session entries failed: {error}"))
                })?;
            state.writer.send(OutFrame::Response(ResponseFrame::ok(
                id,
                Some(slot.session_id.clone()),
                Some(serde_json::json!({
                    "sessionId": slot.session_id,
                    "header": header,
                    "messages": entries,
                    // TODO(next): wire up real seq/upToSeq when Phase-2 visibility resync lands.
                    "upToSeq": serde_json::Value::Null
                })),
            )))?;
        }
        ServeCommand::ListSessions { id } => {
            state.writer.send(OutFrame::Response(ResponseFrame::ok(
                id,
                state.registry.active_session_id(),
                Some(serde_json::json!({
                    "activeSessionId": state.registry.active_session_id(),
                    "sessions": state.registry.list().into_iter().map(|session| {
                        serde_json::json!({
                            "sessionId": session.session_id,
                            "busy": session.busy,
                        })
                    }).collect::<Vec<_>>()
                })),
            )))?;
        }
        ServeCommand::GetState { id, session_id } => {
            let Some(slot) = resolve_slot_or_error(&state, id.clone(), session_id.clone()).await?
            else {
                return Ok(());
            };
            let entry = slot.ctx.session_runtime.session.current_session_entry()?;
            let model = slot.ctx.effective_model(entry.as_ref());
            state.writer.send(OutFrame::Response(ResponseFrame::ok(
                id,
                Some(slot.session_id.clone()),
                Some(serde_json::json!({
                    "sessionId": slot.session_id,
                    "busy": slot.is_busy(),
                    "mode": match slot.mode { crate::SessionMode::Code => "code", crate::SessionMode::Claw => "claw" },
                    "cwd": slot.cwd,
                    "model": model,
                })),
            )))?;
        }
        ServeCommand::SetModel {
            id,
            session_id,
            model,
        } => {
            let Some(slot) = resolve_slot_or_error(&state, id.clone(), session_id.clone()).await?
            else {
                return Ok(());
            };
            slot.ctx
                .session_runtime
                .session
                .switch_current_model(None, Some(model.as_str()))?;
            state.writer.send(OutFrame::Response(ResponseFrame::ok(
                id,
                Some(slot.session_id.clone()),
                Some(serde_json::json!({
                    "sessionId": slot.session_id,
                    "model": model,
                })),
            )))?;
        }
        ServeCommand::CloseSession { id, session_id } => {
            let Some(slot) = resolve_slot_or_error(&state, id.clone(), session_id.clone()).await?
            else {
                return Ok(());
            };
            slot.ctx.session_runtime.cancel_token.lock().cancel();
            slot.ctx.agent_registry.cascade_abort(&slot.session_id);
            let handle = { slot.run_task.lock().take() };
            if let Some(handle) = handle {
                let _ = handle.await;
            }
            event_pump::unregister_session_event_pump(&slot);
            control::ask_bridge(&state).clear_session(&slot.session_id);
            state.shared_event_bus.unregister_session_bus(&slot.session_id);
            slot.ctx.shutdown_completion_subscriber();
            state.registry.remove(&slot.session_id);
            state.writer.send(OutFrame::Response(ResponseFrame::ok(
                id,
                Some(slot.session_id.clone()),
                Some(serde_json::json!({ "closed": true, "sessionId": slot.session_id })),
            )))?;
        }
        other => {
            send_error(
                &state,
                other.command_id().map(ToOwned::to_owned),
                other.session_id().map(ToOwned::to_owned),
                format!("unknown_command: {}", other.wire_type()),
            )?;
        }
    }

    Ok(())
}

async fn resolve_slot_or_error(
    state: &ServeState,
    id: Option<String>,
    session_id: Option<String>,
) -> Result<Option<Arc<super::registry::SessionSlot>>, AppError> {
    let resolved = match state.registry.resolve_session_id(session_id.as_deref()) {
        Ok(resolved) => resolved,
        Err(error) if is_config_error(&error, "unknown_session") => {
            send_error(state, id, session_id, "unknown_session")?;
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    Ok(state.registry.get(&resolved))
}

async fn start_turn(
    state: Arc<ServeState>,
    slot: Arc<super::registry::SessionSlot>,
    id: Option<String>,
    input_message: ChatMessage,
) -> Result<(), AppError> {
    if !slot.mark_busy() {
        send_error(&state, id, Some(slot.session_id.clone()), "busy")?;
        return Ok(());
    }

    let turn_token = tokio_util::sync::CancellationToken::new();
    {
        let mut guard = slot.ctx.session_runtime.cancel_token.lock();
        *guard = turn_token.clone();
    }

    state.registry.set_active_session(&slot.session_id)?;
    state.writer.send(OutFrame::Response(ResponseFrame::ok(
        id,
        Some(slot.session_id.clone()),
        Some(serde_json::json!({ "accepted": true })),
    )))?;

    let slot_for_task = Arc::clone(&slot);
    let state_for_task = Arc::clone(&state);
    let handle = tokio::spawn(async move {
        let result = std::panic::AssertUnwindSafe(run_slot_turn(
            Arc::clone(&slot_for_task),
            input_message,
            turn_token,
        ))
        .catch_unwind()
        .await;
        match result {
            Ok(Ok(_outcome)) => {}
            Ok(Err(error)) => {
                tracing::error!(session_id = %slot_for_task.session_id, error = %error, "serve session turn failed");
            }
            Err(_) => {
                let frame = OutFrame::Event(serde_json::json!({
                    "type": "agent_end",
                    "sessionId": slot_for_task.session_id,
                    "messages": [],
                    "error": "serve session task panicked"
                }));
                let _ = state_for_task.writer.send(frame);
            }
        }
        slot_for_task.mark_idle();
        *slot_for_task.run_task.lock() = None;
    });
    *slot.run_task.lock() = Some(handle);
    Ok(())
}

fn rollback_created_session(slot: &super::registry::SessionSlot) -> Result<(), AppError> {
    slot.ctx
        .session_runtime
        .session
        .delete_session(&slot.session_id)
}

fn build_user_message(text: String, params: &ServeMessageParams) -> Result<ChatMessage, String> {
    if params.attachments.is_empty() {
        return Ok(ChatMessage::user(text));
    }

    let mut parts = Vec::with_capacity(1 + params.attachments.len());
    parts.push(ChatMessageContentPart::text(text));
    for attachment in &params.attachments {
        parts.push(parse_attachment_part(attachment)?);
    }
    Ok(ChatMessage::user_with_parts(parts))
}

fn parse_attachment_part(attachment: &ServeAttachment) -> Result<ChatMessageContentPart, String> {
    match (&attachment.data_base64, &attachment.file_id) {
        (Some(_), Some(_)) => {
            return Err(
                "invalid_attachment: dataBase64 and fileId are mutually exclusive".to_string(),
            );
        }
        (None, None) => {
            return Err(
                "invalid_attachment: exactly one of dataBase64 or fileId is required".to_string(),
            );
        }
        _ => {}
    }

    match attachment.kind {
        ServeAttachmentKind::Image => {
            if let Some(file_id) = attachment.file_id.clone() {
                ChatMessageContentPart::image_file_id(file_id)
                    .map_err(|error| format!("invalid_attachment: {error}"))
            } else {
                let mime_type = attachment
                    .mime_type
                    .clone()
                    .ok_or_else(|| "invalid_attachment: image attachment requires mimeType".to_string())?;
                let data = attachment
                    .data_base64
                    .clone()
                    .ok_or_else(|| "invalid_attachment: image attachment requires dataBase64".to_string())?;
                ChatMessageContentPart::image_base64_data(mime_type, data)
                    .map_err(|error| format!("invalid_attachment: {error}"))
            }
        }
        ServeAttachmentKind::File => {
            if let Some(file_id) = attachment.file_id.clone() {
                ChatMessageContentPart::file_file_id(file_id, None)
                    .map_err(|error| format!("invalid_attachment: {error}"))
            } else {
                let mime_type = attachment
                    .mime_type
                    .clone()
                    .ok_or_else(|| "invalid_attachment: file attachment requires mimeType".to_string())?;
                let data = attachment
                    .data_base64
                    .clone()
                    .ok_or_else(|| "invalid_attachment: file attachment requires dataBase64".to_string())?;
                ChatMessageContentPart::file_base64_data(None, mime_type, data)
                    .map_err(|error| format!("invalid_attachment: {error}"))
            }
        }
    }
}

fn send_error(
    state: &ServeState,
    id: Option<String>,
    session_id: Option<String>,
    error: impl Into<String>,
) -> Result<(), AppError> {
    state.writer.send(OutFrame::Response(ResponseFrame::error(
        id, session_id, error,
    )))
}

fn is_config_error(error: &AppError, expected: &str) -> bool {
    matches!(error, AppError::Config(message) if message == expected)
}
