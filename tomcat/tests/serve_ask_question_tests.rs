mod common;

use std::time::Duration;

use serde_json::json;
use serial_test::serial;

use common::serve::{
    response, setup_serve_fixture, try_extract_json_body,
    spawn_scripted_openai_stream_server_with_auto_title, spawn_serve_child, sse_delta, sse_done,
    sse_finish, sse_tool_call, ScriptedOpenAiServer, ServeChild,
};

const ASK_QUESTION_ARGS: &str = r#"{"questions":[{"id":"q1","prompt":"Pick one","options":[{"id":"a","label":"A","recommended":true},{"id":"b","label":"B"}]}]}"#;

fn is_scripted_followup_delta(value: &serde_json::Value) -> bool {
    matches!(
        value
            .get("assistantMessageEvent")
            .and_then(|v| v.get("delta"))
            .and_then(|v| v.as_str()),
        Some("after approval" | "second session kept running")
    )
}

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

fn new_session(child: &mut ServeChild, request_id: &str) -> String {
    child.send_value(&json!({
        "type": "new_session",
        "id": request_id,
        "params": {}
    }));
    let frames = child.recv_until(Duration::from_secs(5), |value| {
        value.get("id").and_then(|v| v.as_str()) == Some(request_id)
    });
    frames.last().expect("new_session response")["payload"]["sessionId"]
        .as_str()
        .expect("new session id")
        .to_string()
}

fn ask_question_server() -> common::serve::ScriptedOpenAiServer {
    spawn_scripted_openai_stream_server_with_auto_title(vec![
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

fn streamed_request_count(server: &ScriptedOpenAiServer) -> usize {
    server
        .captured_requests()
        .into_iter()
        .filter_map(|request| try_extract_json_body(&request))
        .filter(|body| body["stream"].as_bool() == Some(true))
        .count()
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
    assert_eq!(streamed_request_count(&server), 2);
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
            value.get("type").and_then(|v| v.as_str()) == Some("tool_execution_end")
                && value.get("toolName").and_then(|v| v.as_str()) == Some("ask_question")
                && value
                    .get("result")
                    .and_then(|v| v.as_str())
                    .is_some_and(|result| result.contains("\"cancelled\":true"))
        }),
        "expected cancelled ask_question result before settling, got {frames:?}"
    );
    assert!(
        frames.iter().any(|value| {
            value.get("type").and_then(|v| v.as_str()) == Some("agent_end")
                && value.get("error").is_some_and(|v| v.is_null())
        }),
        "expected cancel path to settle cleanly, got {frames:?}"
    );
}

#[test]
#[serial]
fn serve_ask_question_routes_by_session() {
    common::setup_logging();
    let server = spawn_scripted_openai_stream_server_with_auto_title(vec![
        response(vec![
            sse_tool_call("call_1", "ask_question", ASK_QUESTION_ARGS),
            sse_finish("tool_calls"),
            sse_done(),
        ]),
        response(vec![
            sse_delta("second session kept running"),
            sse_finish("stop"),
            sse_done(),
        ]),
        response(vec![
            sse_delta("after approval"),
            sse_finish("stop"),
            sse_done(),
        ]),
    ]);
    let fx = setup_serve_fixture(&server.base_url);
    let mut child = spawn_serve_child(&fx);
    let session_a = initialize(&mut child);
    let session_b = new_session(&mut child, "new-2");

    child.send_value(&json!({
        "type": "prompt",
        "id": "ask-route-1",
        "sessionId": session_a.clone(),
        "text": "ask session a",
        "params": {}
    }));

    let ask_frames = child.recv_until(Duration::from_secs(5), |value| {
        value.get("type").and_then(|v| v.as_str()) == Some("control_request")
            && value.get("subtype").and_then(|v| v.as_str()) == Some("ask_question")
    });
    let control = ask_frames.last().expect("control request");
    assert_eq!(control["sessionId"].as_str(), Some(session_a.as_str()));
    let request_id = control["requestId"]
        .as_str()
        .expect("ask_question request id");

    child.send_value(&json!({
        "type": "prompt",
        "id": "session-b-1",
        "sessionId": session_b.clone(),
        "text": "run in session b",
        "params": {}
    }));
    let session_b_frames = child.recv_until(Duration::from_secs(5), |value| {
        value.get("type").and_then(|v| v.as_str()) == Some("agent_end")
            && value.get("sessionId").and_then(|v| v.as_str()) == Some(session_b.as_str())
    });
    assert!(
        session_b_frames.iter().any(|value| {
            value.get("type").and_then(|v| v.as_str()) == Some("message_update")
                && value.get("sessionId").and_then(|v| v.as_str()) == Some(session_b.as_str())
                && is_scripted_followup_delta(value)
        }),
        "expected session b to continue independently, got {session_b_frames:?}"
    );
    assert!(
        !session_b_frames.iter().any(|value| {
            value.get("type").and_then(|v| v.as_str()) == Some("control_request")
                && value.get("sessionId").and_then(|v| v.as_str()) == Some(session_b.as_str())
        }),
        "ask_question control should stay on session a: {session_b_frames:?}"
    );

    child.send_value(&json!({
        "type": "control_response",
        "requestId": request_id,
        "sessionId": session_a.clone(),
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

    let session_a_frames = child.recv_until(Duration::from_secs(5), |value| {
        value.get("type").and_then(|v| v.as_str()) == Some("agent_end")
            && value.get("sessionId").and_then(|v| v.as_str()) == Some(session_a.as_str())
    });
    assert!(
        session_a_frames.iter().any(|value| {
            value.get("type").and_then(|v| v.as_str()) == Some("message_update")
                && value.get("sessionId").and_then(|v| v.as_str()) == Some(session_a.as_str())
                && is_scripted_followup_delta(value)
        }),
        "expected session a to resume after approval, got {session_a_frames:?}"
    );
}

#[test]
#[serial]
fn serve_interrupt_emits_agent_interrupted_and_tool_execution_end() {
    common::setup_logging();
    let server = spawn_scripted_openai_stream_server_with_auto_title(vec![response(vec![
        sse_tool_call("call_1", "ask_question", ASK_QUESTION_ARGS),
        sse_finish("tool_calls"),
        sse_done(),
    ])]);
    let fx = setup_serve_fixture(&server.base_url);
    let mut child = spawn_serve_child(&fx);
    let session_id = initialize(&mut child);

    child.send_value(&json!({
        "type": "prompt",
        "id": "interrupt-ask-1",
        "sessionId": session_id.clone(),
        "text": "ask then interrupt",
        "params": {}
    }));

    let mut frames = child.recv_until(Duration::from_secs(5), |value| {
        value.get("type").and_then(|v| v.as_str()) == Some("control_request")
            && value.get("subtype").and_then(|v| v.as_str()) == Some("ask_question")
    });
    assert!(frames.iter().any(|value| {
        value.get("type").and_then(|v| v.as_str()) == Some("tool_execution_start")
            && value.get("toolCallId").and_then(|v| v.as_str()) == Some("call_1")
    }));

    child.send_value(&json!({
        "type": "interrupt",
        "id": "interrupt-ask-ack",
        "sessionId": session_id.clone()
    }));
    frames.extend(child.recv_until(Duration::from_secs(5), |value| {
        value.get("type").and_then(|v| v.as_str()) == Some("agent_end")
            && value.get("sessionId").and_then(|v| v.as_str()) == Some(session_id.as_str())
    }));

    assert!(frames.iter().any(|value| {
        value.get("id").and_then(|v| v.as_str()) == Some("interrupt-ask-ack")
            && value.get("success").and_then(|v| v.as_bool()) == Some(true)
    }));
    assert!(frames.iter().any(|value| {
        value.get("type").and_then(|v| v.as_str()) == Some("agent_interrupted")
            && value.get("sessionId").and_then(|v| v.as_str()) == Some(session_id.as_str())
    }));
    assert!(frames.iter().any(|value| {
        value.get("type").and_then(|v| v.as_str()) == Some("tool_execution_end")
            && value.get("toolCallId").and_then(|v| v.as_str()) == Some("call_1")
    }));
    assert!(frames.iter().any(|value| {
        value.get("type").and_then(|v| v.as_str()) == Some("agent_end")
            && value.get("error").and_then(|v| v.as_str()) == Some("interrupted")
    }));
}
