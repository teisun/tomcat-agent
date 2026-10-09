//! Historical prompt editing: one ordered command, and a separate read-only preview.
use super::{commands, registry::SessionSlot, types::*, ServeState};
use crate::{core::checkpoint::file_baselines, AppError};
use serde_json::json;
use std::sync::{atomic::Ordering, Arc};

fn target(
    slot: &SessionSlot,
    message_id: &str,
) -> Result<(crate::core::session::transcript::MessageEntry, Vec<String>), String> {
    if commands::queued_input_ids(slot).contains(message_id) {
        return Err("rewind_target_ineligible".into());
    }
    slot.ctx
        .session_runtime
        .session
        .rewind_target(message_id)
        .map_err(|e| match e {
            AppError::Config(s) => s,
            other => other.to_string(),
        })
}

fn preview_files(
    slot: &SessionSlot,
    entry: &crate::core::session::transcript::MessageEntry,
    turns: &[String],
) -> Result<file_baselines::RestoreFiles, RevertReason> {
    let transcript = slot
        .ctx
        .session_runtime
        .session
        .transcript_path(&slot.session_id);
    let cwd = file_baselines::session_cwd(&transcript).ok_or(RevertReason::NoBaselines)?;
    file_baselines::preview(
        &transcript,
        turns,
        &entry.timestamp,
        &cwd,
        slot.ctx.config.checkpoint.retention_days,
        chrono::Utc::now(),
    )
}

pub(super) async fn preview(
    state: Arc<ServeState>,
    id: Option<String>,
    session_id: String,
    message_id: String,
) -> Result<(), AppError> {
    let Some(slot) =
        commands::resolve_slot_or_error(&state, id.clone(), Some(session_id.clone())).await?
    else {
        return Ok(());
    };
    let (entry, turns) = match target(&slot, &message_id) {
        Ok(t) => t,
        Err(e) => {
            return state.writer.send(OutFrame::Response(ResponseFrame::error(
                id,
                Some(session_id),
                e,
            )))
        }
    };
    let payload = match preview_files(&slot, &entry, &turns) {
        Ok(files) => PreviewRewindResponse {
            revert_available: true,
            revert_reason: None,
            revert_paths: files.paths(),
        },
        Err(reason) => PreviewRewindResponse {
            revert_available: false,
            revert_reason: Some(reason),
            revert_paths: vec![],
        },
    };
    state.writer.send(OutFrame::Response(ResponseFrame::ok(
        id,
        Some(session_id),
        Some(serde_json::to_value(payload)?),
    )))
}

struct Reservation(Arc<SessionSlot>);
impl Drop for Reservation {
    fn drop(&mut self) {
        self.0.command_job.store(false, Ordering::SeqCst);
        self.0.mark_idle();
    }
}

pub(super) async fn run(
    state: Arc<ServeState>,
    id: Option<String>,
    session_id: String,
    message_id: String,
    files: RewindFiles,
    message: RewindMessage,
) -> Result<(), AppError> {
    let Some(slot) =
        commands::resolve_slot_or_error(&state, id.clone(), Some(session_id.clone())).await?
    else {
        return Ok(());
    };
    let error = |code: String, payload| {
        state
            .writer
            .send(OutFrame::Response(ResponseFrame::error_with_payload(
                id.clone(),
                Some(session_id.clone()),
                code,
                payload,
            )))
    };
    if let Err(e) = target(&slot, &message_id) {
        return error(e, json!({}));
    }
    if slot.is_command_job_running() {
        return error("busy".into(), json!({}));
    }
    // Validate the new input before stopping the old turn or touching files/history.
    let (archival, mut provider) =
        match commands::build_turn_messages(&slot, message.text, &message.params) {
            Ok(pair) => pair,
            Err(e) => return error(e, json!({})),
        };
    if let Err(e) = super::cancel_and_wait(&state, &slot).await {
        return error(e.into(), json!({}));
    }
    if !slot.mark_busy() {
        return error("busy".into(), json!({}));
    }
    slot.command_job.store(true, Ordering::SeqCst);
    let reservation = Reservation(slot.clone());
    let (entry, turns) = match target(&slot, &message_id) {
        Ok(t) => t,
        Err(e) => return error(e, json!({})),
    };
    if files == RewindFiles::Revert {
        let restore = match preview_files(&slot, &entry, &turns) {
            Ok(p) => p,
            Err(reason) => {
                return error("revert_unavailable".into(), json!({"revertReason":reason}))
            }
        };
        if let Err((path, reason)) = restore.restore() {
            return error("revert_failed".into(), json!({"path":path,"reason":reason}));
        }
    }
    let payload = serde_json::to_value(&archival)?;
    let new_id = match slot
        .ctx
        .session_runtime
        .session
        .rewind_user_message_with_files_restored(&message_id, payload, files == RewindFiles::Revert)
    {
        Ok(id) => id,
        Err(e) => return error(e.to_string(), json!({})),
    };
    slot.ctx.session_runtime.steering_queue.lock().clear();
    slot.ctx.session_runtime.follow_up_queue.lock().clear();
    slot.ctx.session_runtime.read_file_state.clear();
    if let Err(e) = commands::rehydrate_slot_context_state(&slot) {
        return error(
            format!("rewind committed but runtime refresh failed: {e}"),
            json!({"committed":true}),
        );
    }
    // Hydrate owns the retained prefix. Replace, never append, the already durable user
    // with the validated provider rendition (not archival SVG bytes).
    provider.msg_id = Some(new_id.clone());
    if let Some(turn) = slot.turn_state.lock().as_mut() {
        if let Some(input) = turn
            .context_state
            .messages
            .iter_mut()
            .find(|m| m.msg_id.as_deref() == Some(&new_id))
        {
            *input = provider;
        }
    }
    commands::release_attachment_leases(&slot, &message.params);
    // No await between releasing the reservation and start_turn's CAS admission.
    drop(reservation);
    commands::start_turn(state, slot, id, None, commands::TurnAck::Accepted).await
}
