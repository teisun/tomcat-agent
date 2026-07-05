mod common;

use std::time::Duration;

use serde_json::json;
use serial_test::serial;

use common::serve::{setup_serve_fixture, spawn_scripted_openai_stream_server, spawn_serve_child};

#[test]
#[serial]
fn serve_parse_error_does_not_break_following_initialize() {
    common::setup_logging();
    let server = spawn_scripted_openai_stream_server(vec![]);
    let fx = setup_serve_fixture(&server.base_url);
    let mut child = spawn_serve_child(&fx);

    child.send_raw("{not json");
    let parse_error = child.recv_value(Duration::from_secs(5));
    assert_eq!(parse_error["type"].as_str(), Some("response"));
    assert_eq!(parse_error["success"].as_bool(), Some(false));
    assert!(
        parse_error["error"]
            .as_str()
            .unwrap_or_default()
            .contains("parse_error"),
        "expected parse error response, got {parse_error:?}"
    );

    child.send_value(&json!({
        "type": "control_request",
        "requestId": "init-1",
        "subtype": "initialize",
        "payload": {}
    }));
    let init = child.recv_until(Duration::from_secs(5), |value| {
        value.get("type").and_then(|v| v.as_str()) == Some("control_response")
            && value.get("requestId").and_then(|v| v.as_str()) == Some("init-1")
    });
    assert_eq!(
        init.last()
            .and_then(|value| value["payload"]["protocolVersion"].as_i64()),
        Some(1)
    );
}

#[test]
#[serial]
fn serve_unknown_command_returns_error_response() {
    common::setup_logging();
    let server = spawn_scripted_openai_stream_server(vec![]);
    let fx = setup_serve_fixture(&server.base_url);
    let mut child = spawn_serve_child(&fx);

    child.send_value(&json!({
        "type": "mystery",
        "id": "mystery-1"
    }));
    let response = child.recv_value(Duration::from_secs(5));
    assert_eq!(response["type"].as_str(), Some("response"));
    assert_eq!(response["id"].as_str(), Some("mystery-1"));
    assert_eq!(response["success"].as_bool(), Some(false));
    assert_eq!(response["error"].as_str(), Some("unknown_command: mystery"));
}

#[test]
#[serial]
fn serve_parse_error_response_preserves_request_id() {
    common::setup_logging();
    let server = spawn_scripted_openai_stream_server(vec![]);
    let fx = setup_serve_fixture(&server.base_url);
    let mut child = spawn_serve_child(&fx);

    child.send_value(&json!({
        "type": "prompt",
        "id": "bad-session-id",
        "sessionId": null,
        "text": "hello"
    }));
    let response = child.recv_value(Duration::from_secs(5));
    assert_eq!(response["type"].as_str(), Some("response"));
    assert_eq!(response["id"].as_str(), Some("bad-session-id"));
    assert!(
        response.get("sessionId").is_none(),
        "invalid null sessionId must not echo back as a string"
    );
    assert_eq!(response["success"].as_bool(), Some(false));
    assert_eq!(
        response["error"].as_str(),
        Some("invalid_request: sessionId must be omitted or a string")
    );
}

#[test]
#[serial]
fn serve_set_thinking_level_roundtrip_over_real_stdio_writes_global_store() {
    common::setup_logging();
    let server = spawn_scripted_openai_stream_server(vec![]);
    let fx = setup_serve_fixture(&server.base_url);
    let mut child = spawn_serve_child(&fx);

    child.send_value(&json!({
        "type": "control_request",
        "requestId": "init-1",
        "subtype": "initialize",
        "payload": {}
    }));
    let init = child.recv_until(Duration::from_secs(5), |value| {
        value.get("type").and_then(|v| v.as_str()) == Some("control_response")
            && value.get("requestId").and_then(|v| v.as_str()) == Some("init-1")
    });
    let session_id = init
        .last()
        .and_then(|value| value["payload"]["sessionId"].as_str())
        .expect("initialize sessionId")
        .to_string();

    child.send_value(&json!({
        "type": "set_thinking_level",
        "id": "effort-1",
        "sessionId": session_id,
        "model": "gpt-5.4",
        "level": "high"
    }));
    let response = child.recv_value(Duration::from_secs(5));
    assert_eq!(response["type"].as_str(), Some("response"));
    assert_eq!(response["id"].as_str(), Some("effort-1"));
    assert_eq!(response["success"].as_bool(), Some(true));
    assert_eq!(response["payload"]["model"].as_str(), Some("gpt-5.4"));
    assert_eq!(response["payload"]["level"].as_str(), Some("high"));

    child.send_value(&json!({
        "type": "get_state",
        "id": "state-1",
        "sessionId": response["sessionId"].as_str().unwrap_or_default()
    }));
    let state = child.recv_until(Duration::from_secs(5), |value| {
        value.get("id").and_then(|v| v.as_str()) == Some("state-1")
    });
    let state_response = state
        .iter()
        .find(|value| value.get("id").and_then(|v| v.as_str()) == Some("state-1"))
        .expect("get_state response");
    assert_eq!(state_response["success"].as_bool(), Some(true));
    assert_eq!(
        state_response["payload"]["thinkingLevel"].as_str(),
        Some("high")
    );

    let store_path = fx.home_path.join(".tomcat").join("model-thinking.json");
    let store = std::fs::read_to_string(&store_path).expect("read global model thinking store");
    let parsed: serde_json::Value =
        serde_json::from_str(&store).expect("parse model thinking store");
    assert_eq!(parsed["models"]["gpt-5.4"].as_str(), Some("high"));
    assert!(
        !fx.home_path
            .join(".tomcat")
            .join("agents")
            .join("main")
            .join("sessions")
            .join("model-thinking.json")
            .exists(),
        "legacy sessions/model-thinking.json should be absent",
    );
}

#[test]
#[serial]
fn serve_eof_exits_cleanly() {
    common::setup_logging();
    let server = spawn_scripted_openai_stream_server(vec![]);
    let fx = setup_serve_fixture(&server.base_url);
    let mut child = spawn_serve_child(&fx);

    child.send_value(&json!({
        "type": "control_request",
        "requestId": "init-1",
        "subtype": "initialize",
        "payload": {}
    }));
    let _ = child.recv_until(Duration::from_secs(5), |value| {
        value.get("type").and_then(|v| v.as_str()) == Some("control_response")
            && value.get("requestId").and_then(|v| v.as_str()) == Some("init-1")
    });

    let output = child.wait_for_exit(Duration::from_secs(5));
    assert!(
        output.status.success(),
        "serve should exit cleanly: {output:?}"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("panic"),
        "stderr should not contain panic output: {stderr}"
    );
}
