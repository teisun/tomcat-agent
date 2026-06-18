mod common;

use std::time::Duration;

use serde_json::json;
use serial_test::serial;

use common::serve::{
    assert_ndjson_line, response, setup_serve_fixture, spawn_scripted_openai_stream_server,
    spawn_serve_child, sse_delta, sse_done, sse_finish,
};

#[test]
#[serial]
fn serve_stdio_user_roundtrip_e2e() {
    common::setup_logging();
    let server = spawn_scripted_openai_stream_server(vec![response(vec![
        sse_delta("hello from serve"),
        sse_finish("stop"),
        sse_done(),
    ])]);
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
    let session_id = init.last().expect("initialize response")["payload"]["sessionId"]
        .as_str()
        .expect("session id")
        .to_string();

    child.send_value(&json!({
        "type": "prompt",
        "id": "p1",
        "sessionId": session_id,
        "text": "say hello",
        "params": {}
    }));

    let frames = child.recv_until(Duration::from_secs(5), |value| {
        value.get("type").and_then(|v| v.as_str()) == Some("agent_end")
    });
    for value in &frames {
        assert_ndjson_line(value);
    }
    assert!(
        frames.iter().any(|value| {
            value.get("type").and_then(|v| v.as_str()) == Some("message_update")
                && value
                    .get("assistantMessageEvent")
                    .and_then(|v| v.get("delta"))
                    .and_then(|v| v.as_str())
                    == Some("hello from serve")
        }),
        "expected streamed reply, got {frames:?}"
    );

    let output = child.wait_for_exit(Duration::from_secs(5));
    assert!(
        output.status.success(),
        "serve e2e should exit cleanly: {output:?}"
    );
    assert_eq!(server.captured_requests().len(), 1);
}
