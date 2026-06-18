mod common;

use std::time::Duration;

use serde_json::json;
use serial_test::serial;

use common::serve::{
    response, setup_serve_fixture, spawn_scripted_openai_stream_server, spawn_serve_child,
    sse_delta, sse_done, sse_finish, sse_tool_call, ServeChild,
};

const ASK_QUESTION_ARGS: &str = r#"{"questions":[{"id":"q1","prompt":"Pick one","options":[{"id":"a","label":"A","recommended":true},{"id":"b","label":"B"}]}]}"#;

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
    frames.last().expect("initialize response")["payload"]["sessionId"]
        .as_str()
        .expect("default session")
        .to_string()
}

fn ask_question_server() -> common::serve::ScriptedOpenAiServer {
    spawn_scripted_openai_stream_server(vec![
        response(vec![
            sse_tool_call("call_1", "ask_question", ASK_QUESTION_ARGS),
            sse_finish("tool_calls"),
            sse_done(),
        ]),
        response(vec![
            sse_delta("after approval"),
            sse_finish("stop"),
            sse_done(),
        ]),
    ])
}

#[test]
#[serial]
fn serve_ask_question_roundtrip_resumes_turn() {
    common::setup_logging();
    let server = ask_question_server();
    let fx = setup_serve_fixture(&server.base_url);
    let mut child = spawn_serve_child(&fx);
    let session_id = initialize(&mut child);

    child.send_value(&json!({
        "type": "prompt",
        "id": "ask-1",
        "sessionId": session_id.clone(),
        "text": "ask me a question",
        "params": {}
    }));

    let frames = child.recv_until(Duration::from_secs(5), |value| {
        value.get("type").and_then(|v| v.as_str()) == Some("control_request")
            && value.get("subtype").and_then(|v| v.as_str()) == Some("ask_question")
    });
    let control = frames.last().expect("control request");
    let request_id = control["requestId"]
        .as_str()
        .expect("ask_question request id");

    child.send_value(&json!({
        "type": "control_response",
        "requestId": request_id,
        "sessionId": session_id,
        "payload": {
            "answers": [{
                "question_id": "q1",
                "option_ids": ["a"],
                "custom_text": null,
                "skipped": false,
                "picked_recommended": true
            }],
            "cancelled": false
        }
    }));

    let frames = child.recv_until(Duration::from_secs(5), |value| {
        value.get("type").and_then(|v| v.as_str()) == Some("agent_end")
    });
    assert!(
        frames.iter().any(|value| {
            value.get("type").and_then(|v| v.as_str()) == Some("message_update")
                && value
                    .get("assistantMessageEvent")
                    .and_then(|v| v.get("delta"))
                    .and_then(|v| v.as_str())
                    == Some("after approval")
        }),
        "expected final delta after approval, got {frames:?}"
    );
    assert_eq!(server.captured_requests().len(), 2);
}

#[test]
#[serial]
fn serve_ask_question_cancel_roundtrip_does_not_hang() {
    common::setup_logging();
    let server = ask_question_server();
    let fx = setup_serve_fixture(&server.base_url);
    let mut child = spawn_serve_child(&fx);
    let session_id = initialize(&mut child);

    child.send_value(&json!({
        "type": "prompt",
        "id": "ask-2",
        "sessionId": session_id.clone(),
        "text": "ask then cancel",
        "params": {}
    }));

    let frames = child.recv_until(Duration::from_secs(5), |value| {
        value.get("type").and_then(|v| v.as_str()) == Some("control_request")
            && value.get("subtype").and_then(|v| v.as_str()) == Some("ask_question")
    });
    let control = frames.last().expect("control request");
    let request_id = control["requestId"]
        .as_str()
        .expect("ask_question request id");

    child.send_value(&json!({
        "type": "control_cancel",
        "requestId": request_id,
        "sessionId": session_id,
        "payload": {}
    }));

    let frames = child.recv_until(Duration::from_secs(5), |value| {
        value.get("type").and_then(|v| v.as_str()) == Some("agent_end")
    });
    assert!(
        frames.iter().any(|value| {
            value.get("type").and_then(|v| v.as_str()) == Some("message_update")
                && value
                    .get("assistantMessageEvent")
                    .and_then(|v| v.get("delta"))
                    .and_then(|v| v.as_str())
                    == Some("after approval")
        }),
        "expected turn to settle after cancel, got {frames:?}"
    );
    assert_eq!(server.captured_requests().len(), 2);
}
