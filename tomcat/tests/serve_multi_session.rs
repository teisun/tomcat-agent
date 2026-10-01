mod common;

#[cfg(feature = "test-streamable-http-server")]
#[path = "support/serve_mcp_multi_session.rs"]
mod mcp_cases;

use std::time::Duration;

use serde_json::json;
use serial_test::serial;

use common::serve::{
    assert_ndjson_line, response, setup_serve_fixture,
    spawn_scripted_openai_stream_server_with_auto_title, spawn_serve_child, sse_delta, sse_done,
    sse_finish, ServeChild,
};

// `tomcat serve` loads embedded assets before processing stdin. Match the other
// real-stdio suite's budget so this remains a correctness wait, not a startup
// performance assertion under the default four-way integration gate.
const SERVE_TIMEOUT: Duration = Duration::from_secs(10);

fn initialize(child: &mut ServeChild) -> String {
    child.send_value(&json!({
        "type": "control_request",
        "requestId": "init-1",
        "subtype": "initialize",
        "payload": {}
    }));
    let frames = child.recv_until(SERVE_TIMEOUT, |value| {
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
    let frames = child.recv_until(SERVE_TIMEOUT, |value| {
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
        let value = child.recv_value(SERVE_TIMEOUT);
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
fn compact_job_does_not_block_another_session_and_emits_no_agent_lifecycle() {
    use common::serve::{spawn_routed_openai_stream_server, ScriptedPart};
    use std::sync::{mpsc, Arc, Mutex};
    // The server records compact entering before waiting; B completes while
    // that gate is still closed. No sleep or machine-speed assertion is needed.
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let release_rx = Arc::new(Mutex::new(release_rx));
    let server = spawn_routed_openai_stream_server(move |request| {
        if request["stream"] == false {
            let messages = request["messages"].to_string();
            assert!(messages.contains("Respond with text only. Do not call any tools."));
            assert!(messages.contains("A seed for compact"));
            entered_tx.send(()).unwrap();
            release_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(15))
                .expect("compact release");
            response(vec![ScriptedPart {delay_ms:0, body:json!({"id":"compact","choices":[{"index":0,"message":{"role":"assistant","content":"A compact checkpoint with user goal and progress."},"finish_reason":"stop"}],"usage":{"prompt_tokens":30,"completion_tokens":10,"total_tokens":40}}).to_string()}])
        } else {
            response(vec![sse_delta("done"), sse_finish("stop"), sse_done()])
        }
    });
    let fx = setup_serve_fixture(&server.base_url);
    let mut child = spawn_serve_child(&fx);
    let a = initialize(&mut child);
    let b = new_session(&mut child, "b-new");
    child.send_value(
        &json!({"type":"prompt","id":"seed","sessionId":a,"text":"A seed for compact","params":{}}),
    );
    child.recv_until(SERVE_TIMEOUT, |frame| {
        frame["type"] == "agent_idle" && frame["sessionId"] == a
    });
    child.send_value(&json!({"type":"compact","id":"compact-a","sessionId":a}));
    entered_rx
        .recv_timeout(SERVE_TIMEOUT)
        .expect("real compact HTTP request must reach gate");
    child.send_value(&json!({"type":"get_state","id":"state-b","sessionId":b}));
    let mut frames = child.recv_until(SERVE_TIMEOUT, |frame| frame["id"] == "state-b");
    assert_eq!(frames.last().unwrap()["success"], true);
    for (kind, id) in [
        ("prompt", "reject-prompt"),
        ("follow_up", "reject-follow"),
        ("steer", "reject-steer"),
    ] {
        child.send_value(
            &json!({"type":kind,"id":id,"sessionId":a,"text":"must not append","params":{}}),
        );
        let next = child.recv_until(SERVE_TIMEOUT, |frame| frame["id"] == id);
        assert_eq!(next.last().unwrap()["error"], "busy");
        frames.extend(next);
    }
    child.send_value(&json!({"type":"prompt","id":"prompt-b","sessionId":b,"text":"B independent turn","params":{}}));
    frames.extend(child.recv_until(SERVE_TIMEOUT, |frame| {
        frame["type"] == "agent_idle" && frame["sessionId"] == b
    }));
    assert!(!frames.iter().any(|frame| frame["id"] == "compact-a"));
    assert!(frames
        .iter()
        .any(|frame| frame["type"] == "agent_end" && frame["sessionId"] == b));
    release_tx.send(()).unwrap();
    frames.extend(child.recv_until(SERVE_TIMEOUT, |frame| frame["id"] == "compact-a"));
    assert_eq!(frames.last().unwrap()["success"], true);
    assert!(
        frames.last().unwrap()["payload"]["coveredMessageCount"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert!(!frames.iter().any(|frame| frame["sessionId"] == a
        && matches!(
            frame["type"].as_str(),
            Some("agent_start" | "agent_end" | "agent_idle")
        )));
    child.send_value(
        &json!({"type":"run_slash_command","id":"after-compact","sessionId":a,"text":"/reload"}),
    );
    let next = child.recv_until(SERVE_TIMEOUT, |frame| frame["id"] == "after-compact");
    assert_eq!(
        next.last().unwrap()["payload"]["ok"],
        true,
        "admission must already be idle at compact response"
    );
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
        let value = child.recv_value(SERVE_TIMEOUT);
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

    let _ = child.recv_until(SERVE_TIMEOUT, |value| {
        value.get("type").and_then(|v| v.as_str()) == Some("agent_end")
    });
}
