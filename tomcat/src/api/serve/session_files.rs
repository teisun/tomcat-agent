//! File-only operations; reads never reserve an agent turn.
use super::{
    commands::resolve_slot_or_error,
    session_job::spawn_session_job_with_payload,
    types::{OutFrame, ResponseFrame},
    ServeState,
};
use crate::{core::checkpoint::session_files, AppError};
use std::sync::Arc;

fn code(error: AppError) -> String {
    match error {
        AppError::Config(code) => code,
        e => e.to_string(),
    }
}

pub(super) async fn read(
    state: Arc<ServeState>,
    id: Option<String>,
    session_id: String,
    file: Option<(String, String)>,
) -> Result<(), AppError> {
    let Some(slot) = resolve_slot_or_error(&state, id.clone(), Some(session_id.clone())).await?
    else {
        return Ok(());
    };
    let transcript = slot
        .ctx
        .session_runtime
        .session
        .transcript_path(&session_id);
    tokio::spawn(async move {
        let query_session = session_id.clone();
        let result = tokio::task::spawn_blocking(move || match file {
            Some((source, path)) => {
                session_files::baseline(&transcript, &query_session, &source, &path)
                    .and_then(|r| serde_json::to_value(r).map_err(AppError::from))
            }
            None => session_files::list(&transcript, &query_session)
                .and_then(|r| serde_json::to_value(r).map_err(AppError::from)),
        })
        .await;
        let response = match result {
            Ok(Ok(payload)) => ResponseFrame::ok(id, Some(session_id), Some(payload)),
            Ok(Err(error)) => ResponseFrame::error(id, Some(session_id), code(error)),
            Err(error) => ResponseFrame::error(id, Some(session_id), error.to_string()),
        };
        let _ = state.writer.send(OutFrame::Response(response));
    });
    Ok(())
}

pub(super) async fn keep(
    state: Arc<ServeState>,
    id: Option<String>,
    session_id: String,
    source: String,
) -> Result<(), AppError> {
    let Some(slot) = resolve_slot_or_error(&state, id.clone(), Some(session_id.clone())).await?
    else {
        return Ok(());
    };
    let transcript = slot
        .ctx
        .session_runtime
        .session
        .transcript_path(&session_id);
    spawn_session_job_with_payload(state, slot, id, async move {
        let result = session_files::keep(&transcript, &session_id, &source)
            .map_err(|e| (code(e), serde_json::Value::Null))?;
        serde_json::to_value(result).map_err(|e| (e.to_string(), serde_json::Value::Null))
    })
}

pub(super) async fn restore(
    state: Arc<ServeState>,
    id: Option<String>,
    session_id: String,
    source: String,
    paths: Vec<String>,
) -> Result<(), AppError> {
    let Some(slot) = resolve_slot_or_error(&state, id.clone(), Some(session_id.clone())).await?
    else {
        return Ok(());
    };
    let transcript = slot
        .ctx
        .session_runtime
        .session
        .transcript_path(&session_id);
    spawn_session_job_with_payload(state, slot, id, async move {
        // This synchronous commit runs to completion in the reserved task. It is
        // deliberately not a detached spawn_blocking writer whose outer job could
        // be cancelled while the file writes continued without the reservation.
        let result = session_files::restore(&transcript, &session_id, &source, &paths)?;
        serde_json::to_value(result).map_err(|e| (e.to_string(), serde_json::Value::Null))
    })
}
