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
    assert_eq!(response["success"].as_bool(), Some(false));
    assert_eq!(
        response["error"].as_str(),
        Some("unknown_command: mystery")
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
