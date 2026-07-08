mod common;

use std::time::Duration;

use serde_json::json;
use serial_test::serial;

use common::serve::{
    assert_ndjson_line, response, setup_serve_fixture,
    spawn_scripted_openai_stream_server_with_auto_title, spawn_serve_child, sse_delta, sse_done,
    sse_finish, ServeChild,
};

fn initialize(child: &mut ServeChild) -> String {
    child.send_value(&json!({
        "type": "control_request",
        "requestId": "init-1",
        "subtype": "initialize",
        "payload": {}
    }));
    let frames = child.recv_until(Duration::from_secs(5), |value| {
        value.get("type").and_then(|v| v.as_str()) == Some("control_response")
            && value.get("requestId").and_then(|v| v.as_str()) == Some("init-1")
    });
    let frame = frames.last().expect("initialize response");
    frame["payload"]["sessionId"]
        .as_str()
        .expect("default session id")
        .to_string()
}

fn new_session(child: &mut ServeChild, id: &str) -> String {
    child.send_value(&json!({
        "type": "new_session",
        "id": id,
        "params": {}
    }));
    let frames = child.recv_until(Duration::from_secs(5), |value| {
        value.get("id").and_then(|v| v.as_str()) == Some(id)
    });
    let frame = frames.last().expect("new_session response");
    frame["payload"]["sessionId"]
        .as_str()
        .expect("new session id")
        .to_string()
}

#[test]
#[serial]
fn serve_multi_session_concurrency_and_isolation() {
    common::setup_logging();
    let server = spawn_scripted_openai_stream_server_with_auto_title(vec![
        response(vec![
            sse_delta("slow"),
            common::serve::ScriptedPart {
                delay_ms: 250,
                body: sse_finish("stop").body,
            },
            sse_done(),
        ]),
        response(vec![sse_delta("fast"), sse_finish("stop"), sse_done()]),
    ]);
    let fx = setup_serve_fixture(&server.base_url);
    let mut child = spawn_serve_child(&fx);
    let session_a = initialize(&mut child);
    let session_b = new_session(&mut child, "new-1");

    child.send_value(&json!({
        "type": "prompt",
        "id": "p1",
        "sessionId": session_a.clone(),
        "text": "slow",
        "params": {}
    }));
    child.send_value(&json!({
        "type": "prompt",
        "id": "p2",
        "sessionId": session_b.clone(),
        "text": "fast",
        "params": {}
    }));

    let mut frames = Vec::new();
    let mut saw_end_a = false;
    let mut saw_end_b = false;
    while !(saw_end_a && saw_end_b) {
        let value = child.recv_value(Duration::from_secs(5));
        assert_ndjson_line(&value);
        if value.get("type").and_then(|v| v.as_str()) == Some("agent_end") {
            match value.get("sessionId").and_then(|v| v.as_str()) {
                Some(id) if id == session_a => saw_end_a = true,
                Some(id) if id == session_b => saw_end_b = true,
                _ => {}
            }
        }
        frames.push(value);
    }

    let start_a = frames
        .iter()
        .position(|value| {
            value.get("type").and_then(|v| v.as_str()) == Some("agent_start")
                && value.get("sessionId").and_then(|v| v.as_str()) == Some(session_a.as_str())
        })
        .expect("session A agent_start");
    let start_b = frames
        .iter()
        .position(|value| {
            value.get("type").and_then(|v| v.as_str()) == Some("agent_start")
                && value.get("sessionId").and_then(|v| v.as_str()) == Some(session_b.as_str())
        })
        .expect("session B agent_start");
    let end_a = frames
        .iter()
        .position(|value| {
            value.get("type").and_then(|v| v.as_str()) == Some("agent_end")
                && value.get("sessionId").and_then(|v| v.as_str()) == Some(session_a.as_str())
        })
        .expect("session A agent_end");
    let end_b = frames
        .iter()
        .position(|value| {
            value.get("type").and_then(|v| v.as_str()) == Some("agent_end")
                && value.get("sessionId").and_then(|v| v.as_str()) == Some(session_b.as_str())
        })
        .expect("session B agent_end");
    let first_end = end_a.min(end_b);
    let second_start = start_a.max(start_b);

    assert!(start_a < end_a, "session A should start before it ends");
    assert!(start_b < end_b, "session B should start before it ends");
    assert!(
        second_start < first_end,
        "both sessions should start before either session ends: {frames:?}"
    );
    assert!(frames.iter().any(|value| {
        value.get("type").and_then(|v| v.as_str()) == Some("message_update")
            && value.get("sessionId").and_then(|v| v.as_str()) == Some(session_a.as_str())
    }));
    assert!(frames.iter().any(|value| {
        value.get("type").and_then(|v| v.as_str()) == Some("message_update")
            && value.get("sessionId").and_then(|v| v.as_str()) == Some(session_b.as_str())
    }));
    assert!(frames.iter().all(|value| {
        value
            .get("sessionId")
            .and_then(|v| v.as_str())
            .map(|session_id| session_id == session_a || session_id == session_b)
            .unwrap_or(true)
    }));
    assert!(frames.iter().any(|value| {
        value.get("type").and_then(|v| v.as_str()) == Some("message_update")
            && value["assistantMessageEvent"]["delta"].as_str() == Some("slow")
    }));
    assert!(frames.iter().any(|value| {
        value.get("type").and_then(|v| v.as_str()) == Some("message_update")
            && value["assistantMessageEvent"]["delta"].as_str() == Some("fast")
    }));
}

#[test]
#[serial]
fn serve_same_session_is_busy_until_turn_finishes() {
    common::setup_logging();
    let server = spawn_scripted_openai_stream_server_with_auto_title(vec![response(vec![
        sse_delta("busy"),
        common::serve::ScriptedPart {
            delay_ms: 250,
            body: sse_finish("stop").body,
        },
        sse_done(),
    ])]);
    let fx = setup_serve_fixture(&server.base_url);
    let mut child = spawn_serve_child(&fx);
    let session_id = initialize(&mut child);

    child.send_value(&json!({
        "type": "prompt",
        "id": "p1",
        "sessionId": session_id.clone(),
        "text": "first",
        "params": {}
    }));
    child.send_value(&json!({
        "type": "prompt",
        "id": "p2",
        "sessionId": session_id,
        "text": "second",
        "params": {}
    }));

    let mut frames = Vec::new();
    let mut busy = None;
    for _ in 0..16 {
        let value = child.recv_value(Duration::from_secs(5));
        if value.get("id").and_then(|v| v.as_str()) == Some("p2") {
            busy = Some(value.clone());
            frames.push(value);
            break;
        }
        frames.push(value);
    }
    let busy = busy.unwrap_or_else(|| panic!("missing busy response, saw frames: {frames:?}"));
    assert_eq!(busy["success"].as_bool(), Some(false));
    assert_eq!(busy["error"].as_str(), Some("busy"));

    let _ = child.recv_until(Duration::from_secs(5), |value| {
        value.get("type").and_then(|v| v.as_str()) == Some("agent_end")
    });
}
