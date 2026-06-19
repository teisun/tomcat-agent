mod common;

use std::fs;
use std::time::Duration;

use serde_json::json;
use serial_test::serial;

use common::serve::{
    assert_ndjson_line, extract_json_body, response, setup_serve_fixture,
    spawn_scripted_openai_stream_server, spawn_serve_child, sse_delta, sse_done, sse_finish,
    ScriptedPart, ServeChild, ServeFixture,
};

fn initialize(child: &mut ServeChild) -> String {
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
    init.last().expect("initialize response")["payload"]["sessionId"]
        .as_str()
        .expect("session id")
        .to_string()
}

fn configure_openai_responses_fixture(fx: &ServeFixture, base_url: &str) {
    fs::write(
        fx.home_path.join(".tomcat").join("models.toml"),
        format!(
            r#"[[models]]
id = "gpt-5.4"
api = "openai-responses"
provider = "openai"
base_url = "{base_url}"
capabilities = {{ vision = true, files = true, tools = true, reasoning = true, web_search = false }}
"#
        ),
    )
    .expect("write openai-responses models override");
}

fn responses_sse_delta(content: &str) -> ScriptedPart {
    ScriptedPart {
        delay_ms: 0,
        body: format!(
            "data: {{\"type\":\"response.output_text.delta\",\"item_id\":\"m1\",\"content_index\":0,\"delta\":\"{content}\"}}\n\n"
        ),
    }
}

fn responses_sse_completed() -> ScriptedPart {
    ScriptedPart {
        delay_ms: 0,
        body: "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n".to_string(),
    }
}

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
    let session_id = initialize(&mut child);

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

#[test]
#[serial]
fn serve_interrupt_emits_agent_interrupted_e2e() {
    common::setup_logging();
    let server = spawn_scripted_openai_stream_server(vec![response(vec![
        sse_delta("partial"),
        common::serve::ScriptedPart {
            delay_ms: 350,
            body: sse_finish("stop").body,
        },
        sse_done(),
    ])]);
    let fx = setup_serve_fixture(&server.base_url);
    let mut child = spawn_serve_child(&fx);
    let session_id = initialize(&mut child);

    child.send_value(&json!({
        "type": "prompt",
        "id": "p-interrupt",
        "sessionId": session_id.clone(),
        "text": "start then interrupt",
        "params": {}
    }));
    let mut frames = child.recv_until(Duration::from_secs(5), |value| {
        value.get("type").and_then(|v| v.as_str()) == Some("message_update")
    });

    child.send_value(&json!({
        "type": "interrupt",
        "id": "interrupt-1",
        "sessionId": session_id.clone()
    }));
    frames.extend(child.recv_until(Duration::from_secs(5), |value| {
        value.get("type").and_then(|v| v.as_str()) == Some("agent_end")
            && value.get("sessionId").and_then(|v| v.as_str()) == Some(session_id.as_str())
    }));

    for value in &frames {
        assert_ndjson_line(value);
    }
    assert!(frames.iter().any(|value| {
        value.get("id").and_then(|v| v.as_str()) == Some("interrupt-1")
            && value.get("success").and_then(|v| v.as_bool()) == Some(true)
    }));
    assert!(frames.iter().any(|value| {
        value.get("type").and_then(|v| v.as_str()) == Some("agent_interrupted")
            && value.get("sessionId").and_then(|v| v.as_str()) == Some(session_id.as_str())
    }));
    assert!(frames.iter().any(|value| {
        value.get("type").and_then(|v| v.as_str()) == Some("agent_end")
            && value.get("sessionId").and_then(|v| v.as_str()) == Some(session_id.as_str())
            && value.get("error").and_then(|v| v.as_str()) == Some("interrupted")
    }));
}

#[test]
#[serial]
fn serve_stdout_only_emits_ndjson_frames() {
    common::setup_logging();
    let server = spawn_scripted_openai_stream_server(vec![response(vec![
        sse_delta("ndjson ok"),
        sse_finish("stop"),
        sse_done(),
    ])]);
    let fx = setup_serve_fixture(&server.base_url);
    let mut child = spawn_serve_child(&fx);

    child.send_raw("{not json");
    let parse_error = child.recv_value(Duration::from_secs(5));
    assert_ndjson_line(&parse_error);
    assert_eq!(parse_error["success"].as_bool(), Some(false));

    let session_id = initialize(&mut child);
    child.send_value(&json!({
        "type": "prompt",
        "id": "ndjson-1",
        "sessionId": session_id,
        "text": "say hi",
        "params": {}
    }));
    let frames = child.recv_until(Duration::from_secs(5), |value| {
        value.get("type").and_then(|v| v.as_str()) == Some("agent_end")
    });
    for value in &frames {
        assert_ndjson_line(value);
    }
}

#[test]
#[serial]
fn serve_prompt_with_attachment_roundtrip() {
    common::setup_logging();
    let server = spawn_scripted_openai_stream_server(vec![response(vec![
        responses_sse_delta("vision ok"),
        responses_sse_completed(),
    ])]);
    let fx = setup_serve_fixture(&server.base_url);
    configure_openai_responses_fixture(&fx, &server.base_url);
    let mut child = spawn_serve_child(&fx);
    let session_id = initialize(&mut child);

    child.send_value(&json!({
        "type": "prompt",
        "id": "img-e2e-1",
        "sessionId": session_id,
        "text": "describe attached image",
        "params": {
            "attachments": [
                {
                    "kind": "image",
                    "fileId": "file-vision"
                }
            ]
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
                    == Some("vision ok")
        }),
        "expected attachment prompt to reach agent_end, got {frames:?}"
    );

    let requests = server.captured_requests();
    assert_eq!(requests.len(), 1, "expected one responses API request");
    let body = extract_json_body(&requests[0]);
    let input = body["input"].as_array().expect("responses input array");
    let content = input[0]["content"].as_array().expect("responses content array");
    assert_eq!(content[0]["type"].as_str(), Some("input_text"));
    assert_eq!(content[0]["text"].as_str(), Some("describe attached image"));
    assert_eq!(content[1]["type"].as_str(), Some("input_image"));
    assert_eq!(content[1]["file_id"].as_str(), Some("file-vision"));
}
