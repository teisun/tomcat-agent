//! User deletion is distinct from closing a runtime and from detached-session compensation.
use super::types::{OutFrame, ResponseFrame};
use super::{cleanup_session_slot, ServeState, SESSION_SHUTDOWN_TIMEOUT};
use crate::core::session::{attachments::validate_session_id, manager::DeleteSessionOutcome};
use crate::core::tools::primitive::BashTaskStatus;
use crate::{AppError, SessionManager};
use std::sync::Arc;

pub(super) async fn handle(
    state: Arc<ServeState>,
    id: Option<String>,
    session_id: String,
) -> Result<(), AppError> {
    let result = delete(&state, &session_id).await;
    let response = match result {
        Ok(outcome) => {
            ResponseFrame::ok(id, Some(session_id), Some(serde_json::to_value(outcome)?))
        }
        Err(AppError::Config(code))
            if matches!(
                code.as_str(),
                "busy"
                    | "session_in_use"
                    | "session_scope_mismatch"
                    | "unknown_session"
                    | "stop_timeout"
            ) =>
        {
            ResponseFrame::error(id, Some(session_id), code)
        }
        Err(error) => ResponseFrame::error(id, Some(session_id), error.to_string()),
    };
    state.writer.send(OutFrame::Response(response))
}

async fn delete(state: &Arc<ServeState>, id: &str) -> Result<DeleteSessionOutcome, AppError> {
    validate_session_id(id)?;
    let manager = if let Some(active) = state
        .registry
        .active_session_id()
        .and_then(|id| state.registry.get(&id))
    {
        SessionManager::new_scoped(
            crate::resolve_sessions_dir(&state.cfg)?,
            active.ctx.session_runtime.session.current_session_key(),
        )
    } else {
        super::scoped_session_manager(state)
            .ok_or_else(|| AppError::Config("unknown_session".into()))?
    };
    if manager
        .get_session_by_id(id)?
        .is_some_and(|entry| entry.session_key != manager.current_session_key())
    {
        return Err(AppError::Config("session_scope_mismatch".into()));
    }
    if let Some(slot) = state.registry.get(id) {
        if !slot.mark_busy() {
            return Err(AppError::Config("busy".into()));
        }
        let background_running = slot
            .ctx
            .session_runtime
            .bash_task_registry
            .list()
            .iter()
            .any(|task| {
                matches!(
                    task.status,
                    BashTaskStatus::Running | BashTaskStatus::DrainingOutput
                )
            });
        let unjoined = slot
            .run_task
            .lock()
            .as_ref()
            .is_some_and(|task| !task.is_finished());
        if background_running
            || unjoined
            || slot.ctx.agent_registry.has_children(id)
            || state.ask_question.has_pending_session(id)
            || state.confirmation.has_pending_session(id)
            || slot
                .resume_pending_ask_question
                .load(std::sync::atomic::Ordering::SeqCst)
        {
            slot.mark_idle();
            return Err(AppError::Config("busy".into()));
        }
        if !crate::api::chat::drain_checkpoint_record_tasks(&slot.ctx, SESSION_SHUTDOWN_TIMEOUT)
            .await
        {
            slot.mark_idle();
            return Err(AppError::Config("stop_timeout".into()));
        }
        // No agent turn is being stopped. The busy reservation only closes new admission.
        if let Err(error) = cleanup_session_slot(state, &slot, super::SlotCleanup::Delete).await {
            slot.mark_idle();
            return Err(error);
        }
    }
    let trail = crate::resolve_agent_trail_dir(&state.cfg)?;
    let id = id.to_owned();
    // The source boundary excludes other processes and revalidates after acquiring occupancy.
    tokio::task::spawn_blocking(move || manager.begin_delete_session(&id)?.commit(Some(&trail)))
        .await
        .map_err(|error| AppError::Internal(error.to_string()))?
}
