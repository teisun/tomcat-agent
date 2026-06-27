//! `serve` 命令分发层。
//!
//! 负责把 `ServeCommand` 翻译为：
//! - 会话路由
//! - turn 启动 / 排队
//! - 响应帧与错误帧
//! - `ChatMessage` 多模态装配

use std::sync::Arc;

use futures_util::FutureExt;

use crate::core::llm::{ChatMessage, ChatMessageContentPart, ThinkingLevel};
use crate::core::plan_runtime::PlanRuntimeError;
use crate::AppError;
use crate::{SessionManager, SessionMode};

use super::control;
use super::types::{
    ListSessionsScope, OutFrame, ResponseFrame, ServeAttachment, ServeAttachmentKind, ServeCommand,
    ServeMessageParams, ServeSessionMode, SetPlanModeAction,
};
use super::{cleanup_session_slot, create_session_slot, register_slot_hooks, run_slot_turn, ServeState};

enum TurnAck {
    Accepted,
    Payload(serde_json::Value),
}

fn parse_serve_thinking_level(level: &str) -> Option<ThinkingLevel> {
    match level.trim().to_ascii_lowercase().as_str() {
        "low" => Some(ThinkingLevel::Low),
        "medium" => Some(ThinkingLevel::Medium),
        "high" => Some(ThinkingLevel::High),
        "xhigh" => Some(ThinkingLevel::Xhigh),
        _ => None,
    }
}

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
            start_turn(state, slot, id, input_message, TurnAck::Accepted).await?;
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
            start_turn(state, slot, id, ChatMessage::steering(text), TurnAck::Accepted).await?;
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
            start_turn(state, slot, id, input_message, TurnAck::Accepted).await?;
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
            if state.registry.get(&session_id).is_none() {
                match open_existing_session_slot(Arc::clone(&state), &session_id).await {
                    Ok(slot) => {
                        let inserted_session_id = slot.session_id.clone();
                        match state.registry.insert(Arc::clone(&slot)) {
                            Ok(()) => register_slot_hooks(&state, &slot),
                            Err(error) if is_config_error(&error, "too_many_sessions") => {
                                send_error(&state, id, Some(session_id), "too_many_sessions")?;
                                return Ok(());
                            }
                            Err(error) => return Err(error),
                        }
                        state.registry.set_active_session(&inserted_session_id)?;
                    }
                    Err(error) if is_config_error(&error, "unknown_session") => {
                        send_error(&state, id, Some(session_id), "unknown_session")?;
                        return Ok(());
                    }
                    Err(error) if is_config_error(&error, "too_many_sessions") => {
                        send_error(&state, id, Some(session_id), "too_many_sessions")?;
                        return Ok(());
                    }
                    Err(error) => return Err(error),
                }
            } else if let Err(error) = state.registry.set_active_session(&session_id) {
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
        ServeCommand::ListSessions { id, scope } => match scope.unwrap_or(ListSessionsScope::Live) {
            ListSessionsScope::Live => {
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
            ListSessionsScope::Disk => {
                let slot = resolve_active_slot(&state)?;
                let sessions_dir = crate::resolve_sessions_dir(&state.cfg)?;
                let session_manager = SessionManager::new_scoped(
                    sessions_dir,
                    slot.ctx
                        .session_runtime
                        .session
                        .current_session_key()
                        .to_string(),
                );
                let current_session_id = session_manager.current_session_id()?;
                let sessions = session_manager
                    .list_sessions()?
                    .into_iter()
                    .map(|(session_id, entry)| {
                        let busy = state
                            .registry
                            .get(&session_id)
                            .map(|live_slot| live_slot.is_busy())
                            .unwrap_or(false);
                        let title = entry.title.clone().or_else(|| {
                            // 惰性回填：无持久化 title 时从 transcript 首条 user message 派生，不落盘。
                            let path = session_manager.transcript_path(&session_id);
                            crate::core::session::transcript::read_first_user_message_text(&path, 200)
                                .map(|text| crate::core::session::manager::derive_title_from_user_message(&text))
                        }).or_else(|| Some("New session".to_string()));
                        serde_json::json!({
                            "sessionId": session_id,
                            "updatedAt": entry.updated_at,
                            "isCurrent": current_session_id.as_deref() == Some(entry.session_id.as_str()),
                            "busy": busy,
                            "title": title,
                        })
                    })
                    .collect::<Vec<_>>();
                state.writer.send(OutFrame::Response(ResponseFrame::ok(
                    id,
                    current_session_id.clone(),
                    Some(serde_json::json!({
                        "activeSessionId": current_session_id,
                        "sessions": sessions,
                    })),
                )))?;
            }
        },
        ServeCommand::GetState { id, session_id } => {
            let Some(slot) = resolve_slot_or_error(&state, id.clone(), session_id.clone()).await?
            else {
                return Ok(());
            };
            let entry = slot.ctx.session_runtime.session.current_session_entry()?;
            let model = slot.ctx.effective_model(entry.as_ref());
            let thinking_level = slot.ctx.global_services.model_thinking.get(&model);
            let plan_state = slot.ctx.session_runtime.plan_runtime.mode();
            let active_plan_id = plan_state
                .active_plan_id()
                .map(ToOwned::to_owned)
                .or_else(|| slot.ctx.session_runtime.plan_runtime.active_planning_plan_id());
            let active_plan_path = slot
                .ctx
                .session_runtime
                .plan_runtime
                .active_plan_path()
                .map(|path| crate::infra::platform::format_home_path(&path));
            let session_todos = crate::core::tools::plan_tool::shared_todo_ops::items_json(
                &slot
                    .ctx
                    .session_runtime
                    .plan_runtime
                    .snapshot_session_todos(),
            );
            let plan_todos = active_plan_path
                .as_ref()
                .and_then(|_| slot.ctx.session_runtime.plan_runtime.active_plan_path())
                .and_then(|path| crate::core::plan_runtime::file_store::read_plan(&path).ok())
                .map(|plan| {
                    crate::core::tools::plan_tool::shared_todo_ops::items_json(
                        &plan.frontmatter.todos,
                    )
                })
                .unwrap_or_default();
            state.writer.send(OutFrame::Response(ResponseFrame::ok(
                id,
                Some(slot.session_id.clone()),
                Some(serde_json::json!({
                    "sessionId": slot.session_id,
                    "busy": slot.is_busy(),
                    "mode": match slot.mode { crate::SessionMode::Code => "code", crate::SessionMode::Claw => "claw" },
                    "cwd": slot.cwd,
                    "model": model,
                    "thinkingLevel": thinking_level.as_str(),
                    "planState": plan_state.as_str(),
                    "planId": active_plan_id,
                    "planPath": active_plan_path,
                    "planTodos": plan_todos,
                    "sessionTodos": session_todos,
                    "sessionKey": slot.ctx.session_runtime.session.current_session_key(),
                })),
            )))?;
        }
        ServeCommand::SetPlanMode {
            id,
            session_id,
            action,
            plan_id,
        } => {
            let Some(slot) = resolve_slot_or_error(&state, id.clone(), session_id.clone()).await?
            else {
                return Ok(());
            };
            if slot.is_busy() {
                send_error(&state, id, Some(slot.session_id.clone()), "busy")?;
                return Ok(());
            }
            match action {
                SetPlanModeAction::Enter => match slot.ctx.session_runtime.plan_runtime.enter_planning()
                {
                    Ok(()) => {
                        state.writer.send(OutFrame::Response(ResponseFrame::ok(
                            id,
                            Some(slot.session_id.clone()),
                            Some(plan_state_payload(&slot, None)),
                        )))?;
                    }
                    Err(error) => {
                        send_error(
                            &state,
                            id,
                            Some(slot.session_id.clone()),
                            normalize_plan_runtime_error_code(&error),
                        )?;
                    }
                },
                SetPlanModeAction::Exit => match slot.ctx.session_runtime.plan_runtime.exit_to_chat() {
                    Ok(()) => {
                        state.writer.send(OutFrame::Response(ResponseFrame::ok(
                            id,
                            Some(slot.session_id.clone()),
                            Some(plan_state_payload(&slot, None)),
                        )))?;
                    }
                    Err(error) => {
                        let error_code = match error {
                            PlanRuntimeError::AlreadyInMode(_)
                            | PlanRuntimeError::NotInPlanning(_) => "plan_state_conflict",
                            _ => normalize_plan_runtime_error_code(&error),
                        };
                        send_error(
                            &state,
                            id,
                            Some(slot.session_id.clone()),
                            error_code,
                        )?;
                    }
                },
                SetPlanModeAction::Build => {
                    let build_target = match plan_id {
                        Some(target) => target,
                        None => match slot.ctx.session_runtime.plan_runtime.default_build_target() {
                            Ok(target) => target,
                            Err(error) => {
                                send_error(
                                    &state,
                                    id,
                                    Some(slot.session_id.clone()),
                                    normalize_plan_runtime_error_code(&error),
                                )?;
                                return Ok(());
                            }
                        },
                    };
                    match slot
                        .ctx
                        .session_runtime
                        .plan_runtime
                        .build_plan(&build_target, Some(slot.session_id.clone()))
                    {
                        Ok(outcome) => {
                            let response_payload =
                                plan_state_payload(&slot, Some(outcome.plan_path.to_string_lossy().to_string()));
                            start_turn(
                                Arc::clone(&state),
                                slot,
                                id,
                                ChatMessage::user(format!(
                                    "start building {}",
                                    outcome.plan_path.to_string_lossy()
                                )),
                                TurnAck::Payload(response_payload),
                            )
                            .await?;
                        }
                        Err(error) => {
                            send_error(
                                &state,
                                id,
                                Some(slot.session_id.clone()),
                                normalize_plan_runtime_error_code(&error),
                            )?;
                        }
                    }
                }
            }
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
            if let Err(error) = slot.ctx.global_services.model_catalog.lookup_explicit(&model) {
                send_error(
                    &state,
                    id,
                    Some(slot.session_id.clone()),
                    render_error_message(&error),
                )?;
                return Ok(());
            }
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
        ServeCommand::SetThinkingLevel {
            id,
            session_id,
            model,
            level,
        } => {
            let Some(slot) = resolve_slot_or_error(&state, id.clone(), session_id.clone()).await?
            else {
                return Ok(());
            };
            if let Err(error) = slot.ctx.global_services.model_catalog.lookup_explicit(&model) {
                send_error(
                    &state,
                    id,
                    Some(slot.session_id.clone()),
                    render_error_message(&error),
                )?;
                return Ok(());
            }
            let Some(parsed_level) = parse_serve_thinking_level(&level) else {
                send_error(
                    &state,
                    id,
                    Some(slot.session_id.clone()),
                    "invalid_thinking_level",
                )?;
                return Ok(());
            };
            slot.ctx
                .global_services
                .model_thinking
                .set(&model, parsed_level)?;
            state.writer.send(OutFrame::Response(ResponseFrame::ok(
                id,
                Some(slot.session_id.clone()),
                Some(serde_json::json!({
                    "sessionId": slot.session_id,
                    "model": model,
                    "level": parsed_level.as_str(),
                })),
            )))?;
        }
        ServeCommand::ListModels { id } => {
            let slot = resolve_active_slot(&state)?;
            let models = slot
                .ctx
                .global_services
                .model_catalog
                .entries()
                .into_iter()
                .map(|entry| {
                    serde_json::json!({
                        "id": entry.id,
                        "modelName": entry.model_name,
                        "provider": entry.provider,
                        "api": entry.api,
                        "baseUrl": entry.base_url,
                        "capabilities": entry.capabilities,
                    })
                })
                .collect::<Vec<_>>();
            state.writer.send(OutFrame::Response(ResponseFrame::ok(
                id,
                state.registry.active_session_id(),
                Some(serde_json::json!({ "models": models })),
            )))?;
        }
        ServeCommand::CloseSession { id, session_id } => {
            let Some(slot) = resolve_slot_or_error(&state, id.clone(), session_id.clone()).await?
            else {
                return Ok(());
            };
            cleanup_session_slot(&state, &slot, true, "close_session").await?;
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
    let Some(slot) = state.registry.get(&resolved) else {
        send_error(state, id, Some(resolved), "unknown_session")?;
        return Ok(None);
    };
    Ok(Some(slot))
}

fn resolve_active_slot(
    state: &ServeState,
) -> Result<Arc<super::registry::SessionSlot>, AppError> {
    if let Some(active_session_id) = state.registry.active_session_id() {
        if let Some(slot) = state.registry.get(&active_session_id) {
            return Ok(slot);
        }
    }
    state.registry
        .list()
        .into_iter()
        .find_map(|summary| state.registry.get(&summary.session_id))
        .ok_or_else(|| AppError::Config("unknown_session".to_string()))
}

async fn open_existing_session_slot(
    state: Arc<ServeState>,
    session_id: &str,
) -> Result<Arc<super::registry::SessionSlot>, AppError> {
    if state.registry.len() >= state.registry.max_sessions() {
        return Err(AppError::Config("too_many_sessions".to_string()));
    }
    let base_slot = resolve_active_slot(&state)?;
    let sessions_dir = crate::resolve_sessions_dir(&state.cfg)?;
    let session_manager = SessionManager::new_scoped(
        sessions_dir,
        base_slot
            .ctx
            .session_runtime
            .session
            .current_session_key()
            .to_string(),
    );
    let entry = match session_manager.switch_current_to_session_id(session_id) {
        Ok(entry) => entry,
        Err(AppError::Config(_)) => return Err(AppError::Config("unknown_session".to_string())),
        Err(error) => return Err(error),
    };
    session_manager.pin_session(&entry.session_id);
    create_session_slot(
        state,
        super::types::NewSessionParams {
            cwd: entry.cwd.or_else(|| base_slot.cwd.clone()),
            mode: Some(match base_slot.mode {
                SessionMode::Code => ServeSessionMode::Code,
                SessionMode::Claw => ServeSessionMode::Claw,
            }),
        },
        false,
    )
    .await
}

fn plan_state_payload(
    slot: &super::registry::SessionSlot,
    plan_path_override: Option<String>,
) -> serde_json::Value {
    let plan_runtime = &slot.ctx.session_runtime.plan_runtime;
    let plan_state = plan_runtime.mode();
    let plan_id = plan_state
        .active_plan_id()
        .map(ToOwned::to_owned)
        .or_else(|| plan_runtime.active_planning_plan_id());
    let plan_path = plan_path_override.or_else(|| {
        plan_runtime
            .active_plan_path()
            .map(|path| crate::infra::platform::format_home_path(&path))
    });
    serde_json::json!({
        "sessionId": slot.session_id,
        "planState": plan_state.as_str(),
        "planId": plan_id,
        "planPath": plan_path,
        "sessionKey": slot.ctx.session_runtime.session.current_session_key(),
    })
}

fn normalize_plan_runtime_error_code(error: &PlanRuntimeError) -> &'static str {
    match error {
        PlanRuntimeError::AlreadyInMode(_) => "plan_already_in_mode",
        PlanRuntimeError::NotInPlanning(_) => "plan_state_conflict",
        PlanRuntimeError::UnsafePlanId(_) | PlanRuntimeError::Io(_) => "plan_io_error",
        PlanRuntimeError::BuildBlocked(_) => "plan_build_blocked",
        PlanRuntimeError::BuildPlanNotFound { .. }
        | PlanRuntimeError::BuildPlanPathNotFound { .. } => "plan_not_found",
    }
}

async fn start_turn(
    state: Arc<ServeState>,
    slot: Arc<super::registry::SessionSlot>,
    id: Option<String>,
    input_message: ChatMessage,
    ack: TurnAck,
) -> Result<(), AppError> {
    if !slot.mark_busy() {
        if id.is_some() {
            send_error(&state, id, Some(slot.session_id.clone()), "busy")?;
        }
        return Ok(());
    }
    slot.reset_terminal_emitted();

    let turn_token = tokio_util::sync::CancellationToken::new();
    {
        let mut guard = slot.ctx.session_runtime.cancel_token.lock();
        *guard = turn_token.clone();
    }

    state.registry.set_active_session(&slot.session_id)?;
    match ack {
        TurnAck::Accepted => {
            state.writer.send(OutFrame::Response(ResponseFrame::ok(
                id,
                Some(slot.session_id.clone()),
                Some(serde_json::json!({ "accepted": true })),
            )))?;
        }
        TurnAck::Payload(payload) => {
            state.writer.send(OutFrame::Response(ResponseFrame::ok(
                id,
                Some(slot.session_id.clone()),
                Some(payload),
            )))?;
        }
    }

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
            Ok(Ok(crate::AgentRunOutcome::Completed(_)))
            | Ok(Ok(crate::AgentRunOutcome::Interrupted(_))) => {}
            Ok(Ok(crate::AgentRunOutcome::Failed(error))) => {
                emit_agent_end_once(
                    &state_for_task,
                    &slot_for_task,
                    render_error_message(&error),
                );
            }
            Ok(Err(error)) => {
                tracing::error!(session_id = %slot_for_task.session_id, error = %error, "serve session turn failed");
                emit_agent_end_once(
                    &state_for_task,
                    &slot_for_task,
                    render_error_message(&error),
                );
            }
            Err(_) => {
                emit_agent_end_once(
                    &state_for_task,
                    &slot_for_task,
                    "serve session task panicked",
                );
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

fn emit_agent_end_once(
    state: &ServeState,
    slot: &super::registry::SessionSlot,
    error: impl Into<String>,
) {
    if !slot.mark_terminal_emitted_if_absent() {
        return;
    }
    let frame = OutFrame::Event(serde_json::json!({
        "type": "agent_end",
        "sessionId": slot.session_id,
        "messages": [],
        "error": error.into(),
    }));
    let _ = state.writer.send(frame);
}

fn render_error_message(error: &AppError) -> String {
    match error {
        AppError::Config(message) => message.clone(),
        _ => error.to_string(),
    }
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
