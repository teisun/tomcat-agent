use std::sync::Arc;

use tracing::warn;

use crate::api::chat::context::ChatContext;
use crate::core::llm::ChatMessage;
use crate::infra::error::AppError;
use crate::{compound_turn_id, CheckpointKind, CheckpointRecordRequest};

pub(super) fn append_turn_message_if_needed(
    sink: &Arc<dyn crate::core::session::manager::MessageAppendSink>,
    msg: &mut ChatMessage,
) -> Result<(), AppError> {
    if msg.msg_id.is_some() {
        return Ok(());
    }
    let row_id = sink.append_message(serde_json::to_value(&*msg)?)?;
    msg.msg_id = Some(row_id);
    Ok(())
}

pub(super) fn push_turn_message(
    messages: &mut Vec<ChatMessage>,
    sink: &Arc<dyn crate::core::session::manager::MessageAppendSink>,
    mut msg: ChatMessage,
) -> Result<(), AppError> {
    append_turn_message_if_needed(sink, &mut msg)?;
    messages.push(msg);
    Ok(())
}

pub(crate) fn schedule_checkpoint_prune(ctx: &ChatContext) {
    let store = ctx.checkpoint_store.clone();
    let retention = crate::core::RetentionPolicy {
        retention_max: ctx.config.checkpoint.retention_max,
        retention_days: ctx.config.checkpoint.retention_days,
    };
    std::thread::spawn(move || {
        if let Err(err) = store.prune(retention) {
            warn!(error = %err, "checkpoint prune failed");
        }
    });
}

pub(crate) fn persist_turn_result(
    ctx: &ChatContext,
    context_state: &mut crate::core::ContextState,
    new_messages: Vec<crate::core::llm::ChatMessage>,
    kind: CheckpointKind,
) -> Result<Vec<String>, AppError> {
    let mut appended_row_ids = Vec::new();
    for message in new_messages {
        let mut chat_message = message;
        if chat_message.msg_id.is_none() {
            let row_id = ctx
                .session
                .append_message(serde_json::to_value(&chat_message)?)?;
            chat_message.msg_id = Some(row_id);
        }
        let row_id = chat_message.msg_id.clone().unwrap_or_default();
        if !row_id.is_empty() {
            appended_row_ids.push(row_id.clone());
        }
        let already_present = chat_message.msg_id.as_deref().is_some_and(|msg_id| {
            context_state
                .messages
                .iter()
                .any(|existing| existing.msg_id.as_deref() == Some(msg_id))
        });
        if !already_present {
            context_state.messages.push(chat_message);
        }
    }
    ctx.session.persist_context_observability(context_state)?;
    maybe_record_turn_checkpoint(ctx, kind, &appended_row_ids);
    Ok(appended_row_ids)
}

fn maybe_record_turn_checkpoint(
    ctx: &ChatContext,
    kind: CheckpointKind,
    appended_row_ids: &[String],
) {
    let Ok(Some(session_id)) = ctx.session.current_session_id() else {
        return;
    };
    let Some(request) = build_turn_checkpoint_request(&session_id, kind, appended_row_ids) else {
        return;
    };
    if let Err(err) = ctx.checkpoint_store.record(request) {
        warn!(error = %err, "checkpoint record failed");
    }
}

pub(crate) fn build_turn_checkpoint_request(
    session_id: &str,
    kind: CheckpointKind,
    appended_row_ids: &[String],
) -> Option<CheckpointRecordRequest> {
    let (Some(start_id), Some(end_id)) = (appended_row_ids.first(), appended_row_ids.last()) else {
        return None;
    };
    Some(CheckpointRecordRequest {
        session_id: session_id.to_string(),
        turn_id: compound_turn_id(start_id, end_id),
        kind,
        message_anchor: Some(end_id.clone()),
        notes: None,
    })
}
