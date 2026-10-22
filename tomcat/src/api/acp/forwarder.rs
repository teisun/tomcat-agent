//! A byte forwarder, not another agent runtime. Closing stdin requests cleanup
//! over the private local hop; stdout contains only the service's ACP frames.

use super::{CLOSE, CLOSED, CLOSE_FAILED};
use crate::{AppConfig, AppError};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio_util::sync::CancellationToken;

#[cfg(test)]
#[path = "tests/forwarder_test.rs"]
mod tests;

pub(crate) fn run(
    cfg: &AppConfig,
    environment: crate::infra::platform::EnvironmentSnapshot,
    skills_dir: Option<std::path::PathBuf>,
) -> Result<(), AppError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let result = runtime.block_on(async {
        let stream = crate::api::serve::service::connect_acp(cfg, environment, skills_dir).await?;
        let input = crate::infra::platform::stdin::protocol_input()?;
        forward(stream, input, tokio::io::stdout()).await
    });
    // Tokio stdout may be doing a blocking OS pipe write if the host stopped
    // reading. Do not turn the explicit close deadline into an infinite
    // runtime-drop wait. All actual agent work lives in the service, not here.
    runtime.shutdown_background();
    result
}

async fn forward<S, R, W>(stream: S, mut input: R, mut output: W) -> Result<(), AppError>
where
    S: AsyncRead + AsyncWrite + Unpin,
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let (read, mut write) = tokio::io::split(stream);
    let closing = CancellationToken::new();
    let sending = async {
        let mut bytes = [0; 16 * 1024];
        let mut last = None;
        loop {
            let count = input.read(&mut bytes).await?;
            if count == 0 {
                break;
            }
            write.write_all(&bytes[..count]).await?;
            last = Some(bytes[count - 1]);
        }
        closing.cancel();
        // Finish a final unterminated host line before the private close record.
        if last.is_some_and(|byte| byte != b'\n') {
            write.write_all(b"\n").await?;
        }
        write.write_all(CLOSE.as_bytes()).await?;
        write.write_all(b"\n").await?;
        write.flush().await?;
        Ok::<_, AppError>(())
    };
    let receiving = async {
        let mut reader = BufReader::new(read);
        let mut line = Vec::new();
        loop {
            line.clear();
            if reader.read_until(b'\n', &mut line).await? == 0 {
                return Err(AppError::Config(
                    "ACP service connection closed before cleanup acknowledgement".into(),
                ));
            }
            let record = line.strip_suffix(b"\n").unwrap_or(&line);
            if record == CLOSED.as_bytes() {
                return Ok(());
            }
            if record == CLOSE_FAILED.as_bytes() {
                return Err(AppError::Config(
                    "ACP task cleanup failed; see shared service logs".into(),
                ));
            }
            output.write_all(&line).await?;
            output.flush().await?;
        }
    };
    let exchange = async {
        tokio::try_join!(sending, receiving)?;
        Ok(())
    };
    tokio::pin!(exchange);
    tokio::select! {
        result = &mut exchange => result,
        _ = closing.cancelled() => {
            // Leave room for the 30s checkpoint flush and actual child/MCP exit.
            tokio::time::timeout(Duration::from_secs(60), &mut exchange).await
                .map_err(|_| AppError::Config("ACP cleanup acknowledgement timed out after 60s; task cleanup is not confirmed".into()))?
        }
    }
}
