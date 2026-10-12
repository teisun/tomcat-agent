//! Slow session commands release the global FIFO immediately. They reserve only
//! their own slot and produce a response, not agent lifecycle events.
use super::{
    registry::SessionSlot,
    types::{OutFrame, ResponseFrame},
    ServeState,
};
use crate::AppError;
use futures_util::FutureExt;
use serde_json::Value;
use std::future::Future;
use std::sync::atomic::Ordering;
use std::sync::Arc;

pub(super) fn spawn_session_job<F>(
    state: Arc<ServeState>,
    slot: Arc<SessionSlot>,
    id: Option<String>,
    job: F,
) -> Result<(), AppError>
where
    F: Future<Output = Result<Value, String>> + Send + 'static,
{
    spawn_session_job_with_payload(state, slot, id, async move {
        job.await.map_err(|error| (error, Value::Null))
    })
}

pub(super) fn spawn_session_job_with_payload<F>(
    state: Arc<ServeState>,
    slot: Arc<SessionSlot>,
    id: Option<String>,
    job: F,
) -> Result<(), AppError>
where
    F: Future<Output = Result<Value, (String, Value)>> + Send + 'static,
{
    if !slot.mark_busy() {
        return state.writer.send(OutFrame::Response(ResponseFrame::error(
            id,
            Some(slot.session_id.clone()),
            "busy",
        )));
    }
    slot.command_job.store(true, Ordering::SeqCst);
    // Hold the same mutex through spawn+assignment and completion+idle. A job
    // that finishes immediately cannot clear before its handle is installed,
    // nor can its cleanup erase a newly admitted turn's handle.
    let mut handle_slot = slot.run_task.lock();
    let task_slot = Arc::clone(&slot);
    let handle = tokio::spawn(async move {
        let result = std::panic::AssertUnwindSafe(job).catch_unwind().await;
        {
            let mut running = task_slot.run_task.lock();
            task_slot.command_job.store(false, Ordering::SeqCst);
            task_slot.mark_idle();
            *running = None;
        }
        let response = match result {
            Ok(Ok(payload)) => {
                ResponseFrame::ok(id, Some(task_slot.session_id.clone()), Some(payload))
            }
            Ok(Err((error, payload))) => ResponseFrame::error_with_payload(
                id,
                Some(task_slot.session_id.clone()),
                error,
                payload,
            ),
            Err(_) => ResponseFrame::error(
                id,
                Some(task_slot.session_id.clone()),
                "command task panicked",
            ),
        };
        if let Err(error) = state.writer.send(OutFrame::Response(response)) {
            tracing::warn!(session_id=%task_slot.session_id, %error, "session command response failed");
        }
    });
    *handle_slot = Some(handle);
    Ok(())
}
