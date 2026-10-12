use super::*;
use std::collections::{HashMap, VecDeque};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

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
        .send(OutFrame::Response(ResponseFrame::ok(
            Some("1".to_string()),
            None,
            None,
        )))
        .unwrap();
    handle
        .send(OutFrame::Response(ResponseFrame::ok(
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

#[tokio::test]
async fn serve_writer_backpressure_notice_emitted_once() {
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
    handle
        .send(event("s1", "message_update", Some("c")))
        .unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;

    let bytes = shared.0.lock().clone();
    let rendered = String::from_utf8(bytes).unwrap();
    let notices = rendered
        .lines()
        .filter(|line| line.contains("\"type\":\"llm_notice\""))
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

fn shell_preview(session: &str, call: &str, task: &str, output: &str, offset: u64) -> OutFrame {
    OutFrame::Event(serde_json::json!({
        "type": "tool_execution_update",
        "sessionId": session,
        "toolCallId": call,
        "toolName": "bash",
        "partialResult": {
            "taskId": task,
            "output": output,
            "startOffset": offset,
            "nextOffset": offset + output.len() as u64,
            "truncated": false
        }
    }))
}

#[test]
fn shell_previews_replace_by_tool_call_and_stay_bounded() {
    let mut buffers = HashMap::new();
    let mut order = VecDeque::new();
    let mut global = VecDeque::new();
    let config = WriterConfig {
        delta_coalesce_ms: 0,
        max_buffered_frames: 2,
    };
    enqueue_frame(
        shell_preview("s", "c1", "t1", "old", 0),
        &mut buffers,
        &mut order,
        &mut global,
        config,
    );
    enqueue_frame(
        shell_preview("s", "c2", "t2", "other", 0),
        &mut buffers,
        &mut order,
        &mut global,
        config,
    );
    enqueue_frame(
        shell_preview("s", "c1", "t1", "latest", 3),
        &mut buffers,
        &mut order,
        &mut global,
        config,
    );

    let buffer = buffers.get("s").expect("session");
    assert_eq!(buffer.frames.len(), 2);
    let latest = buffer
        .frames
        .iter()
        .find(|frame| shell_preview_key(&frame.frame).as_deref() == Some("c1"))
        .expect("c1");
    let OutFrame::Event(value) = &latest.frame else {
        panic!("event")
    };
    assert_eq!(value["partialResult"]["output"], "latest");
    assert_eq!(value["partialResult"]["truncated"], true);
}

#[test]
fn shell_preview_pressure_never_drops_completion_or_task_output_updates() {
    let mut buffers = HashMap::new();
    let mut order = VecDeque::new();
    let mut global = VecDeque::new();
    let config = WriterConfig {
        delta_coalesce_ms: 0,
        max_buffered_frames: 1,
    };
    enqueue_frame(
        shell_preview("s", "c1", "t1", "preview", 0),
        &mut buffers,
        &mut order,
        &mut global,
        config,
    );
    enqueue_frame(
        event("s", "tool_execution_end", None),
        &mut buffers,
        &mut order,
        &mut global,
        config,
    );
    enqueue_frame(
        OutFrame::Event(serde_json::json!({
            "type": "tool_execution_update",
            "sessionId": "s",
            "toolCallId": "task-output-call",
            "toolName": "task_output",
            "partialResult": {"taskId": "t1", "output": "countdown"}
        })),
        &mut buffers,
        &mut order,
        &mut global,
        config,
    );

    let buffer = buffers.get("s").expect("session");
    assert!(buffer
        .frames
        .iter()
        .any(|frame| frame.frame.wire_type() == Some("tool_execution_end")));
    assert!(buffer.frames.iter().any(|frame| match &frame.frame {
        OutFrame::Event(value) =>
            value.get("toolName").and_then(serde_json::Value::as_str) == Some("task_output"),
        _ => false,
    }));
}
