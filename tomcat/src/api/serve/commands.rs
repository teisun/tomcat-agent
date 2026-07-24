//! `serve` 命令分发层。
//!
//! 负责把 `ServeCommand` 翻译为：
//! - 会话路由
//! - turn 启动 / 排队
//! - 响应帧与错误帧
//! - `ChatMessage` 多模态装配

use std::sync::Arc;

use base64::Engine as _;
use futures_util::FutureExt;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::api::chat::commands::{checkpoint_kind_label, restore_core, RestoreCoreReport};
use crate::core::llm::{
    list_model_views, list_provider_keys, remove_user_model, set_provider_key, upsert_user_model,
    ChatMessage, ChatMessageContentPart, ContextRefKind, ContextReference, ProviderKeyInput,
    ThinkingLevel,
};
use crate::core::plan_runtime::PlanRuntimeError;
use crate::core::session::transcript::{
    entry_id, find_entry_line_offset, read_entry_at_offset, TranscriptPage,
};
use crate::infra::events::{AgentEvent, WireEvent};
use crate::AppError;
use crate::{CheckpointId, ListOptions, SessionManager, SessionMode};

use super::control;
use super::types::{
    ListModelsPayload, ListProviderKeysPayload, ListSessionsScope, OutFrame, RemoveModelResponse,
    ResponseFrame, ServeAttachment, ServeAttachmentKind, ServeCommand, ServeContentSegment,
    ServeContextRefKind, ServeContextReference, ServeMessageParams, ServeSessionMode,
    SetPlanModeAction, SetProviderKeyResponse, UpsertModelResponse,
};
use super::{
    cleanup_session_slot, create_session_slot, register_slot_hooks, run_slot_turn, ServeState,
};

enum TurnAck {
    Accepted,
    Payload(serde_json::Value),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GetMessagesCursor {
    offset: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    boundary_id: Option<String>,
}

fn decode_get_messages_cursor(cursor: &str) -> Result<GetMessagesCursor, AppError> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(cursor.as_bytes())
        .map_err(|error| AppError::Config(format!("invalid get_messages cursor: {error}")))?;
    serde_json::from_slice::<GetMessagesCursor>(&bytes)
        .map_err(|error| AppError::Config(format!("invalid get_messages cursor payload: {error}")))
}

fn encode_get_messages_cursor(offset: u64, boundary_id: Option<&str>) -> Result<String, AppError> {
    let cursor = GetMessagesCursor {
        offset,
        boundary_id: boundary_id.map(ToString::to_string),
    };
    let bytes = serde_json::to_vec(&cursor).map_err(|error| {
        AppError::Config(format!("serialize get_messages cursor failed: {error}"))
    })?;
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

fn resolve_cursor_offset(
    session: &SessionManager,
    session_id: &str,
    cursor: &GetMessagesCursor,
) -> Result<u64, AppError> {
    let Some(boundary_id) = cursor.boundary_id.as_deref() else {
        return Ok(cursor.offset);
    };
    let transcript_path = session.transcript_path(session_id);
    if let Some(entry) = read_entry_at_offset(&transcript_path, cursor.offset)? {
        if entry_id(&entry) == Some(boundary_id) {
            return Ok(cursor.offset);
        }
    }
    if let Some(relocated_offset) = find_entry_line_offset(&transcript_path, boundary_id)? {
        return Ok(relocated_offset);
    }
    Ok(cursor.offset)
}

fn encode_next_cursor(page: &TranscriptPage) -> Result<Option<String>, AppError> {
    if !page.has_more {
        return Ok(None);
    }
    let Some(offset) = page.next_cursor_offset else {
        return Ok(None);
    };
    encode_get_messages_cursor(offset, page.entries.first().and_then(entry_id)).map(Some)
}

fn parse_serve_thinking_level(level: &str) -> Option<ThinkingLevel> {
    ThinkingLevel::parse(level)
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
            let input_message = persist_turn_input_message(&slot, input_message, &params)?;
            start_turn(state, slot, id, input_message, TurnAck::Accepted).await?;
        }
        ServeCommand::Steer {
            id,
            session_id,
            text,
            params,
        } => {
            let Some(slot) = resolve_slot_or_error(&state, id.clone(), session_id.clone()).await?
            else {
                return Ok(());
            };
            let input_message =
                persist_turn_input_message(&slot, ChatMessage::steering(text), &params)?;
            if slot.is_busy() {
                slot.ctx
                    .session_runtime
                    .steering_queue
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
            let input_message = persist_turn_input_message(&slot, input_message, &params)?;
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
                .read_session_header_for_session(&slot.session_id)
                .map_err(|error| {
                    AppError::Config(format!("read session header failed: {error}"))
                })?;
            let cursor = params
                .cursor
                .as_deref()
                .map(decode_get_messages_cursor)
                .transpose()?;
            let before = cursor
                .as_ref()
                .map(|cursor| {
                    resolve_cursor_offset(
                        &slot.ctx.session_runtime.session,
                        &slot.session_id,
                        cursor,
                    )
                })
                .transpose()?;
            let page = slot
                .ctx
                .session_runtime
                .session
                .get_entries_before_for_session(&slot.session_id, cap, before)
                .map_err(|error| {
                    AppError::Config(format!("read session entries failed: {error}"))
                })?;
            let next_cursor = encode_next_cursor(&page).map_err(|error| {
                AppError::Config(format!("encode get_messages cursor failed: {error}"))
            })?;
            state.writer.send(OutFrame::Response(ResponseFrame::ok(
                id,
                Some(slot.session_id.clone()),
                Some(serde_json::json!({
                    "sessionId": slot.session_id,
                    "header": header,
                    "messages": page.entries,
                    "nextCursor": next_cursor,
                    "hasMore": page.has_more,
                    // TODO(next): wire up real seq/upToSeq when Phase-2 visibility resync lands.
                    "upToSeq": serde_json::Value::Null
                })),
            )))?;
        }
        ServeCommand::ListCheckpoints { id, session_id } => {
            let Some(slot) = resolve_slot_or_error(&state, id.clone(), session_id.clone()).await?
            else {
                return Ok(());
            };
            let checkpoints = match slot
                .ctx
                .scope_services
                .checkpoint_store
                .list(&slot.session_id, ListOptions::default())
            {
                Ok(checkpoints) => checkpoints,
                Err(error) => {
                    send_error(
                        &state,
                        id,
                        Some(slot.session_id.clone()),
                        format!("list_checkpoints failed: {error}"),
                    )?;
                    return Ok(());
                }
            };
            state.writer.send(OutFrame::Response(ResponseFrame::ok(
                id,
                Some(slot.session_id.clone()),
                Some(json!({
                    "sessionId": slot.session_id,
                    "checkpoints": checkpoints
                        .into_iter()
                        .map(checkpoint_meta_payload)
                        .collect::<Vec<_>>(),
                })),
            )))?;
        }
        ServeCommand::RestoreCheckpoint {
            id,
            session_id,
            checkpoint_id,
            revert_files,
            dry_run,
        } => {
            let Some(slot) = resolve_slot_or_error(&state, id.clone(), session_id.clone()).await?
            else {
                return Ok(());
            };
            if slot.is_busy() {
                send_error(&state, id, Some(slot.session_id.clone()), "busy")?;
                return Ok(());
            }
            let report = match restore_core(
                &slot.ctx,
                CheckpointId::new(checkpoint_id),
                revert_files,
                dry_run.unwrap_or(false),
            ) {
                Ok(report) => report,
                Err(message) => {
                    send_error(&state, id, Some(slot.session_id.clone()), message)?;
                    return Ok(());
                }
            };
            state.writer.send(OutFrame::Response(ResponseFrame::ok(
                id,
                Some(slot.session_id.clone()),
                Some(restore_core_payload(report)),
            )))?;
        }
        ServeCommand::ListSessions { id, scope } => {
            match scope.unwrap_or(ListSessionsScope::Live) {
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
                                    "interrupted": session.interrupted,
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
                        let interrupted = state
                            .registry
                            .get(&session_id)
                            .map(|live_slot| live_slot.is_interrupted())
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
                            "interrupted": interrupted,
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
            }
        }
        ServeCommand::GetState { id, session_id } => {
            let Some(slot) = resolve_slot_or_error(&state, id.clone(), session_id.clone()).await?
            else {
                return Ok(());
            };
            let entry = slot.ctx.session_runtime.session.current_session_entry()?;
            let model = slot.ctx.effective_model(entry.as_ref());
            let thinking_level = slot.ctx.resolve_thinking_level(&model);
            let plan_state = slot.ctx.session_runtime.plan_runtime.mode();
            let active_plan_id = plan_state
                .active_plan_id()
                .map(ToOwned::to_owned)
                .or_else(|| {
                    slot.ctx
                        .session_runtime
                        .plan_runtime
                        .active_planning_plan_id()
                });
            let active_plan_path_raw = if plan_state.is_plan_attached() {
                slot.ctx.session_runtime.plan_runtime.active_plan_path()
            } else {
                None
            };
            let active_plan_path = active_plan_path_raw
                .as_ref()
                .map(|path| crate::infra::platform::format_home_path(path));
            let context_utilization_ratio = entry
                .as_ref()
                .and_then(|session| session.context_utilization_ratio);
            let session_todos = crate::core::tools::plan_tool::shared_todo_ops::items_json(
                &slot
                    .ctx
                    .session_runtime
                    .plan_runtime
                    .snapshot_session_todos(),
            );
            let plan_todos = active_plan_path_raw
                .as_ref()
                .and_then(|path| crate::core::plan_runtime::file_store::read_plan(path).ok())
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
                    "interrupted": slot.is_interrupted(),
                    "mode": match slot.mode { crate::SessionMode::Code => "code", crate::SessionMode::Claw => "claw" },
                    "cwd": slot.cwd,
                    "model": model,
                    "thinkingLevel": thinking_level.as_str(),
                    "planState": plan_state.as_str(),
                    "planId": active_plan_id,
                    "planPath": active_plan_path,
                    "planTodos": plan_todos,
                    "sessionTodos": session_todos,
                    "contextUtilizationRatio": context_utilization_ratio,
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
                SetPlanModeAction::Enter => {
                    match slot.ctx.session_runtime.plan_runtime.enter_planning() {
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
                    }
                }
                SetPlanModeAction::Exit => {
                    let exit_result = if matches!(
                        slot.ctx.session_runtime.plan_runtime.mode(),
                        crate::core::plan_runtime::PlanState::Executing { .. }
                    ) {
                        match slot
                            .ctx
                            .session_runtime
                            .plan_runtime
                            .demote_to_pending_on_cancel()
                        {
                            Ok(Some(_)) | Ok(None) => {
                                slot.ctx.session_runtime.plan_runtime.exit_to_chat()
                            }
                            Err(error) => Err(error),
                        }
                    } else {
                        slot.ctx.session_runtime.plan_runtime.exit_to_chat()
                    };
                    match exit_result {
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
                            send_error(&state, id, Some(slot.session_id.clone()), error_code)?;
                        }
                    }
                }
                SetPlanModeAction::Build => {
                    let build_target = match plan_id {
                        Some(target) => target,
                        None => {
                            match slot.ctx.session_runtime.plan_runtime.default_build_target() {
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
                            }
                        }
                    };
                    match slot
                        .ctx
                        .session_runtime
                        .plan_runtime
                        .build_plan(&build_target, Some(slot.session_id.clone()))
                    {
                        Ok(outcome) => {
                            let response_payload = plan_state_payload(
                                &slot,
                                Some(outcome.plan_path.to_string_lossy().to_string()),
                            );
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
            if let Err(error) = slot
                .ctx
                .global_services
                .model_catalog
                .lookup_explicit(&model)
            {
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
            if let Err(error) = slot
                .ctx
                .global_services
                .model_catalog
                .lookup_explicit(&model)
            {
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
            let catalog = resolve_model_catalog_snapshot(&state)?;
            let models = list_model_views(catalog.as_ref());
            state.writer.send(OutFrame::Response(ResponseFrame::ok(
                id,
                state.registry.active_session_id(),
                Some(
                    serde_json::to_value(ListModelsPayload { models }).map_err(|error| {
                        AppError::Config(format!("serialize list_models payload failed: {error}"))
                    })?,
                ),
            )))?;
        }
        ServeCommand::UpsertModel { id, model } => {
            let result = match upsert_user_model(&state.cfg, model) {
                Ok(model) => model,
                Err(error) => {
                    send_error(
                        &state,
                        id,
                        state.registry.active_session_id(),
                        render_error_message(&error),
                    )?;
                    return Ok(());
                }
            };
            refresh_all_model_catalogs(&state)?;
            state.writer.send(OutFrame::Response(ResponseFrame::ok(
                id,
                state.registry.active_session_id(),
                Some(
                    serde_json::to_value(UpsertModelResponse {
                        model: result.model,
                        warnings: result.warnings,
                    })
                    .map_err(|error| {
                        AppError::Config(format!("serialize upsert_model payload failed: {error}"))
                    })?,
                ),
            )))?;
        }
        ServeCommand::RemoveModel { id, model_id } => {
            if let Err(error) = remove_user_model(&state.cfg, &model_id) {
                send_error(
                    &state,
                    id,
                    state.registry.active_session_id(),
                    render_error_message(&error),
                )?;
                return Ok(());
            }
            refresh_all_model_catalogs(&state)?;
            state.writer.send(OutFrame::Response(ResponseFrame::ok(
                id,
                state.registry.active_session_id(),
                Some(
                    serde_json::to_value(RemoveModelResponse { model_id }).map_err(|error| {
                        AppError::Config(format!("serialize remove_model payload failed: {error}"))
                    })?,
                ),
            )))?;
        }
        ServeCommand::SetProviderKey {
            id,
            env_name,
            value,
        } => {
            let status = match set_provider_key(&state.cfg, ProviderKeyInput { env_name, value }) {
                Ok(status) => status,
                Err(error) => {
                    send_error(
                        &state,
                        id,
                        state.registry.active_session_id(),
                        render_error_message(&error),
                    )?;
                    return Ok(());
                }
            };
            refresh_all_model_catalogs(&state)?;
            state.writer.send(OutFrame::Response(ResponseFrame::ok(
                id,
                state.registry.active_session_id(),
                Some(
                    serde_json::to_value(SetProviderKeyResponse::from(status)).map_err(
                        |error| {
                            AppError::Config(format!(
                                "serialize set_provider_key payload failed: {error}"
                            ))
                        },
                    )?,
                ),
            )))?;
        }
        ServeCommand::ListProviderKeys { id } => {
            let keys = match list_provider_keys(&state.cfg) {
                Ok(keys) => keys,
                Err(error) => {
                    send_error(
                        &state,
                        id,
                        state.registry.active_session_id(),
                        render_error_message(&error),
                    )?;
                    return Ok(());
                }
            };
            state.writer.send(OutFrame::Response(ResponseFrame::ok(
                id,
                state.registry.active_session_id(),
                Some(
                    serde_json::to_value(ListProviderKeysPayload { keys }).map_err(|error| {
                        AppError::Config(format!(
                            "serialize list_provider_keys payload failed: {error}"
                        ))
                    })?,
                ),
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

fn resolve_active_slot(state: &ServeState) -> Result<Arc<super::registry::SessionSlot>, AppError> {
    if let Some(active_session_id) = state.registry.active_session_id() {
        if let Some(slot) = state.registry.get(&active_session_id) {
            return Ok(slot);
        }
    }
    state
        .registry
        .list()
        .into_iter()
        .find_map(|summary| state.registry.get(&summary.session_id))
        .ok_or_else(|| AppError::Config("unknown_session".to_string()))
}

fn resolve_model_catalog_snapshot(
    state: &ServeState,
) -> Result<Arc<crate::core::llm::ModelCatalog>, AppError> {
    if let Some(active_session_id) = state.registry.active_session_id() {
        if let Some(slot) = state.registry.get(&active_session_id) {
            return Ok(slot.ctx.global_services.model_catalog.snapshot());
        }
    }
    if let Some(slot) = state
        .registry
        .list()
        .into_iter()
        .find_map(|summary| state.registry.get(&summary.session_id))
    {
        return Ok(slot.ctx.global_services.model_catalog.snapshot());
    }
    Ok(state.shared_model_catalog.snapshot())
}

fn refresh_all_model_catalogs(state: &ServeState) -> Result<(), AppError> {
    state.shared_model_catalog.reload(&state.cfg)?;
    for summary in state.registry.list() {
        if let Some(slot) = state.registry.get(&summary.session_id) {
            slot.ctx.global_services.model_catalog.reload(&state.cfg)?;
        }
    }
    Ok(())
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

fn checkpoint_label(kind: &crate::core::CheckpointKind) -> Option<&str> {
    match kind {
        crate::core::CheckpointKind::Manual { label } => Some(label.as_str()),
        _ => None,
    }
}

fn checkpoint_changed_files(meta: &crate::core::CheckpointMeta) -> Vec<String> {
    meta.notes
        .as_ref()
        .and_then(|notes| notes.get("changedPaths"))
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .filter(|path| !path.trim().is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn checkpoint_meta_payload(meta: crate::core::CheckpointMeta) -> serde_json::Value {
    let kind = checkpoint_kind_label(&meta.kind).to_string();
    let label = checkpoint_label(&meta.kind).map(ToOwned::to_owned);
    let changed_files = checkpoint_changed_files(&meta);
    let crate::core::CheckpointMeta {
        id,
        session_id,
        turn_id,
        git_commit,
        message_anchor,
        created_at,
        ..
    } = meta;
    json!({
        "id": id.to_string(),
        "sessionId": session_id,
        "turnId": turn_id,
        "kind": kind,
        "label": label,
        "gitCommit": git_commit,
        "messageAnchor": message_anchor,
        "createdAt": created_at,
        "changedFiles": changed_files,
    })
}

fn restore_core_payload(report: RestoreCoreReport) -> serde_json::Value {
    let kind = checkpoint_kind_label(&report.meta.kind).to_string();
    let label = checkpoint_label(&report.meta.kind).map(ToOwned::to_owned);
    let RestoreCoreReport {
        changed_paths,
        dry_run,
        meta,
        restored_paths,
        revert_files,
        reloaded_plan_id,
        summary,
        transcript_truncated,
        warnings,
    } = report;
    let crate::core::CheckpointMeta {
        id,
        session_id,
        turn_id,
        message_anchor,
        created_at,
        ..
    } = meta;
    json!({
        "checkpointId": id.to_string(),
        "sessionId": session_id,
        "turnId": turn_id,
        "kind": kind,
        "label": label,
        "messageAnchor": message_anchor,
        "createdAt": created_at,
        "changedPaths": changed_paths,
        "restoredPaths": restored_paths,
        "dryRun": dry_run,
        "revertFiles": revert_files,
        "transcriptTruncated": transcript_truncated,
        "reloadedPlanId": reloaded_plan_id,
        "summary": summary,
        "warnings": warnings,
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
    if let Err(error) = slot
        .ctx
        .agent_registry
        .rearm_root(&slot.session_id, turn_token.child_token())
    {
        slot.mark_idle();
        return Err(AppError::Config(format!(
            "agent_registry root rearm 失败: {error}"
        )));
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
        emit_agent_idle(&state_for_task, &slot_for_task);
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

fn emit_agent_idle(state: &ServeState, slot: &super::registry::SessionSlot) {
    let frame = OutFrame::Event(
        serde_json::to_value(WireEvent {
            session_id: Some(slot.session_id.clone()),
            event: AgentEvent::AgentIdle,
        })
        .expect("agent_idle wire event should serialize"),
    );
    let _ = state.writer.send(frame);
}

fn render_error_message(error: &AppError) -> String {
    crate::api::chat::render_error_message(error)
}

fn to_context_ref_kind(kind: ServeContextRefKind) -> ContextRefKind {
    match kind {
        ServeContextRefKind::Selection => ContextRefKind::Selection,
        ServeContextRefKind::File => ContextRefKind::File,
    }
}

fn to_context_reference(reference: &ServeContextReference) -> ContextReference {
    ContextReference {
        ref_kind: to_context_ref_kind(reference.kind),
        path: reference.path.clone(),
        label: reference.label.clone(),
        line_start: reference.line_start,
        line_end: reference.line_end,
        text: reference.text.clone(),
    }
}

pub(crate) fn build_user_message(
    text: String,
    params: &ServeMessageParams,
) -> Result<ChatMessage, String> {
    if params.segments.is_empty() && params.attachments.is_empty() {
        return Ok(ChatMessage::user(text));
    }

    let mut parts = Vec::with_capacity(params.segments.len().max(1) + params.attachments.len());
    if params.segments.is_empty() {
        parts.push(ChatMessageContentPart::text(text));
    } else {
        for segment in &params.segments {
            match segment {
                ServeContentSegment::Text { text } => {
                    parts.push(ChatMessageContentPart::text(text.clone()));
                }
                ServeContentSegment::Reference { reference } => {
                    parts.push(ChatMessageContentPart::reference(to_context_reference(
                        reference,
                    )));
                }
            }
        }
    }
    for attachment in &params.attachments {
        parts.push(parse_attachment_part(attachment)?);
    }
    Ok(ChatMessage::user_with_parts(parts))
}

fn normalized_user_message_id(params: &ServeMessageParams) -> Option<&str> {
    params
        .user_message_id
        .as_deref()
        .map(str::trim)
        .filter(|candidate| !candidate.is_empty())
}

fn persist_turn_input_message(
    slot: &Arc<super::registry::SessionSlot>,
    mut message: ChatMessage,
    params: &ServeMessageParams,
) -> Result<ChatMessage, AppError> {
    let row_id = if let Some(forced_id) = normalized_user_message_id(params) {
        if slot
            .ctx
            .session_runtime
            .session
            .get_entry_for_session(&slot.session_id, forced_id)?
            .is_none()
        {
            slot.ctx
                .session_runtime
                .session
                .append_message_with_id(serde_json::to_value(&message)?, forced_id)?
        } else {
            slot.ctx
                .session_runtime
                .session
                .append_message(serde_json::to_value(&message)?)?
        }
    } else {
        slot.ctx
            .session_runtime
            .session
            .append_message(serde_json::to_value(&message)?)?
    };
    message.msg_id = Some(row_id);
    Ok(message)
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
                let mime_type = attachment.mime_type.clone().ok_or_else(|| {
                    "invalid_attachment: image attachment requires mimeType".to_string()
                })?;
                let data = attachment.data_base64.clone().ok_or_else(|| {
                    "invalid_attachment: image attachment requires dataBase64".to_string()
                })?;
                ChatMessageContentPart::image_base64_data(mime_type, data)
                    .map_err(|error| format!("invalid_attachment: {error}"))
            }
        }
        ServeAttachmentKind::File => {
            if let Some(file_id) = attachment.file_id.clone() {
                ChatMessageContentPart::file_file_id(file_id, attachment.filename.clone())
                    .map_err(|error| format!("invalid_attachment: {error}"))
            } else {
                let mime_type = attachment.mime_type.clone().ok_or_else(|| {
                    "invalid_attachment: file attachment requires mimeType".to_string()
                })?;
                if !mime_type.eq_ignore_ascii_case("application/pdf") {
                    return Err(format!(
                        "invalid_attachment: file attachments only support application/pdf; use kind=image for images (got {mime_type})"
                    ));
                }
                let filename = attachment.filename.clone().ok_or_else(|| {
                    "invalid_attachment: file attachment requires filename".to_string()
                })?;
                let data = attachment.data_base64.clone().ok_or_else(|| {
                    "invalid_attachment: file attachment requires dataBase64".to_string()
                })?;
                ChatMessageContentPart::file_base64_data(filename, mime_type, data)
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
