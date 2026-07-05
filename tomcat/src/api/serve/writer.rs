//! `serve` 的唯一 stdout writer。
//!
//! 所有命令响应、控制帧和事件下行都必须先进入这里，再由单写者任务序列化成 NDJSON。

use std::collections::{HashMap, VecDeque};
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::Notify;

use crate::AppError;

use super::ndjson::ndjson_safe_stringify;
use super::types::OutFrame;

type BoxWriter = Pin<Box<dyn AsyncWrite + Send>>;

/// writer 的背压与合并参数。
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

/// 供其他模块写入 stdout 队列的轻量句柄。
#[derive(Clone)]
pub struct WriterHandle {
    shared: Arc<WriterShared>,
}

impl WriterHandle {
    /// 将一帧写入 writer 队列。
    pub fn send(&self, frame: OutFrame) -> Result<(), AppError> {
        self.shared.enqueue(frame);
        Ok(())
    }
}

/// 以真实 stdout 作为下行目标创建 writer。
pub fn spawn_stdout_writer(config: WriterConfig) -> WriterHandle {
    spawn_writer(Box::pin(tokio::io::stdout()), config)
}

/// 以任意 `AsyncWrite` 创建 writer，便于测试和未来扩展传输层。
pub fn spawn_writer(writer: BoxWriter, config: WriterConfig) -> WriterHandle {
    let shared = Arc::new(WriterShared::new(config));
    let shared_for_task = Arc::clone(&shared);
    tokio::spawn(async move {
        if let Err(error) = writer_task(writer, shared_for_task).await {
            tracing::error!(error = %error, "serve writer task failed");
        }
    });
    WriterHandle { shared }
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

struct WriterQueues {
    buffers: HashMap<String, SessionBuffer>,
    session_order: VecDeque<String>,
    global_frames: VecDeque<BufferedFrame>,
}

impl WriterQueues {
    fn new() -> Self {
        Self {
            buffers: HashMap::new(),
            session_order: VecDeque::new(),
            global_frames: VecDeque::new(),
        }
    }
}

struct WriterShared {
    queues: Mutex<WriterQueues>,
    notify: Notify,
    config: WriterConfig,
}

impl WriterShared {
    fn new(config: WriterConfig) -> Self {
        Self {
            queues: Mutex::new(WriterQueues::new()),
            notify: Notify::new(),
            config,
        }
    }

    fn enqueue(&self, frame: OutFrame) {
        {
            let mut queues = self.queues.lock();
            let WriterQueues {
                buffers,
                session_order,
                global_frames,
            } = &mut *queues;
            enqueue_frame(frame, buffers, session_order, global_frames, self.config);
        }
        self.notify.notify_one();
    }

    fn dequeue(&self) -> Option<OutFrame> {
        let mut queues = self.queues.lock();
        if let Some(frame) = queues.global_frames.pop_front() {
            return Some(frame.frame);
        }

        let session_id = queues.session_order.pop_front()?;
        let buffer = queues.buffers.get_mut(&session_id)?;
        let frame = buffer.frames.pop_front().map(|buffered| buffered.frame);
        if buffer.frames.is_empty() {
            queues.buffers.remove(&session_id);
        } else {
            queues.session_order.push_back(session_id);
        }
        frame
    }
}

async fn writer_task(mut writer: BoxWriter, shared: Arc<WriterShared>) -> Result<(), AppError> {
    loop {
        if let Some(frame) = shared.dequeue() {
            write_frame(&mut writer, &frame).await?;
            continue;
        }
        shared.notify.notified().await;
    }
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

async fn write_frame(writer: &mut BoxWriter, frame: &OutFrame) -> Result<(), AppError> {
    let rendered = ndjson_safe_stringify(frame)?;
    writer
        .write_all(rendered.as_bytes())
        .await
        .map_err(AppError::Io)?;
    writer.write_all(b"\n").await.map_err(AppError::Io)?;
    writer.flush().await.map_err(AppError::Io)
}
