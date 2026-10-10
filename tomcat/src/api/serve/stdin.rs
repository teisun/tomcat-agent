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
    // Normal commands retain FIFO execution because many mutate session state. Interrupt is a
    // control-plane signal: keeping it behind a transcript read or model bootstrap would make
    // "Stop" appear broken even though cancellation itself is healthy.
    let (normal_tx, mut normal_rx) = tokio::sync::mpsc::unbounded_channel();
    let normal_state = Arc::clone(&state);
    let normal_worker = tokio::spawn(async move {
        while let Some(command) = normal_rx.recv().await {
            dispatch_command(Arc::clone(&normal_state), command).await?;
        }
        Ok::<(), AppError>(())
    });

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
        route_parsed_command(Arc::clone(&state), &normal_tx, command).await?;
    }
    drop(normal_tx);
    normal_worker.await.map_err(|error| {
        AppError::Config(crate::infra::i18n::tr(
            "serve.workerPanicked",
            &[("detail", &error.to_string())],
        ))
    })?
}

/// Keep stateful work FIFO while letting the cancellation control plane preempt that backlog.
pub(crate) async fn route_parsed_command(
    state: Arc<ServeState>,
    normal_tx: &tokio::sync::mpsc::UnboundedSender<ServeCommand>,
    command: ServeCommand,
) -> Result<(), AppError> {
    if matches!(&command, ServeCommand::Interrupt { .. }) {
        dispatch_command(state, command).await
    } else {
        normal_tx
            .send(command)
            .map_err(|_| AppError::Config(crate::infra::i18n::tr("serve.workerStopped", &[])))
    }
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
