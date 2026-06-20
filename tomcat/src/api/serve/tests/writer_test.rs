use super::*;
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
    handle.send(event("s1", "message_update", Some("he"))).unwrap();
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
    handle.send(event("s1", "message_update", Some("a"))).unwrap();
    handle.send(event("s1", "message_update", Some("b"))).unwrap();
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
    handle.send(event("s1", "message_update", Some("a"))).unwrap();
    handle.send(event("s1", "message_update", Some("b"))).unwrap();
    handle.send(event("s1", "message_update", Some("c"))).unwrap();
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
    handle.send(event("s1", "message_update", Some("a1"))).unwrap();
    handle.send(event("s1", "message_update", Some("a2"))).unwrap();
    handle.send(event("s2", "message_update", Some("b1"))).unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    let bytes = shared.0.lock().clone();
    let rendered = String::from_utf8(bytes).unwrap();
    let lines: Vec<&str> = rendered.lines().collect();
    assert_eq!(lines.len(), 3);
    assert!(lines[0].contains("\"sessionId\":\"s1\""));
    assert!(lines[1].contains("\"sessionId\":\"s2\""));
    assert!(lines[2].contains("\"sessionId\":\"s1\""));
}
