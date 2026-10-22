use std::sync::Arc;

use tokio::io::AsyncBufReadExt;

use crate::AppError;

use super::commands::handle_command;
use super::ndjson::{extract_response_refs, parse_command_line};
use super::types::{OutFrame, ResponseFrame, ServeCommand};
use super::ServeState;

/// In-memory queue metadata, never part of either wire protocol. A prompt's
/// token is created on receipt, so an interrupt cannot be lost before dequeue.
#[derive(Debug)]
pub(crate) struct QueuedCommand {
    pub command: ServeCommand,
    pub turn_token: Option<tokio_util::sync::CancellationToken>,
    pub session_mcp: Option<crate::core::connector::mcp::manager::SessionMcpConfig>,
}

impl From<ServeCommand> for QueuedCommand {
    fn from(command: ServeCommand) -> Self {
        Self {
            command,
            turn_token: None,
            session_mcp: None,
        }
    }
}

impl QueuedCommand {
    pub(crate) fn with_session_mcp(
        mut self,
        config: crate::core::connector::mcp::manager::SessionMcpConfig,
    ) -> Self {
        self.session_mcp = Some(config);
        self
    }
}

pub(crate) async fn run_stdio_loop(state: Arc<ServeState>) -> Result<(), AppError> {
    let reader = tokio::io::BufReader::new(
        crate::infra::platform::stdin::protocol_input().map_err(AppError::Io)?,
    );
    run_connection_loop(state, reader).await
}

pub(super) async fn run_connection_loop<R: tokio::io::AsyncBufRead + Unpin>(
    state: Arc<ServeState>,
    reader: R,
) -> Result<(), AppError> {
    let commands = futures_util::stream::try_unfold(
        (reader, Arc::clone(&state)),
        |(mut reader, state)| async move {
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).await? == 0 {
                    return Ok::<_, AppError>(None);
                }
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                match parse_command_line(trimmed) {
                    Ok(command) => return Ok(Some((command, (reader, state)))),
                    Err(error) => {
                        let (id, session_id) = extract_response_refs(trimmed);
                        state.writer.send(OutFrame::Response(ResponseFrame::error(
                            id,
                            session_id,
                            render_command_error(&error),
                        )))?;
                    }
                }
            }
        },
    );
    run_command_stream(state, Box::pin(commands)).await
}

/// Native NDJSON and ACP feed the same FIFO, interrupt and disconnect workflow.
/// ACP supplies typed commands; it never serializes private internal operations.
pub(crate) async fn run_command_stream<S, C>(
    state: Arc<ServeState>,
    mut commands: S,
) -> Result<(), AppError>
where
    S: futures_util::Stream<Item = Result<C, AppError>> + Unpin,
    C: Into<QueuedCommand> + Send,
{
    use futures_util::StreamExt;
    // Normal commands retain FIFO execution because many mutate session state. Interrupt is a
    // control-plane signal: keeping it behind a transcript read or model bootstrap would make
    // "Stop" appear broken even though cancellation itself is healthy.
    let (normal_tx, mut normal_rx) = tokio::sync::mpsc::unbounded_channel();
    let normal_state = Arc::clone(&state);
    let normal_worker = tokio::spawn(async move {
        while let Some(command) = normal_rx.recv().await {
            if normal_state.shared && normal_state.is_closed() {
                break;
            }
            dispatch_command(Arc::clone(&normal_state), command).await?;
        }
        Ok::<(), AppError>(())
    });

    let input_result: Result<(), AppError> = async {
        loop {
            let command = tokio::select! {
                _ = state.lifetime.cancelled(), if state.shared => break,
                _ = state.writer.closed(), if state.shared => break,
                command = commands.next() => command,
            };
            let Some(command) = command else {
                break;
            };
            route_parsed_command(Arc::clone(&state), &normal_tx, command?).await?;
        }
        Ok(())
    }
    .await;
    if state.shared {
        state.begin_disconnect();
    }
    drop(normal_tx);
    let (worker_result, disconnect_result) = tokio::join!(normal_worker, async {
        if state.shared {
            state.disconnect().await
        } else {
            Ok(())
        }
    });
    input_result?;
    worker_result.map_err(|error| {
        AppError::Config(crate::infra::i18n::tr(
            "serve.workerPanicked",
            &[("detail", &error.to_string())],
        ))
    })??;
    disconnect_result
}

/// Keep stateful work FIFO while letting the cancellation control plane preempt that backlog.
pub(crate) async fn route_parsed_command(
    state: Arc<ServeState>,
    normal_tx: &tokio::sync::mpsc::UnboundedSender<QueuedCommand>,
    command: impl Into<QueuedCommand>,
) -> Result<(), AppError> {
    let command = command.into();
    if matches!(&command.command, ServeCommand::Interrupt { .. }) {
        dispatch_command(state, command).await
    } else {
        normal_tx
            .send(command)
            .map_err(|_| AppError::Config(crate::infra::i18n::tr("serve.workerStopped", &[])))
    }
}

pub(crate) async fn dispatch_command(
    state: Arc<ServeState>,
    command: impl Into<QueuedCommand>,
) -> Result<(), AppError> {
    let input = command.into();
    let command_id = input.command.command_id().map(ToOwned::to_owned);
    let session_id = input.command.session_id().map(ToOwned::to_owned);
    let wire_type = input.command.wire_type();
    if let Err(error) = handle_command(Arc::clone(&state), input).await {
        tracing::warn!(
            command = wire_type,
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
