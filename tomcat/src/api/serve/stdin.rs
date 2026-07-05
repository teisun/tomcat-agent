use std::sync::Arc;

use tokio::io::AsyncBufReadExt;

use crate::AppError;

use super::commands::handle_command;
use super::ndjson::{extract_response_refs, parse_command_line};
use super::types::{OutFrame, ResponseFrame, ServeCommand};
use super::ServeState;

pub(crate) async fn run_stdio_loop(state: Arc<ServeState>) -> Result<(), AppError> {
    let stdin = tokio::io::stdin();
    let reader = tokio::io::BufReader::new(stdin);
    let mut lines = reader.lines();
    while let Some(line) = lines.next_line().await.map_err(AppError::Io)? {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let command = match parse_command_line(trimmed) {
            Ok(command) => command,
            Err(error) => {
                let message = render_command_error(&error);
                let (id, session_id) = extract_response_refs(trimmed);
                state.writer.send(OutFrame::Response(ResponseFrame::error(
                    id, session_id, message,
                )))?;
                continue;
            }
        };
        dispatch_command(Arc::clone(&state), command).await?;
    }
    Ok(())
}

pub(crate) async fn dispatch_command(
    state: Arc<ServeState>,
    command: ServeCommand,
) -> Result<(), AppError> {
    let command_id = command.command_id().map(ToOwned::to_owned);
    let session_id = command.session_id().map(ToOwned::to_owned);
    if let Err(error) = handle_command(Arc::clone(&state), command.clone()).await {
        tracing::warn!(
            command = command.wire_type(),
            error = %error,
            "serve command failed; returning error frame and keeping stdio loop alive"
        );
        state.writer.send(OutFrame::Response(ResponseFrame::error(
            command_id,
            session_id,
            render_command_error(&error),
        )))?;
    }
    Ok(())
}

fn render_command_error(error: &AppError) -> String {
    match error {
        AppError::Config(message) => message.clone(),
        _ => error.to_string(),
    }
}
