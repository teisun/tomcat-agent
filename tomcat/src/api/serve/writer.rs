use std::collections::{HashMap, VecDeque};
use std::pin::Pin;
use std::time::{Duration, Instant};

use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;

use crate::AppError;

use super::ndjson::ndjson_safe_stringify;
use super::types::OutFrame;

type BoxWriter = Pin<Box<dyn AsyncWrite + Send>>;

#[derive(Debug, Clone, Copy)]
pub struct WriterConfig {
    pub delta_coalesce_ms: u32,
    pub max_buffered_frames: usize,
}

impl From<&crate::ServeConfig> for WriterConfig {
    fn from(value: &crate::ServeConfig) -> Self {
        Self {
            delta_coalesce_ms: value.delta_coalesce_ms,
            max_buffered_frames: value.max_buffered_frames,
        }
    }
}

#[derive(Clone)]
pub struct WriterHandle {
    tx: mpsc::UnboundedSender<OutFrame>,
}

impl WriterHandle {
    pub fn send(&self, frame: OutFrame) -> Result<(), AppError> {
        self.tx
            .send(frame)
            .map_err(|_| AppError::Config("serve writer channel closed".to_string()))
    }
}

pub fn spawn_stdout_writer(config: WriterConfig) -> WriterHandle {
    spawn_writer(Box::pin(tokio::io::stdout()), config)
}

pub fn spawn_writer(writer: BoxWriter, config: WriterConfig) -> WriterHandle {
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        if let Err(error) = writer_task(writer, rx, config).await {
            tracing::error!(error = %error, "serve writer task failed");
        }
    });
    WriterHandle { tx }
}

struct BufferedFrame {
    frame: OutFrame,
    queued_at: Instant,
}

struct SessionBuffer {
    frames: VecDeque<BufferedFrame>,
    warned_slow_consumer: bool,
}

impl SessionBuffer {
    fn new() -> Self {
        Self {
            frames: VecDeque::new(),
            warned_slow_consumer: false,
        }
    }
}

async fn writer_task(
    mut writer: BoxWriter,
    mut rx: mpsc::UnboundedReceiver<OutFrame>,
    config: WriterConfig,
) -> Result<(), AppError> {
    let mut buffers: HashMap<String, SessionBuffer> = HashMap::new();
    let mut session_order: VecDeque<String> = VecDeque::new();
    let mut global_frames: VecDeque<BufferedFrame> = VecDeque::new();

    while let Some(frame) = rx.recv().await {
        enqueue_frame(
            frame,
            &mut buffers,
            &mut session_order,
            &mut global_frames,
            config,
        );
        while let Ok(frame) = rx.try_recv() {
            enqueue_frame(
                frame,
                &mut buffers,
                &mut session_order,
                &mut global_frames,
                config,
            );
        }
        drain_pending(
            &mut writer,
            &mut buffers,
            &mut session_order,
            &mut global_frames,
        )
        .await?;
    }

    drain_pending(
        &mut writer,
        &mut buffers,
        &mut session_order,
        &mut global_frames,
    )
    .await?;
    Ok(())
}

fn enqueue_frame(
    frame: OutFrame,
    buffers: &mut HashMap<String, SessionBuffer>,
    session_order: &mut VecDeque<String>,
    global_frames: &mut VecDeque<BufferedFrame>,
    config: WriterConfig,
) {
    let Some(session_id) = frame.session_id().map(ToOwned::to_owned) else {
        global_frames.push_back(BufferedFrame {
            frame,
            queued_at: Instant::now(),
        });
        return;
    };

    let buffer = buffers
        .entry(session_id.clone())
        .or_insert_with(SessionBuffer::new);
    let was_empty = buffer.frames.is_empty();
    if try_coalesce_tail(buffer, &frame, config.delta_coalesce_ms) {
        if was_empty {
            session_order.push_back(session_id);
        }
        return;
    }

    let limit = config.max_buffered_frames.max(1);
    if buffer.frames.len() >= limit && frame.is_message_delta() {
        if !buffer.warned_slow_consumer {
            buffer.warned_slow_consumer = true;
            buffer.frames.push_back(BufferedFrame {
                frame: OutFrame::Event(serde_json::json!({
                    "type": "llm_notice",
                    "sessionId": session_id,
                    "finishReason": "backpressure",
                    "message": "serve writer dropped message deltas under backpressure"
                })),
                queued_at: Instant::now(),
            });
        }
    } else {
        if frame.is_message_delta() && buffer.frames.len() + 1 < limit {
            buffer.warned_slow_consumer = false;
        }
        buffer.frames.push_back(BufferedFrame {
            frame,
            queued_at: Instant::now(),
        });
    }

    if was_empty && !buffer.frames.is_empty() {
        session_order.push_back(session_id);
    }
}

fn try_coalesce_tail(buffer: &mut SessionBuffer, next: &OutFrame, window_ms: u32) -> bool {
    if window_ms == 0 || !next.is_message_delta() {
        return false;
    }
    let Some(tail) = buffer.frames.back_mut() else {
        return false;
    };
    if !tail.frame.is_message_delta() {
        return false;
    }
    if tail.queued_at.elapsed() > Duration::from_millis(window_ms as u64) {
        return false;
    }
    coalesce_message_update(&mut tail.frame, next)
}

fn coalesce_message_update(target: &mut OutFrame, next: &OutFrame) -> bool {
    let OutFrame::Event(target_value) = target else {
        return false;
    };
    let OutFrame::Event(next_value) = next else {
        return false;
    };
    let Some(target_event) = target_value
        .get_mut("assistantMessageEvent")
        .and_then(serde_json::Value::as_object_mut)
    else {
        return false;
    };
    let Some(next_event) = next_value
        .get("assistantMessageEvent")
        .and_then(serde_json::Value::as_object)
    else {
        return false;
    };
    let target_kind = target_event.get("kind").and_then(serde_json::Value::as_str);
    let next_kind = next_event.get("kind").and_then(serde_json::Value::as_str);
    if target_kind != next_kind {
        return false;
    }
    if target_event.get("source") != next_event.get("source")
        || target_event.get("signature") != next_event.get("signature")
    {
        return false;
    }
    let Some(next_delta) = next_event.get("delta").and_then(serde_json::Value::as_str) else {
        return false;
    };
    let Some(target_delta) = target_event.get_mut("delta") else {
        return false;
    };
    let Some(existing) = target_delta.as_str() else {
        return false;
    };
    *target_delta = serde_json::Value::String(format!("{existing}{next_delta}"));
    true
}

async fn drain_pending(
    writer: &mut BoxWriter,
    buffers: &mut HashMap<String, SessionBuffer>,
    session_order: &mut VecDeque<String>,
    global_frames: &mut VecDeque<BufferedFrame>,
) -> Result<(), AppError> {
    while !global_frames.is_empty() || !session_order.is_empty() {
        if let Some(frame) = global_frames.pop_front() {
            write_frame(writer, &frame.frame).await?;
        }

        let Some(session_id) = session_order.pop_front() else {
            continue;
        };
        let Some(buffer) = buffers.get_mut(&session_id) else {
            continue;
        };
        if let Some(frame) = buffer.frames.pop_front() {
            write_frame(writer, &frame.frame).await?;
        }
        if buffer.frames.is_empty() {
            buffers.remove(&session_id);
        } else {
            session_order.push_back(session_id);
        }
    }
    Ok(())
}

async fn write_frame(writer: &mut BoxWriter, frame: &OutFrame) -> Result<(), AppError> {
    let rendered = ndjson_safe_stringify(frame)?;
    writer
        .write_all(rendered.as_bytes())
        .await
        .map_err(AppError::Io)?;
    writer.write_all(b"\n").await.map_err(AppError::Io)?;
    writer.flush().await.map_err(AppError::Io)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::io::AsyncWrite;

    #[derive(Clone, Default)]
    struct SharedBuffer(Arc<parking_lot::Mutex<Vec<u8>>>);

    struct VecWriter {
        inner: SharedBuffer,
    }

    impl AsyncWrite for VecWriter {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            buf: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            let inner = self.get_mut().inner.clone();
            let mut guard = inner.0.lock();
            guard.extend_from_slice(buf);
            std::task::Poll::Ready(Ok(buf.len()))
        }

        fn poll_flush(
            self: Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            let _ = self;
            std::task::Poll::Ready(Ok(()))
        }

        fn poll_shutdown(
            self: Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            let _ = self;
            std::task::Poll::Ready(Ok(()))
        }
    }

    fn event(session: &str, ty: &str, delta: Option<&str>) -> OutFrame {
        let payload = if let Some(delta) = delta {
            serde_json::json!({
                "type": ty,
                "sessionId": session,
                "assistantMessageEvent": {
                    "kind": "content_delta",
                    "delta": delta
                }
            })
        } else {
            serde_json::json!({
                "type": ty,
                "sessionId": session
            })
        };
        OutFrame::Event(payload)
    }

    #[tokio::test]
    async fn serve_writer_single_drain_orders_frames() {
        let shared = SharedBuffer::default();
        let writer = VecWriter {
            inner: shared.clone(),
        };
        let handle = spawn_writer(
            Box::pin(writer),
            WriterConfig {
                delta_coalesce_ms: 0,
                max_buffered_frames: 8,
            },
        );
        handle
            .send(OutFrame::Response(super::super::types::ResponseFrame::ok(
                Some("1".to_string()),
                None,
                None,
            )))
            .unwrap();
        handle
            .send(OutFrame::Response(super::super::types::ResponseFrame::ok(
                Some("2".to_string()),
                None,
                None,
            )))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        let bytes = shared.0.lock().clone();
        let rendered = String::from_utf8(bytes).unwrap();
        let lines: Vec<&str> = rendered.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("\"id\":\"1\""));
        assert!(lines[1].contains("\"id\":\"2\""));
    }

    #[tokio::test]
    async fn serve_writer_coalesces_deltas_under_pressure() {
        let shared = SharedBuffer::default();
        let writer = VecWriter {
            inner: shared.clone(),
        };
        let handle = spawn_writer(
            Box::pin(writer),
            WriterConfig {
                delta_coalesce_ms: 100,
                max_buffered_frames: 8,
            },
        );
        handle
            .send(event("s1", "message_update", Some("he")))
            .unwrap();
        handle
            .send(event("s1", "message_update", Some("llo")))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        let bytes = shared.0.lock().clone();
        let rendered = String::from_utf8(bytes).unwrap();
        assert!(rendered.contains("\"delta\":\"hello\""));
        assert_eq!(rendered.lines().count(), 1);
    }

    #[tokio::test]
    async fn serve_writer_never_drops_lifecycle() {
        let shared = SharedBuffer::default();
        let writer = VecWriter {
            inner: shared.clone(),
        };
        let handle = spawn_writer(
            Box::pin(writer),
            WriterConfig {
                delta_coalesce_ms: 0,
                max_buffered_frames: 1,
            },
        );
        handle
            .send(event("s1", "message_update", Some("a")))
            .unwrap();
        handle
            .send(event("s1", "message_update", Some("b")))
            .unwrap();
        handle.send(event("s1", "agent_end", None)).unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        let bytes = shared.0.lock().clone();
        let rendered = String::from_utf8(bytes).unwrap();
        assert!(rendered.contains("\"type\":\"agent_end\""));
    }

    #[test]
    fn serve_writer_backpressure_notice_emitted_once() {
        let mut buffers: HashMap<String, SessionBuffer> = HashMap::new();
        let mut session_order = VecDeque::new();
        let mut global_frames = VecDeque::new();
        let config = WriterConfig {
            delta_coalesce_ms: 0,
            max_buffered_frames: 1,
        };

        enqueue_frame(
            event("s1", "message_update", Some("a")),
            &mut buffers,
            &mut session_order,
            &mut global_frames,
            config,
        );
        enqueue_frame(
            event("s1", "message_update", Some("b")),
            &mut buffers,
            &mut session_order,
            &mut global_frames,
            config,
        );
        enqueue_frame(
            event("s1", "message_update", Some("c")),
            &mut buffers,
            &mut session_order,
            &mut global_frames,
            config,
        );

        let buffer = buffers.get("s1").expect("session buffer");
        let notices = buffer
            .frames
            .iter()
            .filter(|frame| {
                frame.frame.session_id() == Some("s1")
                    && matches!(
                        &frame.frame,
                        OutFrame::Event(value)
                            if value.get("type").and_then(serde_json::Value::as_str)
                                == Some("llm_notice")
                    )
            })
            .count();
        assert_eq!(notices, 1);
    }

    #[tokio::test]
    async fn serve_writer_round_robins_across_sessions() {
        let shared = SharedBuffer::default();
        let writer = VecWriter {
            inner: shared.clone(),
        };
        let handle = spawn_writer(
            Box::pin(writer),
            WriterConfig {
                delta_coalesce_ms: 0,
                max_buffered_frames: 8,
            },
        );
        handle
            .send(event("s1", "message_update", Some("a1")))
            .unwrap();
        handle
            .send(event("s1", "message_update", Some("a2")))
            .unwrap();
        handle
            .send(event("s2", "message_update", Some("b1")))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        let bytes = shared.0.lock().clone();
        let rendered = String::from_utf8(bytes).unwrap();
        let lines: Vec<&str> = rendered.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].contains("\"sessionId\":\"s1\""));
        assert!(lines[1].contains("\"sessionId\":\"s2\""));
        assert!(lines[2].contains("\"sessionId\":\"s1\""));
    }
}
