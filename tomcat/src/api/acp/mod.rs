//! ACP lives inside the shared service. The CLI process only forwards the private
//! local transport; native commands, ownership, cancellation and writer queues are reused.

mod adapter;
mod events;
mod forwarder;
mod launch;
pub use launch::AcpLaunchOptions;
mod permissions;

pub(crate) use forwarder::run;

use std::io;
use std::pin::Pin;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::task::{Context, Poll};

use agent_client_protocol::{Agent, ByteStreams};
use futures_util::StreamExt;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use crate::api::serve::{
    self,
    types::OutFrame,
    writer::{WriterConfig, WriterHandle},
    ServeRuntime, ServeState,
};
use crate::AppError;
use adapter::{Adapter, Replies};

// Private local-transport records, not ACP methods. They cannot be confused with
// a JSON-RPC frame and are removed by the forwarder, never written to host stdout.
pub(super) const CLOSE: &str = "\0tomcat.close";
pub(super) const CLOSED: &str = "\0tomcat.closed";
pub(super) const CLOSE_FAILED: &str = "\0tomcat.close_failed";

pub(crate) async fn run_connection<R, W>(
    runtime: Arc<ServeRuntime>,
    reader: R,
    output: W,
    environment: crate::infra::platform::EnvironmentSnapshot,
    skills_dir: Option<std::path::PathBuf>,
    options: Option<AcpLaunchOptions>,
) -> Result<(), AppError>
where
    R: AsyncBufRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (input, commands) = tokio::sync::mpsc::unbounded_channel();
    let replies = Arc::new(Replies::new(input.clone()));
    let route = replies.clone();
    let (mode_changes, mut changed_modes) = tokio::sync::mpsc::unbounded_channel::<String>();
    let writer = serve::writer::spawn_writer_with_encoder(
        Box::pin(output),
        // Hosts persist these deltas as the result; dropped text cannot be recovered.
        WriterConfig {
            max_buffered_frames: usize::MAX,
            ..WriterConfig::from(&runtime.cfg.serve)
        },
        move |frame| match frame {
            OutFrame::Acp { line, .. } => Ok(Some(line)),
            OutFrame::Response(response) => {
                route.response(response);
                Ok(None)
            }
            OutFrame::Event(event) => {
                route.event(&event);
                let Some(session_id) = event["sessionId"].as_str() else {
                    return Ok(None);
                };
                if event["type"] == "session.agent_mode.changed"
                    && route.prompts.lock().contains_key(session_id)
                {
                    let _ = mode_changes.send(session_id.to_owned());
                }
                events::event_update(&event)
                    .map(|update| events::notification(session_id, update))
                    .transpose()
            }
            OutFrame::Control(frame) => route.permissions.forward(frame),
        },
    );
    let state = ServeState::connect_with_source(
        runtime,
        writer.clone(),
        Some(crate::core::session::SessionOrigin::Acp { client_name: None }),
        environment,
        skills_dir,
        options,
    );
    let _cancel_on_drop = state.lifetime.clone().drop_guard();
    let native = tokio::spawn(serve::stdin::run_command_stream(
        state.clone(),
        tokio_stream::wrappers::UnboundedReceiverStream::new(commands).map(Ok),
    ));
    let goodbye = Arc::new(AtomicBool::new(false));
    let closing = goodbye.clone();
    let lifetime = state.lifetime.clone();
    let incoming = futures_util::stream::try_unfold(
        (reader, closing, lifetime),
        |(mut reader, closing, lifetime)| async move {
            let mut bytes = Vec::new();
            if reader.read_until(b'\n', &mut bytes).await? == 0 {
                return Ok::<_, io::Error>(None);
            }
            if bytes.strip_suffix(b"\n").unwrap_or(&bytes) == CLOSE.as_bytes() {
                closing.store(true, Ordering::Release);
                lifetime.cancel();
                return Ok(None);
            }
            Ok(Some((
                bytes::Bytes::from(bytes),
                (reader, closing, lifetime),
            )))
        },
    );
    let incoming = tokio_util::io::StreamReader::new(Box::pin(incoming)).compat();
    let transport = ByteStreams::new(
        SdkOutput {
            writer: writer.clone(),
            pending: Vec::new(),
        }
        .compat_write(),
        incoming,
    );
    let adapter = Adapter {
        state: state.clone(),
        input,
        replies: replies.clone(),
    };
    // The encoder must never wait for replies delivered by that same encoder.
    let notifying = adapter.clone();
    let notifications = tokio::spawn(async move {
        loop {
            let result: Result<(), AppError> = tokio::select! {
                _ = notifying.state.lifetime.cancelled() => break,
                session_id = changed_modes.recv() => {
                    let Some(session_id) = session_id else { break };
                    async {
                        let options = notifying.config_options(&session_id).await
                            .map_err(|error| AppError::Config(error.to_string()))?;
                        let update = serde_json::from_value(serde_json::json!({
                            "sessionUpdate": "config_option_update", "configOptions": options,
                        }))?;
                        let line = events::notification(&session_id, update)?;
                        notifying.state.writer.send(OutFrame::Acp { session_id: Some(session_id), line })
                    }.await
                }
            };
            if let Err(error) = result {
                tracing::warn!(%error, "ACP configuration notification failed");
            }
        }
    });
    let close_state = state.clone();
    let close_replies = replies.clone();
    let outcome = {
        let sdk = Agent
            .builder()
            .name("tomcat")
            .with_handler(adapter)
            .on_close(move |_| async move {
                close_state.lifetime.cancel();
                close_replies.close();
                Ok(())
            })
            .connect_to(transport);
        tokio::pin!(sdk);
        tokio::select! {
            result = &mut sdk => result.map_err(|e| AppError::Config(e.to_string())),
            _ = state.lifetime.cancelled() => Ok(()),
            _ = writer.closed() => Err(AppError::Config("ACP output connection closed".into())),
        }
    };
    state.lifetime.cancel();
    replies.close();
    notifications.abort();
    let _ = notifications.await;
    let cleanup = native
        .await
        .map_err(|e| AppError::Config(format!("ACP command worker failed: {e}")))
        .and_then(|result| result);
    // This acknowledgement is sent only after the existing disconnect workflow
    // has waited for actual turns, subprocesses and pending commands to stop.
    let acknowledged = if goodbye.load(Ordering::Acquire) {
        writer.send(OutFrame::Acp {
            session_id: None,
            line: if cleanup.is_ok() {
                CLOSED
            } else {
                CLOSE_FAILED
            }
            .into(),
        })
    } else {
        Ok(())
    };
    writer.close();
    cleanup?;
    acknowledged?;
    outcome
}

// SDK-generated RPC responses/requests use the same single writer as translated
// events. Retain the SDK's bytes (including batches), not another JSON-RPC codec.
struct SdkOutput {
    writer: WriterHandle,
    pending: Vec<u8>,
}

impl AsyncWrite for SdkOutput {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.pending.extend_from_slice(bytes);
        while let Some(end) = self.pending.iter().position(|byte| *byte == b'\n') {
            let mut line: Vec<_> = self.pending.drain(..=end).collect();
            line.pop();
            let line = String::from_utf8(line)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            self.writer
                .send(OutFrame::Acp {
                    session_id: None,
                    line,
                })
                .map_err(|e| io::Error::new(io::ErrorKind::BrokenPipe, e.to_string()))?;
        }
        Poll::Ready(Ok(bytes.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
