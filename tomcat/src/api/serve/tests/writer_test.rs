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
async fn alternate_encoder_runs_after_backpressure_and_can_consume_internal_frames() {
    use tokio::io::AsyncReadExt;
    let (mut input, output) = tokio::io::duplex(4096);
    let seen = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let encoded = seen.clone();
    let handle = super::super::writer::spawn_writer_with_encoder(
        Box::pin(output),
        WriterConfig {
            delta_coalesce_ms: 0,
            max_buffered_frames: 1,
        },
        move |frame| {
            let kind = frame.wire_type().unwrap().to_owned();
            encoded.lock().push(kind.clone());
            if kind == "llm_notice" {
                return Ok(None);
            }
            Ok(Some(serde_json::json!({"converted":kind}).to_string()))
        },
    );
    // Do not yield until pressure has been applied. Discarded deltas never enter the encoder.
    for text in ["first", "discarded", "also discarded"] {
        handle
            .send(event("s", "message_update", Some(text)))
            .unwrap();
    }
    handle.send(event("s", "agent_end", None)).unwrap();
    drop(handle);
    let mut text = String::new();
    tokio::time::timeout(Duration::from_secs(2), input.read_to_string(&mut text))
        .await
        .unwrap()
        .unwrap();
    let frames: Vec<serde_json::Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        frames,
        vec![
            serde_json::json!({"converted":"message_update"}),
            serde_json::json!({"converted":"agent_end"})
        ]
    );
    assert_eq!(
        *seen.lock(),
        vec!["message_update", "llm_notice", "agent_end"]
    );
}

#[tokio::test]
async fn writer_last_handle_drop_drains_frames_and_closes_the_stream() {
    use tokio::io::AsyncReadExt;
    let (mut input, output) = tokio::io::duplex(4096);
    let handle = spawn_writer(
        Box::pin(output),
        WriterConfig {
            delta_coalesce_ms: 0,
            max_buffered_frames: 8,
        },
    );
    let survivor = handle.clone();
    drop(handle);
    survivor.send(event("s", "agent_end", None)).unwrap();
    drop(survivor);
    let mut text = String::new();
    tokio::time::timeout(Duration::from_secs(2), input.read_to_string(&mut text))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(text.lines().count(), 1);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(text.trim()).unwrap()["type"],
        "agent_end"
    );
}

#[tokio::test(start_paused = true)]
async fn writer_deadline_applies_only_after_explicit_close_not_to_a_slow_reader() {
    use tokio::io::AsyncReadExt;
    let (mut input, output) = tokio::io::duplex(1);
    let handle = spawn_writer(
        Box::pin(output),
        WriterConfig {
            delta_coalesce_ms: 0,
            max_buffered_frames: 8,
        },
    );
    handle
        .send(event("s", "message_update", Some(&"x".repeat(1024))))
        .unwrap();
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(30)).await;
    let mut bytes = Vec::new();
    assert!(
        tokio::time::timeout(Duration::from_millis(1), handle.closed())
            .await
            .is_err(),
        "active slow connection must stay open"
    );
    handle.close();
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(2)).await;
    tokio::task::yield_now().await;
    tokio::time::timeout(Duration::from_secs(1), input.read_to_end(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    assert!(
        bytes.len() < 1024,
        "closed, non-reading peer must not retain a writer task"
    );
    assert!(handle.send(event("s", "agent_end", None)).is_err());
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

#[test]
fn writer_keeps_replay_separate_and_never_merges_different_assistant_ids() {
    let replay = OutFrame::Event(serde_json::json!({
        "type":"message_update", "sessionId":"s", "assistantMessageId":"a",
        "message":{"role":"assistant","content":"prefix","thinking_text":"reason"},
        "assistantMessageEvent":null
    }));
    assert!(!replay.is_lossless());
    let mut buffers = HashMap::new();
    let mut order = VecDeque::new();
    let mut globals = VecDeque::new();
    for frame in [event("s", "agent_start", None), replay.clone()] {
        super::super::writer::enqueue_frame(
            frame,
            &mut buffers,
            &mut order,
            &mut globals,
            WriterConfig {
                delta_coalesce_ms: 100,
                max_buffered_frames: 1,
            },
        );
    }
    assert_eq!(
        buffers["s"].frames.len(),
        2,
        "prefix replay keeps the existing backpressure notice contract"
    );
    assert_eq!(buffers["s"].frames[1].frame.wire_type(), Some("llm_notice"));
    buffers.clear();
    order.clear();
    let delta = |id: &str, text: &str| {
        OutFrame::Event(serde_json::json!({
            "type":"message_update", "sessionId":"s", "assistantMessageId":id, "message":{},
            "assistantMessageEvent":{"kind":"content_delta","delta":text}
        }))
    };
    for frame in [
        replay,
        delta("a", " tail"),
        delta("b", "next"),
        delta("b", " message"),
    ] {
        super::super::writer::enqueue_frame(
            frame,
            &mut buffers,
            &mut order,
            &mut globals,
            WriterConfig {
                delta_coalesce_ms: 100,
                max_buffered_frames: 8,
            },
        );
    }
    assert_eq!(buffers["s"].frames.len(), 3);
    let OutFrame::Event(last) = &buffers["s"].frames[2].frame else {
        panic!("event");
    };
    assert_eq!(last["assistantMessageId"], "b");
    assert_eq!(last["assistantMessageEvent"]["delta"], "next message");
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
