use std::sync::Arc;

use futures_util::FutureExt;

use crate::api::chat::panels::AskQuestionResult;
use crate::core::llm::ChatMessage;
use crate::{AppError, TranscriptEntry};

use super::control;
use super::event_pump;
use super::types::{OutFrame, ResponseFrame, ServeCommand};
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
            ..
        } => {
            let Some(slot) = resolve_slot_or_error(&state, id.clone(), session_id.clone()).await?
            else {
                return Ok(());
            };
            if slot.is_busy() {
                send_error(&state, id, session_id, "busy")?;
                return Ok(());
            }
            start_turn(state, slot, id, text).await?;
        }
        ServeCommand::Steer {
            id,
            session_id,
            text,
            ..
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
            start_turn(state, slot, id, text).await?;
        }
        ServeCommand::FollowUp {
            id,
            session_id,
            text,
            ..
        } => {
            let Some(slot) = resolve_slot_or_error(&state, id.clone(), session_id.clone()).await?
            else {
                return Ok(());
            };
            if slot.is_busy() {
                slot.ctx
                    .session_runtime
                    .follow_up_queue
                    .lock()
                    .push(ChatMessage::user(text));
                state.writer.send(OutFrame::Response(ResponseFrame::ok(
                    id,
                    Some(slot.session_id.clone()),
                    Some(serde_json::json!({ "queued": true })),
                )))?;
                return Ok(());
            }
            start_turn(state, slot, id, text).await?;
        }
        ServeCommand::NewSession { id, params } => {
            match create_session_slot(Arc::clone(&state), params, true).await {
                Ok(slot) => {
                    let session_id = slot.session_id.clone();
                    match state.registry.insert(Arc::clone(&slot)) {
                        Ok(()) => {}
                        Err(error) if is_config_error(&error, "too_many_sessions") => {
                            send_error(&state, id, None, "too_many_sessions")?;
                            return Ok(());
                        }
                        Err(error) => return Err(error),
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
    text: String,
) -> Result<(), AppError> {
    if !slot.mark_busy() {
        send_error(&state, id, Some(slot.session_id.clone()), "busy")?;
        return Ok(());
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
        let result = std::panic::AssertUnwindSafe(run_slot_turn(Arc::clone(&slot_for_task), text))
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

fn _assert_transcript_entry_is_serializable(_: &[TranscriptEntry]) {}
fn _assert_ask_question_result(_: &AskQuestionResult) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    use serial_test::serial;

    use crate::api::serve::test_support::{
        build_initialized_state_with_panicking_provider, build_initialized_state_with_streams,
        build_initialized_state_with_streams_and_max_sessions, install_test_api_key,
        read_ndjson_lines,
    };
    use crate::api::serve::types::NewSessionParams;
    use crate::core::llm::StreamEvent;

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
    async fn serve_command_routes_by_session_id() {
        let _api_key = install_test_api_key();
        let (state, _buffer, _temp, _slot) = build_initialized_state_with_streams(vec![]).await;
        let first = state.registry.active_session_id().unwrap();

        handle_command(
            Arc::clone(&state),
            ServeCommand::NewSession {
                id: Some("n1".to_string()),
                params: NewSessionParams::default(),
            },
        )
        .await
        .unwrap();
        let sessions = state.registry.list();
        assert_eq!(sessions.len(), 2);
        let second = sessions
            .iter()
            .find(|session| session.session_id != first)
            .unwrap()
            .session_id
            .clone();

        handle_command(
            Arc::clone(&state),
            ServeCommand::SwitchSession {
                id: Some("sw1".to_string()),
                session_id: second.clone(),
            },
        )
        .await
        .unwrap();

        assert_eq!(
            state.registry.active_session_id().as_deref(),
            Some(second.as_str())
        );
        assert!(state.registry.get(&first).is_some());
        assert!(state.registry.get(&second).is_some());
    }

    #[tokio::test]
    #[serial(env_lock)]
    async fn serve_same_session_second_prompt_is_busy() {
        let _api_key = install_test_api_key();
        let (state, buffer, _temp, _slot) = build_initialized_state_with_streams(vec![]).await;
        let session_id = state.registry.active_session_id().unwrap();
        let slot = state.registry.get(&session_id).unwrap();
        slot.busy.store(true, Ordering::SeqCst);

        handle_command(
            Arc::clone(&state),
            ServeCommand::Prompt {
                id: Some("p2".to_string()),
                session_id: Some(session_id.clone()),
                text: "second".to_string(),
                params: serde_json::Map::new(),
            },
        )
        .await
        .unwrap();

        let lines = wait_for_line(&buffer, |line| {
            line.get("id").and_then(serde_json::Value::as_str) == Some("p2")
        })
        .await;
        let response = lines
            .iter()
            .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("p2"))
            .unwrap();
        assert_eq!(
            response.get("success").and_then(serde_json::Value::as_bool),
            Some(false)
        );
        assert_eq!(
            response.get("error").and_then(serde_json::Value::as_str),
            Some("busy")
        );
    }

    #[tokio::test]
    #[serial(env_lock)]
    async fn serve_prompt_unknown_session_returns_error() {
        let _api_key = install_test_api_key();
        let (state, buffer, _temp, _slot) = build_initialized_state_with_streams(vec![]).await;

        handle_command(
            Arc::clone(&state),
            ServeCommand::Prompt {
                id: Some("unknown-1".to_string()),
                session_id: Some("missing-session".to_string()),
                text: "hello".to_string(),
                params: serde_json::Map::new(),
            },
        )
        .await
        .unwrap();

        let lines = wait_for_line(&buffer, |line| {
            line.get("id").and_then(serde_json::Value::as_str) == Some("unknown-1")
        })
        .await;
        let response = lines
            .iter()
            .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("unknown-1"))
            .unwrap();
        assert_eq!(
            response.get("error").and_then(serde_json::Value::as_str),
            Some("unknown_session")
        );
    }

    #[tokio::test]
    #[serial(env_lock)]
    async fn serve_new_session_rejects_when_registry_is_full() {
        let _api_key = install_test_api_key();
        let (state, buffer, _temp, _slot) =
            build_initialized_state_with_streams_and_max_sessions(1, vec![]).await;

        handle_command(
            Arc::clone(&state),
            ServeCommand::NewSession {
                id: Some("full-1".to_string()),
                params: NewSessionParams::default(),
            },
        )
        .await
        .unwrap();

        let lines = wait_for_line(&buffer, |line| {
            line.get("id").and_then(serde_json::Value::as_str) == Some("full-1")
        })
        .await;
        let response = lines
            .iter()
            .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("full-1"))
            .unwrap();
        assert_eq!(
            response.get("error").and_then(serde_json::Value::as_str),
            Some("too_many_sessions")
        );
    }

    #[tokio::test]
    #[serial(env_lock)]
    async fn serve_prompt_drives_agent_run() {
        let _api_key = install_test_api_key();
        let stream = vec![
            Ok(StreamEvent::ContentDelta {
                delta: "hello".to_string(),
            }),
            Ok(StreamEvent::FinishReason {
                reason: "stop".to_string(),
            }),
            Ok(StreamEvent::Usage {
                prompt_tokens: 1,
                completion_tokens: 1,
                total_tokens: Some(2),
            }),
        ];
        let (state, buffer, _temp, _slot) =
            build_initialized_state_with_streams(vec![stream]).await;
        let session_id = state.registry.active_session_id().unwrap();

        handle_command(
            Arc::clone(&state),
            ServeCommand::Prompt {
                id: Some("p1".to_string()),
                session_id: Some(session_id.clone()),
                text: "say hello".to_string(),
                params: serde_json::Map::new(),
            },
        )
        .await
        .unwrap();

        let lines = wait_for_line(&buffer, |line| {
            line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
        })
        .await;
        assert!(
            lines.iter().any(|line| {
                line.get("id").and_then(serde_json::Value::as_str) == Some("p1")
                    && line.get("success").and_then(serde_json::Value::as_bool) == Some(true)
            }),
            "expected prompt acceptance response, got {lines:?}"
        );
        assert!(
            lines.iter().any(|line| {
                line.get("type").and_then(serde_json::Value::as_str) == Some("agent_start")
                    && line.get("sessionId").and_then(serde_json::Value::as_str)
                        == Some(session_id.as_str())
            }),
            "expected agent_start, got {lines:?}"
        );
        assert!(
            lines.iter().any(|line| {
                line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
                    && line.get("sessionId").and_then(serde_json::Value::as_str)
                        == Some(session_id.as_str())
            }),
            "expected agent_end, got {lines:?}"
        );
    }

    #[tokio::test]
    #[serial(env_lock)]
    async fn serve_prompt_panic_isolation_emits_agent_end_error() {
        let _api_key = install_test_api_key();
        let (state, buffer, _temp, slot) = build_initialized_state_with_panicking_provider().await;

        handle_command(
            Arc::clone(&state),
            ServeCommand::Prompt {
                id: Some("panic-1".to_string()),
                session_id: Some(slot.session_id.clone()),
                text: "panic".to_string(),
                params: serde_json::Map::new(),
            },
        )
        .await
        .unwrap();

        let lines = wait_for_line(&buffer, |line| {
            line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
                && line.get("error").and_then(serde_json::Value::as_str)
                    == Some("serve session task panicked")
        })
        .await;
        assert!(
            lines.iter().any(|line| {
                line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
                    && line.get("error").and_then(serde_json::Value::as_str)
                        == Some("serve session task panicked")
                    && line.get("sessionId").and_then(serde_json::Value::as_str)
                        == Some(slot.session_id.as_str())
            }),
            "expected panic-isolated agent_end, got {lines:?}"
        );
    }
}
