//! `serve` 命令分发层。
//!
//! 负责把 `ServeCommand` 翻译为：
//! - 会话路由
//! - turn 启动 / 排队
//! - 响应帧与错误帧
//! - `ChatMessage` 多模态装配

use std::collections::HashSet;
use std::sync::Arc;

use base64::Engine as _;
use chrono::{DateTime, Utc};
use futures_util::FutureExt;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::warn;

use crate::api::chat::commands::{
    checkpoint_kind_label, compact_session, restore_core, RestoreCoreReport,
};
use crate::core::connector::mcp::config::{
    global_mcp_path, project_mcp_path, upsert_global_server, upsert_project_server,
    McpConfigSource, McpOAuthConfig, McpServerConfig, ToolFilter,
};
use crate::core::connector::mcp::manager::ServerState;
use crate::core::connector::ConnectorRegistry;
use crate::core::llm::{
    list_model_views_with_prefs, list_provider_keys, remove_user_model, set_provider_key,
    upsert_user_model, ChatMessage, ChatMessageContent, ChatMessageContentPart, ContextRefKind,
    ContextReference, LlmScene, ProviderKeyInput, ThinkingLevel,
};
use crate::core::plan_runtime::PlanRuntimeError;
use crate::core::session::attachments::{
    safe_filename, validate_file_bytes, validate_image_bytes, AttachmentBlobStore,
    REBUILDABLE_MAX_BYTES,
};
use crate::core::session::manager::init_context_state_with_limits;
use crate::core::session::tool_display_sidecar::read_tool_displays_for_calls;
use crate::core::session::transcript::{
    entry_id, find_entry_line_offset, read_entries_tail_before, read_entry_at_offset,
    TranscriptEntry, TranscriptPage,
};
use crate::infra::events::{AgentEvent, WireEvent};
use crate::AppError;
use crate::{CheckpointId, ListOptions, SessionManager, SessionMode};

use super::control;
use super::types::{
    AttachmentMode, CacheThumbnailInput, IngestAttachmentInput, IngestAttachmentResponse,
    ListModelsPayload, ListProviderKeysPayload, ListSessionsScope, OutFrame, RemoveModelResponse,
    ResponseFrame, ServeAttachment, ServeAttachmentKind, ServeCommand, ServeContentSegment,
    ServeContextRefKind, ServeContextReference, ServeMessageParams, ServeSessionMode,
    SetPlanModeAction, SetProviderKeyResponse, UpsertModelResponse,
};
use super::{
    cleanup_session_slot, create_session_slot, register_slot_hooks, run_slot_turn, ServeState,
};

pub(crate) enum TurnAck {
    Accepted,
    Payload(serde_json::Value),
    Silent,
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

fn message_role(entry: &TranscriptEntry) -> Option<&str> {
    let TranscriptEntry::Message(message) = entry else {
        return None;
    };
    message
        .message
        .get("role")
        .and_then(serde_json::Value::as_str)
}

fn tool_call_id(entry: &TranscriptEntry) -> Option<&str> {
    let TranscriptEntry::Message(message) = entry else {
        return None;
    };
    message
        .message
        .get("tool_call_id")
        .and_then(serde_json::Value::as_str)
}

fn assistant_declares_tool_call(entry: &TranscriptEntry, expected_id: &str) -> bool {
    let TranscriptEntry::Message(message) = entry else {
        return false;
    };
    message_role(entry) == Some("assistant")
        && message
            .message
            .get("tool_calls")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|calls| {
                calls.iter().any(|call| {
                    call.get("id").and_then(serde_json::Value::as_str) == Some(expected_id)
                })
            })
}

/// Keep an assistant tool-call declaration and its leading tool-result companions in one page.
/// A tiny page may otherwise start with a tool result, which leaves clients unable to recover the
/// tool name/arguments until a later pagination request. The durable `tool_call_id` is the join key;
/// the response may exceed the requested limit by the size of this one tool-call group.
fn complete_leading_tool_companions(
    session: &SessionManager,
    session_id: &str,
    mut page: TranscriptPage,
) -> Result<TranscriptPage, AppError> {
    let Some(first) = page.entries.first() else {
        return Ok(page);
    };
    if message_role(first) != Some("tool") {
        return Ok(page);
    }
    let Some(first_id) = entry_id(first) else {
        return Ok(page);
    };
    let Some(expected_tool_call_id) = tool_call_id(first) else {
        return Ok(page);
    };

    let transcript_path = session.transcript_path(session_id);
    let Some(first_offset) = find_entry_line_offset(&transcript_path, first_id)? else {
        return Ok(page);
    };
    // Tool calls from one assistant message are bounded by the runtime catalog, so a bounded reverse
    // read recovers the declaration and any earlier sibling results without loading the transcript.
    let preceding = read_entries_tail_before(&transcript_path, 256, Some(first_offset))?;
    let Some(owner_index) = preceding
        .entries
        .iter()
        .rposition(|entry| assistant_declares_tool_call(entry, expected_tool_call_id))
    else {
        return Ok(page);
    };
    let owner_id = entry_id(&preceding.entries[owner_index]).map(ToOwned::to_owned);
    let existing_ids = page
        .entries
        .iter()
        .filter_map(entry_id)
        .map(ToOwned::to_owned)
        .collect::<std::collections::HashSet<String>>();
    let mut companions = preceding
        .entries
        .into_iter()
        .skip(owner_index)
        .filter(|entry| entry_id(entry).is_none_or(|id| !existing_ids.contains(id)))
        .collect::<Vec<_>>();
    if companions.is_empty() {
        return Ok(page);
    }
    companions.append(&mut page.entries);
    page.entries = companions;

    if let Some(owner_id) = owner_id {
        if let Some(owner_offset) = find_entry_line_offset(&transcript_path, &owner_id)? {
            let preceding = read_entries_tail_before(&transcript_path, 1, Some(owner_offset))?;
            page.has_more = !preceding.entries.is_empty();
            page.next_cursor_offset = page.has_more.then_some(owner_offset);
        }
    }
    Ok(page)
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
            let (archival_message, mut input_message) =
                match build_turn_messages(&slot, text, &params) {
                    Ok(pair) => pair,
                    Err(error) => {
                        send_error(&state, id, Some(slot.session_id.clone()), error)?;
                        return Ok(());
                    }
                };
            let persisted = persist_turn_input_message(&slot, &archival_message, &params)?;
            input_message.msg_id = Some(persisted.row_id);
            if persisted.settled_pending_question && !slot.is_busy() {
                rehydrate_slot_context_state(&slot)?;
            }
            release_attachment_leases(&slot, &params);

            start_turn(state, slot, id, Some(input_message), TurnAck::Accepted).await?;
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
            let mut input_message = ChatMessage::steering(text);
            let persisted = persist_turn_input_message(&slot, &input_message, &params)?;
            input_message.msg_id = Some(persisted.row_id);
            if persisted.settled_pending_question && !slot.is_busy() {
                rehydrate_slot_context_state(&slot)?;
            }
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
            start_turn(state, slot, id, Some(input_message), TurnAck::Accepted).await?;
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
            let (archival_message, mut input_message) =
                match build_turn_messages(&slot, text, &params) {
                    Ok(pair) => pair,
                    Err(error) => {
                        send_error(&state, id, Some(slot.session_id.clone()), error)?;
                        return Ok(());
                    }
                };
            let persisted = persist_turn_input_message(&slot, &archival_message, &params)?;
            input_message.msg_id = Some(persisted.row_id);
            if persisted.settled_pending_question && !slot.is_busy() {
                rehydrate_slot_context_state(&slot)?;
            }
            release_attachment_leases(&slot, &params);
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
            start_turn(state, slot, id, Some(input_message), TurnAck::Accepted).await?;
        }
        ServeCommand::Retry {
            id,
            session_id,
            message_id,
        } => {
            let Some(slot) = resolve_slot_or_error(&state, id.clone(), session_id.clone()).await?
            else {
                return Ok(());
            };
            if slot.is_busy() {
                send_error(&state, id, Some(slot.session_id.clone()), "busy")?;
                return Ok(());
            }
            match slot
                .ctx
                .session_runtime
                .session
                .copy_user_message_forward(&message_id)
            {
                Ok(_) => {}
                Err(error) if is_config_error(&error, "retry_target_stale") => {
                    send_error(
                        &state,
                        id,
                        Some(slot.session_id.clone()),
                        "retry_target_stale",
                    )?;
                    return Ok(());
                }
                Err(error) => return Err(error),
            }
            // copy-forward changed the durable source of truth. Hydrate instead of building a
            // parallel in-memory copy so the next request is exactly the transcript we wrote.
            if let Err(error) = rehydrate_slot_context_state(&slot) {
                send_error(
                    &state,
                    id,
                    Some(slot.session_id.clone()),
                    format!("retry persisted but failed to refresh runtime context: {error}"),
                )?;
                return Ok(());
            }
            start_turn(state, slot, id, None, TurnAck::Accepted).await?;
        }
        ServeCommand::Resume { id, session_id } => {
            let Some(slot) = resolve_slot_or_error(&state, id.clone(), session_id).await? else {
                return Ok(());
            };
            if slot.is_busy() {
                send_error(&state, id, Some(slot.session_id.clone()), "busy")?;
                return Ok(());
            }
            let entries = slot.ctx.session_runtime.session.get_entries(256)?;
            if !crate::core::session::has_complete_tail_tool_results(&entries) {
                send_error(
                    &state,
                    id,
                    Some(slot.session_id.clone()),
                    "nothing_to_resume",
                )?;
                return Ok(());
            }
            start_turn(state, slot, id, None, TurnAck::Accepted).await?;
        }
        ServeCommand::NewSession { id, params } => {
            if params.detached {
                let entry = super::create_detached_session(&state, params)?;
                let session_id = entry.session_id;
                state.writer.send(OutFrame::Response(ResponseFrame::ok(
                    id,
                    Some(session_id.clone()),
                    Some(serde_json::json!({
                        "detached": true,
                        "sessionId": session_id,
                    })),
                )))?;
                return Ok(());
            }
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
            let page = complete_leading_tool_companions(
                &slot.ctx.session_runtime.session,
                &slot.session_id,
                page,
            )?;
            let next_cursor = encode_next_cursor(&page).map_err(|error| {
                AppError::Config(format!("encode get_messages cursor failed: {error}"))
            })?;
            let mut page = page;
            attach_page_tool_displays(
                &slot.ctx.session_runtime.session,
                &slot.session_id,
                &mut page.entries,
            )?;
            match params.attachment_mode {
                AttachmentMode::Inline => {
                    materialize_page_image_refs(&slot, &mut page.entries);
                }
                AttachmentMode::Reference => {
                    dereference_page_attachments(&slot, &mut page.entries);
                }
            }
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
            if let Err(error) = rehydrate_slot_context_state(&slot) {
                send_error(
                    &state,
                    id,
                    Some(slot.session_id.clone()),
                    format!("restore persisted but failed to refresh runtime context: {error}"),
                )?;
                return Ok(());
            }
            rearm_pending_question_after_transcript_change(Arc::clone(&state), Arc::clone(&slot))
                .await?;
            state.writer.send(OutFrame::Response(ResponseFrame::ok(
                id,
                Some(slot.session_id.clone()),
                Some(restore_core_payload(report)),
            )))?;
        }
        ServeCommand::Compact { id, session_id } => {
            let Some(slot) = resolve_slot_or_error(&state, id.clone(), session_id).await? else {
                return Ok(());
            };
            if slot.is_busy() {
                send_error(&state, id, Some(slot.session_id.clone()), "busy")?;
                return Ok(());
            }
            let report = match compact_session(&slot.ctx).await {
                Ok(report) => report,
                Err(error) => {
                    send_error(
                        &state,
                        id,
                        Some(slot.session_id.clone()),
                        format!("compact failed: {error}"),
                    )?;
                    return Ok(());
                }
            };
            // `/compact` 已将 boundary 写入 transcript。与 /restore 一样必须立刻重载
            // slot 的内存状态，否则下一轮会继续带着已失效的消息。
            if let Err(error) = rehydrate_slot_context_state(&slot) {
                send_error(
                    &state,
                    id,
                    Some(slot.session_id.clone()),
                    format!("compact persisted but failed to refresh runtime context: {error}"),
                )?;
                return Ok(());
            }
            state.writer.send(OutFrame::Response(ResponseFrame::ok(
                id,
                Some(slot.session_id.clone()),
                Some(json!({
                    "beforeUsageRatio": report.before_ratio,
                    "afterUsageRatio": report.after_ratio,
                    "coveredMessageCount": report.covered_count,
                })),
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
            let agent_mode = slot.ctx.session_runtime.plan_runtime.mode();
            let active_plan = slot.ctx.session_runtime.plan_runtime.active_plan();
            let active_plan_path_raw = active_plan.as_ref().map(|plan| plan.path.clone());
            let context_utilization_ratio = *slot.last_context_ratio.lock();
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
                    "workspaceMode": match slot.mode { crate::SessionMode::Code => "code", crate::SessionMode::Claw => "claw" },
                    "cwd": slot.cwd,
                    "model": model,
                    "thinkingLevel": thinking_level.as_str(),
                    "agentMode": agent_mode.as_str(),
                    "activePlan": active_plan.map(|plan| serde_json::json!({
                        "id": plan.id,
                        "path": crate::infra::platform::format_home_path(&plan.path),
                        "state": plan.state.as_str(),
                    })),
                    "planTodos": plan_todos,
                    "sessionTodos": session_todos,
                    "contextUtilizationRatio": context_utilization_ratio,
                    "sessionKey": slot.ctx.session_runtime.session.current_session_key(),
                })),
            )))?;
        }
        ServeCommand::IngestAttachment {
            id,
            session_id,
            attachment,
        } => {
            let Some(slot) = resolve_slot_or_error(&state, id.clone(), session_id.clone()).await?
            else {
                return Ok(());
            };
            match ingest_attachment(&slot, &attachment) {
                Ok(response) => {
                    state.writer.send(OutFrame::Response(ResponseFrame::ok(
                        id,
                        Some(slot.session_id.clone()),
                        Some(serde_json::to_value(response)?),
                    )))?;
                }
                Err(error) => {
                    send_error(&state, id, Some(slot.session_id.clone()), error)?;
                }
            }
        }
        ServeCommand::RetainAttachmentLeases {
            id,
            session_id,
            params,
        } => {
            if params.attachments.len() > 512 {
                send_error(&state, id, Some(session_id), "too_many_attachment_leases")?;
                return Ok(());
            }
            let shas = params
                .attachments
                .into_iter()
                .flat_map(|attachment| {
                    std::iter::once(attachment.blob_sha).chain(attachment.provider_sha)
                })
                .collect::<std::collections::BTreeSet<_>>();
            if shas.len() > 512 {
                send_error(&state, id, Some(session_id), "too_many_attachment_leases")?;
                return Ok(());
            }
            let Some(manager) = super::scoped_session_manager(&state) else {
                send_error(&state, id, Some(session_id), "session_manager_unavailable")?;
                return Ok(());
            };
            if manager.get_session_by_id(&session_id)?.is_none() {
                send_error(&state, id, Some(session_id), "unknown_session")?;
                return Ok(());
            }
            match manager
                .attachment_store()
                .retain_pending_batch(&session_id, shas)
            {
                Ok(retained) => {
                    state.writer.send(OutFrame::Response(ResponseFrame::ok(
                        id,
                        Some(session_id),
                        Some(serde_json::to_value(
                            super::types::RetainAttachmentLeasesResponse {
                                retained_shas: retained,
                            },
                        )?),
                    )))?;
                }
                Err(error) => {
                    send_error(&state, id, Some(session_id), error.to_string())?;
                }
            }
        }
        ServeCommand::CacheAttachmentThumbnail {
            id,
            session_id,
            thumbnail,
        } => {
            let Some(slot) = resolve_slot_or_error(&state, id.clone(), session_id.clone()).await?
            else {
                return Ok(());
            };
            match cache_attachment_thumbnail(&slot, &thumbnail) {
                Ok(()) => {
                    state.writer.send(OutFrame::Response(ResponseFrame::ok(
                        id,
                        Some(slot.session_id.clone()),
                        Some(json!({ "cached": true })),
                    )))?;
                }
                Err(error) => {
                    send_error(&state, id, Some(slot.session_id.clone()), error)?;
                }
            }
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
                    match slot.ctx.session_runtime.plan_runtime.enter_plan() {
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
                    match slot.ctx.session_runtime.plan_runtime.exit_plan() {
                        Ok(()) => {
                            state.writer.send(OutFrame::Response(ResponseFrame::ok(
                                id,
                                Some(slot.session_id.clone()),
                                Some(plan_state_payload(&slot, None)),
                            )))?;
                        }
                        Err(error) => {
                            let error_code = match error {
                                PlanRuntimeError::AlreadyInMode(_) => "plan_state_conflict",
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
                            let mut input_message = ChatMessage::user(format!(
                                "start building {}",
                                outcome.plan_path.to_string_lossy()
                            ));
                            input_message.kind = crate::core::llm::MessageKind::PlanBuild;
                            let persisted = persist_turn_input_message(
                                &slot,
                                &input_message,
                                &ServeMessageParams::default(),
                            )?;
                            input_message.msg_id = Some(persisted.row_id);
                            start_turn(
                                Arc::clone(&state),
                                slot,
                                id,
                                Some(input_message),
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
            let previous_entry = slot.ctx.session_runtime.session.current_session_entry()?;
            let model_changed = slot.ctx.effective_model(previous_entry.as_ref()) != model;
            slot.ctx
                .session_runtime
                .session
                .switch_current_model(None, Some(model.as_str()))?;
            if model_changed {
                let entry = slot.ctx.session_runtime.session.current_session_entry()?;
                let main_call = slot.ctx.resolve_call(LlmScene::Main, entry.as_ref())?;
                let updated_runtime_context = {
                    let mut turn_state = slot.turn_state.lock();
                    if let Some(state) = turn_state.as_mut() {
                        state.context_state.apply_limits(&main_call.limits);
                        state.context_budget_chars = state.context_state.context_budget_chars;
                        true
                    } else {
                        false
                    }
                };
                if updated_runtime_context {
                    emit_estimated_context_metrics_snapshot(&slot);
                }
            }
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
                .model_prefs
                .set_reasoning(&model, parsed_level)?;
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
        ServeCommand::SetContextWindow {
            id,
            session_id,
            model,
            context_window,
        } => {
            let Some(slot) = resolve_slot_or_error(&state, id.clone(), session_id.clone()).await?
            else {
                return Ok(());
            };
            let model = model.trim().to_string();
            let entry = match slot
                .ctx
                .global_services
                .model_catalog
                .lookup_explicit(&model)
            {
                Ok(entry) => entry,
                Err(error) => {
                    send_error(
                        &state,
                        id,
                        Some(slot.session_id.clone()),
                        render_error_message(&error),
                    )?;
                    return Ok(());
                }
            };
            if entry.context_window_options.is_empty()
                || !entry.context_window_options.contains(&context_window)
            {
                send_error(
                    &state,
                    id,
                    Some(slot.session_id.clone()),
                    format!(
                        "invalid_context_window: 模型 `{model}` 可选档位为 {:?}。",
                        entry.context_window_options
                    ),
                )?;
                return Ok(());
            }
            slot.ctx
                .global_services
                .model_prefs
                .set_context_window(&model, Some(context_window))?;
            let current_entry = slot.ctx.session_runtime.session.current_session_entry()?;
            let context_choice_applies = slot.ctx.effective_model(current_entry.as_ref()) == model;
            if context_choice_applies {
                let main_call = slot
                    .ctx
                    .resolve_call(LlmScene::Main, current_entry.as_ref())?;
                let updated_runtime_context = {
                    let mut turn_state = slot.turn_state.lock();
                    if let Some(state) = turn_state.as_mut() {
                        state.context_state.apply_limits(&main_call.limits);
                        state.context_budget_chars = state.context_state.context_budget_chars;
                        true
                    } else {
                        false
                    }
                };
                if updated_runtime_context {
                    emit_estimated_context_metrics_snapshot(&slot);
                }
            }
            state.writer.send(OutFrame::Response(ResponseFrame::ok(
                id,
                Some(slot.session_id.clone()),
                Some(serde_json::json!({
                    "sessionId": slot.session_id,
                    "model": model,
                    "contextWindow": context_window,
                })),
            )))?;
        }
        ServeCommand::ListModels { id } => {
            let catalog = resolve_model_catalog_snapshot(&state)?;
            let models = list_model_views_with_prefs(catalog.as_ref(), &state.shared_model_prefs);
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
        ServeCommand::ListConnectors { id } => {
            // A settings panel may open before any chat session exists. In that case,
            // expose the global configuration through an unbound connector registry;
            // it must not invent a workspace from the serve process cwd.
            let connector = resolve_connector_registry(&state)
                .or_else(|_| ConnectorRegistry::new(&state.cfg, None))?;
            let global_config_path = match global_mcp_path(&state.cfg) {
                Ok(path) => path,
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
            let project_config_path = connector
                .mcp_manager()
                .workspace_root()
                .map(|root| project_mcp_path(&state.cfg, root))
                .transpose()?;
            let servers = connector
                .mcp_manager()
                .statuses()
                .into_iter()
                .map(|status| {
                    let configured = connector.mcp_manager().configured_server(&status.name);
                    let config_path = match status.source {
                        McpConfigSource::Global => Some(global_config_path.clone()),
                        McpConfigSource::Project => project_config_path.clone(),
                    };
                    let config_path_display = config_path
                        .as_ref()
                        .map(|path| crate::infra::platform::format_home_path(path));
                    json!({
                        "name": status.name,
                        "source": status.source.as_str(),
                        "state": status.state.code(),
                        "trust": status.trust,
                        "toolCount": status.tool_count,
                        "resourceCount": status.resource_count,
                        "transport": if configured.as_ref().is_some_and(|server| server.config.url.is_some()) { "http" } else { "stdio" },
                        "url": configured.as_ref().and_then(|server| server.config.url.clone()),
                        "command": configured.as_ref().map(|server| server.config.command.clone()),
                        "configPath": config_path_display,
                        "configPathRaw": config_path.map(|path| path.to_string_lossy().to_string()),
                        "oauthConfigured": configured.as_ref().is_some_and(|server| {
                            server.config.oauth.is_some()
                                || server.config.auth.as_deref() == Some("oauth")
                                || status.state.code() == "needs_authorization"
                        }),
                        "auth": configured.as_ref().and_then(|server| server.config.auth.clone()),
                        "error": match &status.state {
                            ServerState::Failed(error) => Some(error.clone()),
                            _ => None,
                        },
                        "toolFilter": configured.as_ref().map(|server| {
                            json!({
                                "include": server.config.tool_filter.include,
                                "exclude": server.config.tool_filter.exclude,
                            })
                        }),
                    })
                })                .collect::<Vec<_>>();
            let config_paths = json!({
                "global": {
                    "display": crate::infra::platform::format_home_path(&global_config_path),
                    "raw": global_config_path.to_string_lossy().to_string(),
                },
                "workspace": project_config_path.as_ref().map(|path| json!({
                    "display": crate::infra::platform::format_home_path(path),
                    "raw": path.to_string_lossy().to_string(),
                })),
            });
            state.writer.send(OutFrame::Response(ResponseFrame::ok(
                id,
                state.registry.active_session_id(),
                Some(json!({ "connectors": servers, "configPaths": config_paths })),
            )))?;
        }
        ServeCommand::ListConnectorTools { id, name } => {
            let connector = match resolve_connector_registry(&state) {
                Ok(connector) => connector,
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
            let tools = match connector.mcp_manager().list_tools(&name) {
                Ok(tools) => tools,
                Err(error) => {
                    send_error(
                        &state,
                        id,
                        state.registry.active_session_id(),
                        render_error_message(&error),
                    )?;
                    return Ok(());
                }
            }
            .into_iter()
            .map(|tool| {
                json!({
                    "modelName": tool.name,
                    "rawName": tool.raw_name,
                    "label": tool.raw_name,
                    "description": tool.description,
                    "enabled": tool.enabled,
                })
            })
            .collect::<Vec<_>>();
            state.writer.send(OutFrame::Response(ResponseFrame::ok(
                id,
                state.registry.active_session_id(),
                Some(json!({ "name": name, "tools": tools })),
            )))?;
        }
        ServeCommand::AddConnector {
            id,
            name,
            command,
            args,
            url,
            mut headers,
            oauth,
            env,
            auth,
            scope,
        } => {
            let static_bearer = headers.iter().find_map(|(key, value)| {
                key.eq_ignore_ascii_case("authorization")
                    .then(|| {
                        value
                            .strip_prefix("Bearer ")
                            .or_else(|| value.strip_prefix("bearer "))
                    })
                    .flatten()
                    .map(ToOwned::to_owned)
            });
            let has_authorization_header = headers
                .keys()
                .any(|key| key.eq_ignore_ascii_case("authorization"));
            if has_authorization_header && static_bearer.is_none() {
                return Err(AppError::Config(
                    "Authorization headers must use the Bearer scheme".to_string(),
                ));
            }
            if has_authorization_header && auth.as_deref() == Some("oauth") {
                return Err(AppError::Config(
                    "OAuth connectors cannot include a static Authorization header".to_string(),
                ));
            }
            if auth.as_deref() == Some("bearer") && static_bearer.is_none() {
                return Err(AppError::Config(
                    "Bearer connectors require an Authorization header".to_string(),
                ));
            }
            if auth.as_deref() == Some("none") && static_bearer.is_some() {
                return Err(AppError::Config(
                    "auth=none cannot be combined with an Authorization header".to_string(),
                ));
            }
            headers.retain(|key, _| !key.eq_ignore_ascii_case("authorization"));
            let auth = auth.or_else(|| static_bearer.as_ref().map(|_| "bearer".to_string()));
            let oauth = oauth
                .map(serde_json::from_value::<McpOAuthConfig>)
                .transpose()
                .map_err(|error| {
                    AppError::Config(format!("invalid connector oauth config: {error}"))
                })?;
            let token_resource = url.clone();
            let config = McpServerConfig {
                command,
                args,
                url,
                auth,
                headers,
                oauth,
                env,
                cwd: None,
                trusted: false,
                integrity: None,
                startup_timeout_ms: 30_000,
                call_timeout_ms: 120_000,
                tool_filter: ToolFilter::default(),
            };
            let use_workspace = scope.as_deref() == Some("workspace");
            let workspace_root = resolve_connector_registry(&state)
                .ok()
                .and_then(|connector| {
                    connector
                        .mcp_manager()
                        .workspace_root()
                        .map(|path| path.to_path_buf())
                });
            let result = if use_workspace {
                match workspace_root.as_deref() {
                    Some(workspace_root) => {
                        upsert_project_server(&state.cfg, workspace_root, name.clone(), config)
                    }
                    None => Err(AppError::Config(
                        "workspace connector configuration requires an explicit session project root".into(),
                    )),
                }
            } else {
                upsert_global_server(&state.cfg, name.clone(), config)
            };
            if let Err(error) = result {
                send_error(
                    &state,
                    id,
                    state.registry.active_session_id(),
                    render_error_message(&error),
                )?;
                return Ok(());
            }
            if let Some(token) = static_bearer {
                let connector = resolve_connector_registry(&state)?;
                connector
                    .mcp_manager()
                    .save_static_bearer(&name, token, token_resource)?;
            }
            let connector = match resolve_connector_registry(&state) {
                Ok(connector) => connector,
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
            connector.mcp_manager().reload_configuration(&state.cfg)?;
            if use_workspace {
                connector.mcp_manager().approve(&name)?;
            }
            if let Err(error) = connector.mcp_manager().connect_server(&name).await {
                warn!(server = %name, error = %error, "connector configuration saved but server is not ready yet");
            }
            state.writer.send(OutFrame::Response(ResponseFrame::ok(
                id,
                state.registry.active_session_id(),
                Some(json!({ "name": name })),
            )))?;
        }
        ServeCommand::RemoveConnector { id, name } => {
            let connector = match resolve_connector_registry(&state) {
                Ok(connector) => connector,
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
            let removed = match connector
                .mcp_manager()
                .remove_configured_server(&name, &state.cfg)
            {
                Ok(removed) => removed,
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
            if removed {
                connector.reload().await?;
            }
            state.writer.send(OutFrame::Response(ResponseFrame::ok(
                id,
                state.registry.active_session_id(),
                Some(json!({ "name": name, "removed": removed })),
            )))?;
        }
        ServeCommand::SetConnectorTrust { id, name, trusted } => {
            let connector = match resolve_connector_registry(&state) {
                Ok(connector) => connector,
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
            let result = if trusted {
                connector.approve_and_connect(&name).await
            } else {
                connector.deny(&name)
            };
            if let Err(error) = result {
                send_error(
                    &state,
                    id,
                    state.registry.active_session_id(),
                    render_error_message(&error),
                )?;
                return Ok(());
            }
            state.writer.send(OutFrame::Response(ResponseFrame::ok(
                id,
                state.registry.active_session_id(),
                Some(json!({ "name": name, "trusted": trusted })),
            )))?;
        }
        ServeCommand::TestConnector { id, name } => {
            let connector = match resolve_connector_registry(&state) {
                Ok(connector) => connector,
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
            if let Err(error) = connector.mcp_manager().reconnect_server(&name).await {
                send_error(
                    &state,
                    id,
                    state.registry.active_session_id(),
                    render_error_message(&error),
                )?;
                return Ok(());
            }
            state.writer.send(OutFrame::Response(ResponseFrame::ok(
                id,
                state.registry.active_session_id(),
                Some(json!({ "name": name, "connected": true })),
            )))?;
        }
        ServeCommand::LoginConnector { id, name } => {
            let connector = match resolve_connector_registry(&state) {
                Ok(connector) => connector,
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
            let manager = connector.mcp_manager();
            let login_name = name.clone();
            tokio::spawn(async move {
                if let Err(error) = manager.login_server(&login_name).await {
                    warn!(server = %login_name, error = %error, "connector OAuth login failed");
                }
            });
            state.writer.send(OutFrame::Response(ResponseFrame::ok(
                id,
                state.registry.active_session_id(),
                Some(json!({ "name": name, "authorizing": true })),
            )))?;
        }
        ServeCommand::CancelLoginConnector { id, name } => {
            let connector = match resolve_connector_registry(&state) {
                Ok(connector) => connector,
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
            let cancelled = connector.mcp_manager().cancel_login(&name);
            state.writer.send(OutFrame::Response(ResponseFrame::ok(
                id,
                state.registry.active_session_id(),
                Some(json!({ "name": name, "cancelled": cancelled })),
            )))?;
        }
        ServeCommand::LogoutConnector { id, name } => {
            let connector = match resolve_connector_registry(&state) {
                Ok(connector) => connector,
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
            let removed = connector.mcp_manager().logout_server(&name)?;
            state.writer.send(OutFrame::Response(ResponseFrame::ok(
                id,
                state.registry.active_session_id(),
                Some(json!({ "name": name, "loggedOut": removed })),
            )))?;
        }
        ServeCommand::ReloadConnector { id } => {
            let connector = match resolve_connector_registry(&state) {
                Ok(connector) => connector,
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
            if let Err(error) = connector.reload().await {
                send_error(
                    &state,
                    id,
                    state.registry.active_session_id(),
                    render_error_message(&error),
                )?;
                return Ok(());
            }
            state.writer.send(OutFrame::Response(ResponseFrame::ok(
                id,
                state.registry.active_session_id(),
                Some(json!({ "reloaded": true })),
            )))?;
        }
        ServeCommand::SetConnectorToolFilter {
            id,
            name,
            include,
            exclude,
            scope: _,
        } => {
            let connector = match resolve_connector_registry(&state) {
                Ok(connector) => connector,
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
            let filter = ToolFilter { include, exclude };
            if let Err(error) = connector
                .mcp_manager()
                .set_configured_tool_filter(&name, filter, &state.cfg)
            {
                send_error(
                    &state,
                    id,
                    state.registry.active_session_id(),
                    render_error_message(&error),
                )?;
                return Ok(());
            }
            if let Ok(connector) = resolve_connector_registry(&state) {
                if let Err(error) = connector.reload().await {
                    send_error(
                        &state,
                        id,
                        state.registry.active_session_id(),
                        render_error_message(&error),
                    )?;
                    return Ok(());
                }
            }
            state.writer.send(OutFrame::Response(ResponseFrame::ok(
                id,
                state.registry.active_session_id(),
                Some(json!({ "name": name, "updated": true })),
            )))?;
        }
        ServeCommand::DiscardDetachedSession { id, session_id } => {
            if state.registry.get(&session_id).is_some() {
                send_error(&state, id, Some(session_id), "session_is_live")?;
                return Ok(());
            }
            let sessions_dir = crate::resolve_sessions_dir(&state.cfg)?;
            let lookup = SessionManager::new(sessions_dir.clone());
            let Some(entry) = lookup.get_session_by_id(&session_id)? else {
                state.writer.send(OutFrame::Response(ResponseFrame::ok(
                    id,
                    Some(session_id.clone()),
                    Some(serde_json::json!({ "discarded": false, "sessionId": session_id })),
                )))?;
                return Ok(());
            };
            let manager = SessionManager::new_scoped(sessions_dir, entry.session_key);
            manager.delete_session(&session_id)?;
            state.writer.send(OutFrame::Response(ResponseFrame::ok(
                id,
                Some(session_id.clone()),
                Some(serde_json::json!({ "discarded": true, "sessionId": session_id })),
            )))?;
        }
        ServeCommand::CloseSession { id, session_id } => {
            let Some(slot) = resolve_slot_or_error(&state, id.clone(), session_id.clone()).await?
            else {
                return Ok(());
            };
            state
                .ask_question
                .cancel_live_session(&slot.session_id, "close_session");
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

/// Join the UI-only sidecar only for tool results contained in this response page.
/// The transcript remains the authoritative LLM history; the sidecar is a bounded
/// rendering enhancement and never participates in model replay.
fn attach_page_tool_displays(
    session: &SessionManager,
    session_id: &str,
    entries: &mut [TranscriptEntry],
) -> Result<(), AppError> {
    let oldest_page_message_at = entries
        .iter()
        .filter_map(|entry| match entry {
            TranscriptEntry::Message(message) => DateTime::parse_from_rfc3339(&message.timestamp)
                .ok()
                .map(|timestamp| timestamp.with_timezone(&Utc)),
            _ => None,
        })
        .min();
    let wanted = entries
        .iter()
        .filter_map(|entry| match entry {
            TranscriptEntry::Message(message) => message
                .message
                .get("tool_call_id")
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned),
            _ => None,
        })
        .collect::<HashSet<_>>();
    let displays = read_tool_displays_for_calls(
        &session.transcript_path(session_id),
        &wanted,
        oldest_page_message_at,
    )?;
    for entry in entries {
        let TranscriptEntry::Message(message) = entry else {
            continue;
        };
        let Some(tool_call_id) = message
            .message
            .get("tool_call_id")
            .and_then(serde_json::Value::as_str)
        else {
            continue;
        };
        let Some(display) = displays.get(tool_call_id) else {
            continue;
        };
        let object = message.message.as_object_mut().ok_or_else(|| {
            AppError::Config("tool transcript message is not an object".to_string())
        })?;
        object.insert("tool_display".to_string(), serde_json::to_value(display)?);
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

fn resolve_connector_registry(
    state: &ServeState,
) -> Result<Arc<crate::core::connector::ConnectorRegistry>, AppError> {
    resolve_active_slot(state)?
        .ctx
        .global_services
        .connector_registry
        .clone()
        .ok_or_else(|| AppError::Config("connector_disabled".to_string()))
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
            detached: false,
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
    let active_plan = plan_runtime.active_plan().map(|plan| {
        let path = plan_path_override
            .clone()
            .unwrap_or_else(|| crate::infra::platform::format_home_path(&plan.path));
        serde_json::json!({
            "id": plan.id,
            "path": path,
            "state": plan.state.as_str(),
        })
    });
    serde_json::json!({
        "sessionId": slot.session_id,
        "agentMode": plan_runtime.mode().as_str(),
        "activePlan": active_plan,
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
        PlanRuntimeError::UnsafePlanId(_) | PlanRuntimeError::Io(_) => "plan_io_error",
        PlanRuntimeError::BuildBlocked(_) => "plan_build_blocked",
        PlanRuntimeError::BuildPlanNotFound { .. }
        | PlanRuntimeError::BuildPlanPathNotFound { .. } => "plan_not_found",
    }
}

pub(crate) async fn start_turn(
    state: Arc<ServeState>,
    slot: Arc<super::registry::SessionSlot>,
    id: Option<String>,
    input_message: Option<ChatMessage>,
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
        TurnAck::Silent => {}
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
            Ok(Ok(crate::AgentRunOutcome::Completed(_))) => {}
            Ok(Ok(crate::AgentRunOutcome::Interrupted(_))) => {
                if let Err(error) = slot_for_task
                    .ctx
                    .session_runtime
                    .plan_runtime
                    .park_executing_plan()
                {
                    tracing::warn!(
                        session_id = %slot_for_task.session_id,
                        error = %error,
                        "failed to park executing plan after serve interruption"
                    );
                }
            }
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

/// 从当前 slot 上下文生成一条估算 metrics。
///
/// 冷启动 slot 还没有 provider usage 时，事件泵会保持水位未知；已有实测水位时，
/// 这条事件会用压缩、回滚或模型预算变化后的最佳估算刷新 UI。
fn emit_estimated_context_metrics_snapshot(slot: &Arc<super::registry::SessionSlot>) {
    let event = {
        let mut turn_state = slot.turn_state.lock();
        let Some(state) = turn_state.as_mut() else {
            return;
        };
        let context_state = &mut state.context_state;
        let input_tokens_used = context_state.estimated_token_count();
        let context_utilization_ratio = context_state.usage_ratio();
        let preheat_in_progress = context_state.preheat.is_warmup_task_active();
        let preheat_result_pending =
            context_state.preheat.preheat_result_pending() && !preheat_in_progress;
        context_state.live.input_tokens_used = input_tokens_used;
        context_state.live.context_utilization_ratio = context_utilization_ratio;
        context_state.live.preheat_in_progress = preheat_in_progress;
        context_state.live.preheat_result_pending = preheat_result_pending;

        AgentEvent::ContextMetricsUpdate {
            input_tokens_used,
            context_utilization_ratio,
            provider_usage_measured: false,
            compaction_count: context_state.session_obs.compaction_count,
            compaction_tokens_freed: context_state.session_obs.compaction_tokens_freed,
            total_tool_result_bytes_persisted: context_state
                .session_obs
                .tool_result_chars_persisted,
            prompt_tokens_total: context_state.session_obs.prompt_tokens_total,
            cache_read_tokens_total: context_state.session_obs.cache_read_tokens_total,
            cache_hit_ratio: context_state.session_obs.cache_hit_ratio(),
            consecutive_miss_max: context_state.session_obs.consecutive_cache_miss_max,
            tail_changed_count: context_state.session_obs.tail_changed_count,
            tail_change_miss_tokens: context_state.session_obs.tail_change_miss_tokens,
            preheat_in_progress,
            preheat_result_pending,
        }
    };

    let emitter = crate::infra::event_bus::ScopedEventEmitter::new(
        Arc::clone(&slot.ctx.global_services.event_bus),
        slot.session_id.clone(),
    );
    let _ = emitter.emit(event);
}

/// 任何直接改写 transcript 历史的 serve 命令都经此处同步内存 context。
///
/// `/compact`、`/restore` 与 pending-question 结算都会改变逻辑消息链；只改 JSONL 会让
/// 下一轮仍发送旧内存副本。入口处已保证 slot 不 busy，故可安全读取并替换 turn state。
fn rehydrate_slot_context_state(slot: &Arc<super::registry::SessionSlot>) -> Result<(), AppError> {
    let system_text = slot
        .turn_state
        .lock()
        .as_ref()
        .map(|state| state.prompt_snapshot.system_text().to_string())
        .ok_or_else(|| AppError::Config("session runtime is unavailable".to_string()))?;
    let entry = slot
        .ctx
        .session_runtime
        .session
        .get_session(slot.ctx.session_runtime.session.current_session_key())?;
    let main_call = slot.ctx.resolve_call(LlmScene::Main, entry.as_ref())?;
    let context_state = init_context_state_with_limits(
        &slot.ctx.session_runtime.session,
        &slot.ctx.config.context,
        &system_text,
        &main_call.limits,
    )?;
    let context_budget_chars = context_state.context_budget_chars;
    let mut turn_state = slot.turn_state.lock();
    let state = turn_state
        .as_mut()
        .ok_or_else(|| AppError::Config("session runtime is unavailable".to_string()))?;
    state.context_state = context_state;
    state.context_budget_chars = context_budget_chars;
    drop(turn_state);
    emit_estimated_context_metrics_snapshot(slot);
    Ok(())
}

/// `/restore` 可能把一个未回答的问题重新带回 transcript 尾部。它与 session attach
/// 共享同一条 no-input resume 入口，避免「磁盘里已是 pending、界面却没有卡片」。
async fn rearm_pending_question_after_transcript_change(
    state: Arc<ServeState>,
    slot: Arc<super::registry::SessionSlot>,
) -> Result<(), AppError> {
    if !crate::api::chat::has_resumable_tail_ask_question(&slot.ctx.session_runtime.session)? {
        return Ok(());
    }
    start_turn(state, slot, None, None, TurnAck::Silent).await
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

/// 构造一个回合的两条消息：落 transcript 的那条与发给模型的那条。
///
/// 为什么必须是两条：transcript 是不可变档案，要留住用户**实际附上**的东西（比如原始 SVG）；
/// 而模型的视觉接口只认位图，SVG 得换成 webview 在 ingest 时转好的 PNG。
///
/// 旧实现的做法是「先建一条，再无条件 `.clone()` 一份去做 SVG 替换」——
/// 11 张图就是把几十 MB base64 白拷一次，哪怕一张 SVG 都没有。这里改成只在
/// 真的存在 provider 覆盖（即某个附件带 `provider_sha`）时才构造第二条，否则直接复用同一条。
fn build_turn_messages(
    slot: &Arc<super::registry::SessionSlot>,
    text: String,
    params: &ServeMessageParams,
) -> Result<(ChatMessage, ChatMessage), String> {
    let store = slot.ctx.session_runtime.session.attachment_store();
    let archival = build_user_message(&store, text, params, AttachmentBytes::Archival)?;
    // Sent image history uses a content reference, while the provider needs base64 for
    // this request. Only image blobs therefore need distinct archival/input messages.
    let needs_provider_message = params.attachments.iter().any(|attachment| {
        attachment.kind == ServeAttachmentKind::Image && attachment.blob_sha.is_some()
    });
    if !needs_provider_message {
        let input = archival.clone();
        return Ok((archival, input));
    }
    let text_for_provider = single_text_of(&archival);
    let input = build_user_message(&store, text_for_provider, params, AttachmentBytes::Provider)?;
    Ok((archival, input))
}

/// `build_user_message` 取哪一份字节。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AttachmentBytes {
    /// 用户实际附上的那份，落 transcript。
    Archival,
    /// 发给模型的那份；SVG 会取 webview 转好的 PNG。
    Provider,
}

/// 当 params 没有 segments 时，`build_user_message` 只用到 `text`；
/// 构造第二条消息时把它取回来，避免要求调用方复制一份 String。
fn single_text_of(message: &ChatMessage) -> String {
    match message.content.as_ref() {
        Some(ChatMessageContent::Text(text)) => text.clone(),
        Some(ChatMessageContent::Parts(parts)) => parts
            .iter()
            .find_map(|part| match part {
                ChatMessageContentPart::InputText { text } => Some(text.clone()),
                _ => None,
            })
            .unwrap_or_default(),
        None => String::new(),
    }
}

pub(crate) fn build_user_message(
    store: &AttachmentBlobStore,
    text: String,
    params: &ServeMessageParams,
    which: AttachmentBytes,
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
        parts.push(resolve_attachment_part(store, attachment, which)?);
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

/// 把输入消息落进 transcript，返回它的 row id。
///
/// Persist 结果只暴露调用方必须知道的两件事：稳定 row id，以及 session 门闩是否刚
/// 结算了 pending question。后者让 serve 刷新内存 context，却不在上层重复改写 transcript。
struct PersistedTurnInput {
    row_id: String,
    settled_pending_question: bool,
}

fn persist_turn_input_message(
    slot: &Arc<super::registry::SessionSlot>,
    message: &ChatMessage,
    params: &ServeMessageParams,
) -> Result<PersistedTurnInput, AppError> {
    let payload = serde_json::to_value(message)?;
    if let Some(forced_id) = normalized_user_message_id(params) {
        if slot
            .ctx
            .session_runtime
            .session
            .get_entry_for_session(&slot.session_id, forced_id)?
            .is_none()
        {
            return slot
                .ctx
                .session_runtime
                .session
                .append_message_with_id_and_pending_resolution(payload, forced_id)
                .map(|(row_id, settled_pending_question)| PersistedTurnInput {
                    row_id,
                    settled_pending_question,
                });
        }
    }
    slot.ctx
        .session_runtime
        .session
        .append_message_with_pending_resolution(payload)
        .map(|(row_id, settled_pending_question)| PersistedTurnInput {
            row_id,
            settled_pending_question,
        })
}

/// 发送成功后释放这一回合用到的全部附件租约。
///
/// 释放租约就是「零拷贝提升」的全部动作 —— 字节原地不动，只是不再被当作待清理的草稿字节。
/// 失败只记日志：租约没释放的后果是 GC 晚一轮回收，不影响用户，不值得让发送失败。
fn release_attachment_leases(
    slot: &Arc<super::registry::SessionSlot>,
    params: &ServeMessageParams,
) {
    let store = slot.ctx.session_runtime.session.attachment_store();
    for attachment in &params.attachments {
        for sha in [
            attachment.blob_sha.as_deref(),
            attachment.provider_sha.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            if let Err(error) = store.promote(&slot.session_id, sha) {
                tracing::warn!(
                    "serve: failed to release attachment lease {sha} for {}: {error}",
                    slot.session_id
                );
            }
        }
    }
}

/// 收录一份附件字节，换回内容哈希。
///
/// 这是全协议唯一接收图片字节的地方，因此也是**唯一的权威校验点**：
/// 扩展层的检查只用于即时 UI 反馈，永远不被信任。发送时 prompt 只带哈希，
/// 后端按哈希取自己已校验过的字节，客户端无法绕过校验。
fn ingest_attachment(
    slot: &Arc<super::registry::SessionSlot>,
    input: &IngestAttachmentInput,
) -> Result<IngestAttachmentResponse, String> {
    let bytes = decode_base64(&input.data_base64, "dataBase64")?;
    match input.kind {
        ServeAttachmentKind::Image => validate_image_bytes(&bytes, &input.mime_type)?,
        ServeAttachmentKind::File => validate_file_bytes(&bytes, &input.mime_type)?,
    }

    let store = slot.ctx.session_runtime.session.attachment_store();
    let lease = |sha: &str| -> Result<(), String> {
        store
            .mark_pending(&slot.session_id, sha)
            .map_err(|error| format!("unable to record attachment lease: {error}"))
    };
    let persist = |bytes: &[u8]| -> Result<String, String> {
        let sha = store
            .put(bytes)
            .map_err(|error| format!("unable to store attachment bytes: {error}"))?;
        lease(&sha)?;
        Ok(sha)
    };

    let blob_sha = persist(&bytes)?;

    // 缩略图是可选的：webview 生成失败时降级为直接引用原图，功能不受影响。
    //
    // 存进 `thumbs/<blob_sha>` 而不是当成一份普通 blob：缩略图是纯派生数据，
    // 不该占租约、不该参与 blob GC，被淘汰了下次重新生成即可。这也让「新粘贴的图」
    // 与「历史图补交的缩略图」落在同一个位置，宿主只需要一条查找规则。
    let has_thumb = match input.thumb_base64.as_deref() {
        Some(encoded) => {
            let thumb = decode_base64(encoded, "thumbBase64")?;
            validate_image_bytes(&thumb, "image/png")
                .map_err(|error| format!("invalid thumbnail: {error}"))?;
            store
                .put_thumbnail(&blob_sha, &thumb)
                .map_err(|error| format!("unable to store thumbnail: {error}"))?;
            true
        }
        None => false,
    };

    // provider 覆盖同样可选，只有 SVG 才有。
    let provider_sha = match input.provider_base64.as_deref() {
        Some(encoded) => {
            let provider_mime = input.provider_mime_type.as_deref().unwrap_or("image/png");
            let provider = decode_base64(encoded, "providerBase64")?;
            validate_image_bytes(&provider, provider_mime)
                .map_err(|error| format!("invalid provider rendition: {error}"))?;
            Some(persist(&provider)?)
        }
        None => None,
    };

    Ok(IngestAttachmentResponse {
        blob_sha,
        has_thumb,
        provider_sha,
        bytes: bytes.len() as u64,
        mime_type: input.mime_type.clone(),
        filename: safe_filename(input.filename.as_deref(), &input.mime_type),
    })
}

/// 收下一张历史图的缩略图。
fn cache_attachment_thumbnail(
    slot: &Arc<super::registry::SessionSlot>,
    input: &CacheThumbnailInput,
) -> Result<(), String> {
    let thumb = decode_base64(&input.thumb_base64, "thumbBase64")?;
    validate_image_bytes(&thumb, "image/png")
        .map_err(|error| format!("invalid thumbnail: {error}"))?;
    let store = slot.ctx.session_runtime.session.attachment_store();
    if !store.exists(&input.source_sha) {
        return Err(format!(
            "unknown attachment blob {}; nothing to attach a thumbnail to",
            input.source_sha
        ));
    }
    store
        .put_thumbnail(&input.source_sha, &thumb)
        .map_err(|error| format!("unable to store thumbnail: {error}"))?;
    evict_rebuildable(&store);
    Ok(())
}

/// Restore byte-free transcript image references for the legacy CLI history contract.
///
/// Webviews use [`AttachmentMode::Reference`] and must never receive the encoded bytes.
/// The default `inline` mode remains available for terminal callers that historically
/// received `input_image.image_b64` in `get_messages`.
fn materialize_page_image_refs(
    slot: &Arc<super::registry::SessionSlot>,
    entries: &mut [TranscriptEntry],
) {
    let store = slot.ctx.session_runtime.session.attachment_store();
    for entry in entries.iter_mut() {
        let TranscriptEntry::Message(message) = entry else {
            continue;
        };
        let Some(parts) = message
            .message
            .get_mut("content")
            .and_then(|content| content.as_array_mut())
        else {
            continue;
        };
        for part in parts {
            let Some(object) = part.as_object_mut() else {
                continue;
            };
            if object.get("type").and_then(|value| value.as_str()) != Some("input_image_ref") {
                continue;
            }
            let Some(sha) = object
                .get("blob_sha")
                .and_then(|value| value.as_str())
                .map(ToOwned::to_owned)
            else {
                continue;
            };
            match store.get(&sha) {
                Ok(Some(bytes)) => {
                    object.insert("type".to_string(), json!("input_image"));
                    object.remove("blob_sha");
                    object.insert(
                        "image_b64".to_string(),
                        json!(base64::engine::general_purpose::STANDARD.encode(bytes)),
                    );
                }
                Ok(None) => {
                    tracing::warn!("serve: transcript image reference points at missing blob {sha}")
                }
                Err(error) => tracing::warn!(
                    "serve: failed to materialize transcript image reference {sha}: {error}"
                ),
            }
        }
    }
}

/// Add webview-only metadata to durable image references without reading image bytes.
/// The transcript itself remains byte-free; Chromium resolves `blobSha` from the attachment
/// root supplied during the initialize handshake.
fn dereference_page_attachments(
    slot: &Arc<super::registry::SessionSlot>,
    entries: &mut [TranscriptEntry],
) {
    let store = slot.ctx.session_runtime.session.attachment_store();
    for entry in entries.iter_mut() {
        let TranscriptEntry::Message(message) = entry else {
            continue;
        };
        let Some(parts) = message
            .message
            .get_mut("content")
            .and_then(|content| content.as_array_mut())
        else {
            continue;
        };
        for part in parts {
            let Some(object) = part.as_object_mut() else {
                continue;
            };
            if object.get("type").and_then(|t| t.as_str()) == Some("input_image_ref") {
                let Some(sha) = object
                    .get("blob_sha")
                    .and_then(|value| value.as_str())
                    .map(ToOwned::to_owned)
                else {
                    continue;
                };
                if store.exists(&sha) {
                    object.insert("blobSha".to_string(), json!(sha));
                    object.insert("hasThumb".to_string(), json!(store.has_thumbnail(&sha)));
                } else {
                    tracing::warn!(
                        "serve: transcript image reference points at missing blob {sha}"
                    );
                }
                continue;
            }

            let (encoded_field, encoded, mime_type, has_thumb) =
                match object.get("type").and_then(|t| t.as_str()) {
                    Some("input_file") => {
                        let Some(encoded) = object
                            .get("file_b64")
                            .and_then(|d| d.as_str())
                            .map(ToString::to_string)
                        else {
                            continue;
                        };
                        let mime_type = object
                            .get("mime_type")
                            .and_then(|value| value.as_str())
                            .or_else(|| object.get("mimeType").and_then(|value| value.as_str()))
                            .unwrap_or("application/pdf")
                            .to_string();
                        let filename = crate::core::session::attachments::safe_filename(
                            object.get("filename").and_then(|value| value.as_str()),
                            &mime_type,
                        );
                        object.insert("filename".to_string(), json!(filename));
                        ("file_b64", encoded, Some(mime_type), false)
                    }
                    _ => continue,
                };
            let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(&encoded) else {
                continue;
            };
            match store.put(&bytes) {
                Ok(sha) => {
                    object.remove(encoded_field);
                    object.insert("blobSha".to_string(), json!(sha));
                    object.insert("bytes".to_string(), json!(bytes.len()));
                    if has_thumb {
                        object.insert("hasThumb".to_string(), json!(store.has_thumbnail(&sha)));
                    }
                    if let Some(mime_type) = mime_type {
                        object.insert("mimeType".to_string(), json!(mime_type));
                    }
                }
                Err(error) => {
                    tracing::warn!("serve: unable to materialize history attachment: {error}");
                }
            }
        }
    }
}

/// 顺手把可重建的缩略图压回预算内。
fn evict_rebuildable(store: &crate::core::session::attachments::AttachmentBlobStore) {
    let _ = store.evict_rebuildable_over_budget(REBUILDABLE_MAX_BYTES);
}

fn decode_base64(encoded: &str, field: &str) -> Result<Vec<u8>, String> {
    base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|error| format!("invalid base64 in {field}: {error}"))
}

/// 把一个附件引用还原成消息里的一个 content part。
///
/// 校验顺序是刻意的：**先看声明是否自洽，再去取字节**。
/// 声明层面的错（缺 mimeType、文件类型不是 PDF、缺 filename）与字节无关，
/// 早报早止，也不必为了报这个错先读一遍磁盘。
fn resolve_attachment_part(
    store: &AttachmentBlobStore,
    attachment: &ServeAttachment,
    which: AttachmentBytes,
) -> Result<ChatMessageContentPart, String> {
    match (&attachment.blob_sha, &attachment.file_id) {
        (Some(_), Some(_)) => {
            return Err(
                "invalid_attachment: blobSha and fileId are mutually exclusive".to_string(),
            );
        }
        (None, None) => {
            return Err(
                "invalid_attachment: exactly one of blobSha or fileId is required".to_string(),
            );
        }
        _ => {}
    }

    if let Some(file_id) = attachment.file_id.clone() {
        return match attachment.kind {
            ServeAttachmentKind::Image => ChatMessageContentPart::image_file_id(file_id)
                .map_err(|error| format!("invalid_attachment: {error}")),
            ServeAttachmentKind::File => {
                ChatMessageContentPart::file_file_id(file_id, attachment.filename.clone())
                    .map_err(|error| format!("invalid_attachment: {error}"))
            }
        };
    }

    // ── 声明自洽性 ──
    let declared_mime = attachment
        .mime_type
        .clone()
        .ok_or_else(|| match attachment.kind {
            ServeAttachmentKind::Image => {
                "invalid_attachment: image attachment requires mimeType".to_string()
            }
            ServeAttachmentKind::File => {
                "invalid_attachment: file attachment requires mimeType".to_string()
            }
        })?;
    let file_name = match attachment.kind {
        ServeAttachmentKind::File => {
            if !declared_mime.eq_ignore_ascii_case("application/pdf") {
                return Err(format!(
                    "invalid_attachment: file attachments only support application/pdf; use kind=image for images (got {declared_mime})"
                ));
            }
            Some(attachment.filename.clone().ok_or_else(|| {
                "invalid_attachment: file attachment requires filename".to_string()
            })?)
        }
        ServeAttachmentKind::Image => None,
    };

    let blob_sha = attachment
        .blob_sha
        .as_deref()
        .expect("blobSha presence checked above");
    crate::core::session::attachments::validate_blob_sha(blob_sha)
        .map_err(|error| format!("invalid_attachment: {error}"))?;
    if attachment.kind == ServeAttachmentKind::Image && which == AttachmentBytes::Archival {
        if !store.exists(blob_sha) {
            return Err(format!(
                "invalid_attachment: unknown attachment blob {blob_sha}; call ingest_attachment first"
            ));
        }
        return ChatMessageContentPart::image_blob_ref(blob_sha, declared_mime, None)
            .map_err(|error| format!("invalid_attachment: {error}"));
    }

    // ── 取字节 ──
    // Provider 方向优先取 provider_sha（SVG 转出的 PNG），其余情况两者相同。
    let sha = match which {
        AttachmentBytes::Provider => attachment.provider_sha.as_deref().unwrap_or(blob_sha),
        AttachmentBytes::Archival => blob_sha,
    };
    let bytes = store
        .get(sha)
        .map_err(|error| format!("invalid_attachment: {error}"))?
        .ok_or_else(|| {
            format!(
                "invalid_attachment: unknown attachment blob {sha}; call ingest_attachment first"
            )
        })?;
    let data = base64::engine::general_purpose::STANDARD.encode(&bytes);

    match attachment.kind {
        ServeAttachmentKind::Image => {
            let mime_type = resolved_image_mime(attachment, which, declared_mime);
            if mime_type == "image/svg+xml" {
                ChatMessageContentPart::validated_svg_base64_for_transcript(data)
                    .map_err(|error| format!("invalid_attachment: {error}"))
            } else {
                ChatMessageContentPart::image_base64_data(mime_type, data)
                    .map_err(|error| format!("invalid_attachment: {error}"))
            }
        }
        ServeAttachmentKind::File => ChatMessageContentPart::file_base64_data(
            file_name.expect("filename validated above for file attachments"),
            declared_mime,
            data,
        )
        .map_err(|error| format!("invalid_attachment: {error}")),
    }
}

/// 取这一份字节对应的 MIME。
///
/// SVG 在 provider 方向已经被 webview 换成了 PNG，所以声明的 MIME 也要跟着换，
/// 否则会把 PNG 字节标成 `image/svg+xml` 发出去。
fn resolved_image_mime(
    attachment: &ServeAttachment,
    which: AttachmentBytes,
    declared: String,
) -> String {
    let overridden = which == AttachmentBytes::Provider
        && attachment
            .provider_sha
            .as_deref()
            .is_some_and(|provider| Some(provider) != attachment.blob_sha.as_deref());
    if overridden {
        "image/png".to_string()
    } else {
        declared
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
