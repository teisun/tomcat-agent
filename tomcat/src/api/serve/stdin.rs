use std::sync::Arc;

use tokio::io::AsyncBufReadExt;

use crate::AppError;

use super::commands::handle_command;
use super::control::handle_stdin_eof;
use super::ndjson::parse_command_line;
use super::types::{OutFrame, ResponseFrame};
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
                state.writer.send(OutFrame::Response(ResponseFrame::error(
                    None,
                    None,
                    error.to_string(),
                )))?;
                continue;
            }
        };
        handle_command(Arc::clone(&state), command).await?;
    }
    handle_stdin_eof(state).await
}
