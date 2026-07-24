mod common;

use std::fs;
use std::time::Duration;

use serde_json::{json, Value};
use serial_test::serial;

use common::serve::{
    assert_ndjson_line, captured_non_title_requests, extract_json_body, response,
    setup_serve_fixture, spawn_scripted_openai_stream_server,
    spawn_scripted_openai_stream_server_with_auto_title, spawn_serve_child, sse_delta, sse_done,
    sse_finish, ScriptedOpenAiServer, ScriptedPart, ServeChild, ServeFixture,
};

const WAIT_TIMEOUT: Duration = Duration::from_secs(10);

fn initialize(child: &mut ServeChild) -> String {
    child.send_value(&json!({
        "type": "control_request",
        "requestId": "init-1",
        "subtype": "initialize",
        "payload": {}
    }));
    let init = child.recv_until(WAIT_TIMEOUT, |value| {
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

[[models]]
id = "utility-flash"
api = "openai"
provider = "openai"
base_url = "http://127.0.0.1:1"
capabilities = {{ vision = false, files = false, tools = false, reasoning = false, web_search = false }}
"#
        ),
    )
    .expect("write openai-responses models override");
}

fn configure_multimodal_history_fixture(fx: &ServeFixture, base_url: &str) {
    fs::write(
        fx.home_path.join(".tomcat").join("models.toml"),
        format!(
            r#"[[models]]
id = "gpt-5.4"
api = "openai-responses"
provider = "openai"
api_key_env = "OPENAI_API_KEY"
base_url = "{base_url}"
capabilities = {{ vision = true, files = true, tools = true, reasoning = true, web_search = false }}

[[models]]
id = "deepseek-v4-pro"
api = "openai"
provider = "deepseek"
api_key_env = "OPENAI_API_KEY"
base_url = "{base_url}"
capabilities = {{ vision = false, files = false, tools = true, reasoning = true, web_search = false }}
"#
        ),
    )
    .expect("write dual-models override");
}

fn configure_moonshot_completions_fixture(fx: &ServeFixture, base_url: &str) {
    let config_path = fx.home_path.join(".tomcat").join("tomcat.config.toml");
    let mut cfg = tomcat::load_config_toml_file(&config_path).expect("load config");
    cfg.llm.default_model = "kimi-k3".to_string();
    cfg.context.compaction_model = "kimi-k3".to_string();
    fs::write(
        &config_path,
        toml::to_string_pretty(&cfg).expect("serialize moonshot config"),
    )
    .expect("persist moonshot config");
    fs::write(
        fx.home_path.join(".tomcat").join("models.toml"),
        format!(
            r#"[[models]]
id = "kimi-k3"
api = "openai"
provider = "moonshot"
api_key_env = "MOONSHOT_API_KEY"
base_url = "{base_url}"
thinking_format = "openai"
supported_reasoning_levels = ["low", "high", "max"]
capabilities = {{ vision = true, files = true, tools = true, reasoning = true, web_search = false }}

[[models]]
id = "utility-flash"
api = "openai"
provider = "openai"
base_url = "http://127.0.0.1:1"
capabilities = {{ vision = false, files = false, tools = false, reasoning = false, web_search = false }}
"#
        ),
    )
    .expect("write moonshot completions models override");
}

fn configure_anthropic_messages_fixture(fx: &ServeFixture, base_url: &str) {
    let config_path = fx.home_path.join(".tomcat").join("tomcat.config.toml");
    let mut cfg = tomcat::load_config_toml_file(&config_path).expect("load config");
    cfg.llm.default_model = "claude-opus-4-8".to_string();
    cfg.context.compaction_model = "claude-opus-4-8".to_string();
    fs::write(
        &config_path,
        toml::to_string_pretty(&cfg).expect("serialize anthropic config"),
    )
    .expect("persist anthropic config");
    fs::write(
        fx.home_path.join(".tomcat").join("models.toml"),
        format!(
            r#"[[models]]
id = "claude-opus-4-8"
api = "anthropic-messages"
provider = "anthropic"
api_key_env = "ANTHROPIC_API_KEY"
base_url = "{base_url}"
thinking_format = "anthropic-adaptive"
supported_reasoning_levels = ["low", "medium", "high", "xhigh", "max"]
capabilities = {{ vision = true, files = true, tools = true, reasoning = true, web_search = false }}

[[models]]
id = "utility-flash"
api = "openai"
provider = "openai"
base_url = "http://127.0.0.1:1"
capabilities = {{ vision = false, files = false, tools = false, reasoning = false, web_search = false }}
"#
        ),
    )
    .expect("write anthropic models override");
}

fn count_event(frames: &[serde_json::Value], event_type: &str) -> usize {
    frames
        .iter()
        .filter(|value| value.get("type").and_then(|v| v.as_str()) == Some(event_type))
        .count()
}

fn first_event_index(frames: &[serde_json::Value], event_type: &str) -> Option<usize> {
    frames
        .iter()
        .position(|value| value.get("type").and_then(|v| v.as_str()) == Some(event_type))
}

fn transcript_entries(fx: &ServeFixture, session_id: &str) -> Vec<serde_json::Value> {
    let config_path = fx.home_path.join(".tomcat").join("tomcat.config.toml");
    let cfg = tomcat::load_config_toml_file(&config_path).expect("load config");
    let path = tomcat::resolve_sessions_dir(&cfg)
        .expect("resolve sessions dir")
        .join(format!("{session_id}.jsonl"));
    fs::read_to_string(path)
        .expect("read transcript")
        .lines()
        .map(|line| serde_json::from_str(line).expect("parse transcript line"))
        .collect()
}

fn non_title_requests(server: &ScriptedOpenAiServer) -> Vec<String> {
    captured_non_title_requests(server)
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

fn anthropic_sse_text_delta(delta: &str) -> ScriptedPart {
    ScriptedPart {
        delay_ms: 0,
        body: format!(
            "event: content_block_delta\ndata: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":\"{delta}\"}}}}\n\n"
        ),
    }
}

fn anthropic_sse_stop() -> Vec<ScriptedPart> {
    vec![
        ScriptedPart {
            delay_ms: 0,
            body: "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}\n\n".to_string(),
        },
        ScriptedPart {
            delay_ms: 0,
            body: "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n".to_string(),
        },
    ]
}

#[test]
#[serial]
fn serve_stdio_user_roundtrip_e2e() {
    common::setup_logging();
    let server = spawn_scripted_openai_stream_server_with_auto_title(vec![response(vec![
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

    let frames = child.recv_until(WAIT_TIMEOUT, |value| {
        value.get("type").and_then(|v| v.as_str()) == Some("agent_idle")
    });
    for value in &frames {
        assert_ndjson_line(value);
    }
    assert_eq!(
        count_event(&frames, "agent_end"),
        1,
        "expected one agent_end: {frames:?}"
    );
    assert_eq!(
        count_event(&frames, "agent_idle"),
        1,
        "expected one agent_idle: {frames:?}"
    );
    assert!(
        first_event_index(&frames, "agent_end") < first_event_index(&frames, "agent_idle"),
        "agent_idle must arrive after agent_end: {frames:?}"
    );
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
    child.send_value(&json!({
        "type": "get_state",
        "id": "state-after-idle",
        "sessionId": session_id
    }));
    let state_frames = child.recv_until(WAIT_TIMEOUT, |value| {
        value.get("id").and_then(|v| v.as_str()) == Some("state-after-idle")
    });
    let state_response = state_frames
        .iter()
        .find(|value| value.get("id").and_then(|v| v.as_str()) == Some("state-after-idle"))
        .expect("state-after-idle response");
    assert_eq!(state_response["payload"]["busy"].as_bool(), Some(false));

    let output = child.wait_for_exit(WAIT_TIMEOUT);
    assert!(
        output.status.success(),
        "serve e2e should exit cleanly: {output:?}"
    );
    assert_eq!(non_title_requests(&server).len(), 1);
}

#[test]
#[serial]
fn serve_interrupt_emits_agent_interrupted_e2e() {
    common::setup_logging();
    let server = spawn_scripted_openai_stream_server_with_auto_title(vec![response(vec![
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
    let mut frames = child.recv_until(WAIT_TIMEOUT, |value| {
        value.get("type").and_then(|v| v.as_str()) == Some("message_update")
    });

    child.send_value(&json!({
        "type": "interrupt",
        "id": "interrupt-1",
        "sessionId": session_id.clone()
    }));
    frames.extend(child.recv_until(WAIT_TIMEOUT, |value| {
        value.get("type").and_then(|v| v.as_str()) == Some("agent_idle")
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
    assert!(frames.iter().any(|value| {
        value.get("type").and_then(|v| v.as_str()) == Some("agent_idle")
            && value.get("sessionId").and_then(|v| v.as_str()) == Some(session_id.as_str())
    }));
    assert!(
        first_event_index(&frames, "agent_interrupted") < first_event_index(&frames, "agent_end")
            && first_event_index(&frames, "agent_end") < first_event_index(&frames, "agent_idle"),
        "interrupt path should settle as agent_interrupted -> agent_end -> agent_idle: {frames:?}"
    );
    child.send_value(&json!({
        "type": "get_state",
        "id": "state-after-interrupt-idle",
        "sessionId": session_id
    }));
    let state_frames = child.recv_until(WAIT_TIMEOUT, |value| {
        value.get("id").and_then(|v| v.as_str()) == Some("state-after-interrupt-idle")
    });
    let state_response = state_frames
        .iter()
        .find(|value| {
            value.get("id").and_then(|v| v.as_str()) == Some("state-after-interrupt-idle")
        })
        .expect("state-after-interrupt-idle response");
    assert_eq!(state_response["payload"]["busy"].as_bool(), Some(false));
}

#[test]
#[serial]
fn serve_stdout_only_emits_ndjson_frames() {
    common::setup_logging();
    let server = spawn_scripted_openai_stream_server_with_auto_title(vec![response(vec![
        sse_delta("ndjson ok"),
        sse_finish("stop"),
        sse_done(),
    ])]);
    let fx = setup_serve_fixture(&server.base_url);
    let mut child = spawn_serve_child(&fx);

    child.send_raw("{not json");
    let parse_error = child.recv_value(WAIT_TIMEOUT);
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
    let frames = child.recv_until(WAIT_TIMEOUT, |value| {
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
    let server = spawn_scripted_openai_stream_server_with_auto_title(vec![response(vec![
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

    let frames = child.recv_until(WAIT_TIMEOUT, |value| {
        value.get("type").and_then(|v| v.as_str()) == Some("agent_idle")
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

    let requests = non_title_requests(&server);
    assert_eq!(requests.len(), 1, "expected one responses API request");
    let body = extract_json_body(&requests[0]);
    let input = body["input"].as_array().expect("responses input array");
    let content = input[0]["content"]
        .as_array()
        .expect("responses content array");
    assert_eq!(content[0]["type"].as_str(), Some("input_text"));
    assert_eq!(content[0]["text"].as_str(), Some("describe attached image"));
    assert_eq!(content[1]["type"].as_str(), Some("input_image"));
    assert_eq!(content[1]["file_id"].as_str(), Some("file-vision"));
}

#[test]
#[serial]
fn serve_prompt_with_inline_file_attachment_roundtrip() {
    common::setup_logging();
    let server = spawn_scripted_openai_stream_server_with_auto_title(vec![response(vec![
        responses_sse_delta("file ok"),
        responses_sse_completed(),
    ])]);
    let fx = setup_serve_fixture(&server.base_url);
    configure_openai_responses_fixture(&fx, &server.base_url);
    let mut child = spawn_serve_child(&fx);
    let session_id = initialize(&mut child);

    child.send_value(&json!({
        "type": "prompt",
        "id": "file-e2e-1",
        "sessionId": session_id,
        "text": "summarize attached file",
        "params": {
            "attachments": [
                {
                    "kind": "file",
                    "filename": "notes.pdf",
                    "mimeType": "application/pdf",
                    "dataBase64": "JVBERi0xLjQK"
                }
            ]
        }
    }));

    let frames = child.recv_until(WAIT_TIMEOUT, |value| {
        value.get("type").and_then(|v| v.as_str()) == Some("agent_end")
    });
    assert!(
        frames.iter().any(|value| {
            value.get("type").and_then(|v| v.as_str()) == Some("message_update")
                && value
                    .get("assistantMessageEvent")
                    .and_then(|v| v.get("delta"))
                    .and_then(|v| v.as_str())
                    == Some("file ok")
        }),
        "expected file attachment prompt to reach agent_end, got {frames:?}"
    );

    let requests = non_title_requests(&server);
    assert_eq!(requests.len(), 1, "expected one responses API request");
    let body = extract_json_body(&requests[0]);
    let input = body["input"].as_array().expect("responses input array");
    let content = input[0]["content"]
        .as_array()
        .expect("responses content array");
    assert_eq!(content[0]["type"].as_str(), Some("input_text"));
    assert_eq!(content[0]["text"].as_str(), Some("summarize attached file"));
    assert_eq!(content[1]["type"].as_str(), Some("input_file"));
    assert_eq!(content[1]["filename"].as_str(), Some("notes.pdf"));
    assert_eq!(
        content[1]["file_data"].as_str(),
        Some("data:application/pdf;base64,JVBERi0xLjQK")
    );
    assert!(content[1].get("file_id").is_none());
}

#[test]
#[serial]
fn serve_prompt_with_moonshot_uploaded_image_uses_ms_scheme() {
    common::setup_logging();
    let server = spawn_scripted_openai_stream_server_with_auto_title(vec![response(vec![
        sse_delta("vision ok"),
        sse_finish("stop"),
        sse_done(),
    ])]);
    let fx = setup_serve_fixture(&server.base_url);
    configure_moonshot_completions_fixture(&fx, &server.base_url);
    let mut child = spawn_serve_child(&fx);
    let session_id = initialize(&mut child);

    child.send_value(&json!({
        "type": "prompt",
        "id": "moonshot-img-1",
        "sessionId": session_id,
        "text": "describe uploaded image",
        "params": {
            "attachments": [
                {
                    "kind": "image",
                    "fileId": "file-vision"
                }
            ]
        }
    }));

    let frames = child.recv_until(WAIT_TIMEOUT, |value| {
        value.get("type").and_then(|v| v.as_str()) == Some("agent_idle")
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
        "expected moonshot attachment reply, got {frames:?}"
    );

    let requests = non_title_requests(&server);
    assert_eq!(
        requests.len(),
        1,
        "expected one moonshot completions request"
    );
    let body = extract_json_body(&requests[0]);
    let messages = body["messages"].as_array().expect("messages array");
    let user = messages
        .iter()
        .find(|message| message["role"].as_str() == Some("user"))
        .unwrap_or_else(|| panic!("expected user message in body={body:?}"));
    let content = user["content"]
        .as_array()
        .unwrap_or_else(|| panic!("content array, got body={body:?}"));
    assert_eq!(content[0]["type"].as_str(), Some("text"));
    assert_eq!(content[1]["type"].as_str(), Some("image_url"));
    assert_eq!(
        content[1]["image_url"]["url"].as_str(),
        Some("ms://file-vision")
    );
    assert!(content[1]["image_url"].get("file_id").is_none());
}

#[test]
#[serial]
fn serve_prompt_with_anthropic_uploaded_pdf_uses_source_file_and_beta_header() {
    common::setup_logging();
    let mut scripted = vec![anthropic_sse_text_delta("anthropic ok")];
    scripted.extend(anthropic_sse_stop());
    let server = spawn_scripted_openai_stream_server_with_auto_title(vec![response(scripted)]);
    let fx = setup_serve_fixture(&server.base_url);
    configure_anthropic_messages_fixture(&fx, &server.base_url);
    let mut child = spawn_serve_child(&fx);
    let session_id = initialize(&mut child);

    child.send_value(&json!({
        "type": "prompt",
        "id": "anthropic-file-1",
        "sessionId": session_id,
        "text": "summarize uploaded file",
        "params": {
            "attachments": [
                {
                    "kind": "file",
                    "filename": "notes.pdf",
                    "fileId": "file-pdf"
                }
            ]
        }
    }));

    let frames = child.recv_until(WAIT_TIMEOUT, |value| {
        value.get("type").and_then(|v| v.as_str()) == Some("agent_idle")
    });
    assert!(
        frames.iter().any(|value| {
            value.get("type").and_then(|v| v.as_str()) == Some("message_update")
                && value
                    .get("assistantMessageEvent")
                    .and_then(|v| v.get("delta"))
                    .and_then(|v| v.as_str())
                    == Some("anthropic ok")
        }),
        "expected anthropic attachment reply, got {frames:?}"
    );

    let requests = non_title_requests(&server);
    assert_eq!(requests.len(), 1, "expected one anthropic request");
    let raw = &requests[0];
    assert!(
        raw.to_ascii_lowercase()
            .contains("anthropic-beta: files-api-2025-04-14"),
        "uploaded anthropic file request should carry files beta header: {raw}"
    );
    let body = extract_json_body(raw);
    let messages = body["messages"].as_array().expect("messages array");
    let user = messages
        .iter()
        .find(|message| message["role"].as_str() == Some("user"))
        .unwrap_or_else(|| panic!("expected user message in body={body:?}"));
    let content = user["content"]
        .as_array()
        .unwrap_or_else(|| panic!("content array, got body={body:?}"));
    assert_eq!(content[0]["type"].as_str(), Some("text"));
    assert_eq!(content[1]["type"].as_str(), Some("document"));
    assert_eq!(content[1]["source"]["type"].as_str(), Some("file"));
    assert_eq!(content[1]["source"]["file_id"].as_str(), Some("file-pdf"));
    assert_eq!(content[1]["title"].as_str(), Some("notes.pdf"));
}

#[test]
#[serial]
fn serve_prompt_with_context_reference_segments_roundtrip() {
    common::setup_logging();
    let server = spawn_scripted_openai_stream_server_with_auto_title(vec![response(vec![
        responses_sse_delta("context ok"),
        responses_sse_completed(),
    ])]);
    let fx = setup_serve_fixture(&server.base_url);
    configure_openai_responses_fixture(&fx, &server.base_url);
    let mut child = spawn_serve_child(&fx);
    let session_id = initialize(&mut child);

    child.send_value(&json!({
        "type": "prompt",
        "id": "refs-e2e-1",
        "sessionId": session_id,
        "text": "",
        "params": {
            "segments": [
                {
                    "type": "text",
                    "text": "before "
                },
                {
                    "type": "reference",
                    "kind": "selection",
                    "path": "src/lib.rs",
                    "label": "lib.rs:10-12",
                    "lineStart": 10,
                    "lineEnd": 12,
                    "text": "fn hello() {}"
                },
                {
                    "type": "text",
                    "text": " after "
                },
                {
                    "type": "reference",
                    "kind": "file",
                    "path": "docs/guide.md",
                    "label": "guide.md"
                }
            ]
        }
    }));

    let frames = child.recv_until(WAIT_TIMEOUT, |value| {
        value.get("type").and_then(|v| v.as_str()) == Some("agent_end")
    });
    assert!(
        frames.iter().any(|value| {
            value.get("type").and_then(|v| v.as_str()) == Some("message_update")
                && value
                    .get("assistantMessageEvent")
                    .and_then(|v| v.get("delta"))
                    .and_then(|v| v.as_str())
                    == Some("context ok")
        }),
        "expected reference prompt to reach agent_end, got {frames:?}"
    );
    assert!(
        frames
            .iter()
            .any(|value| value.get("type").and_then(|v| v.as_str()) == Some("agent_end")),
        "expected agent_end before reading transcript, got {frames:?}"
    );

    child.send_value(&json!({
        "type": "get_state",
        "id": "state-context-reference",
        "sessionId": session_id
    }));
    let state_frames = child.recv_until(WAIT_TIMEOUT, |value| {
        value.get("id").and_then(|v| v.as_str()) == Some("state-context-reference")
    });
    let state_response = state_frames
        .iter()
        .find(|value| value.get("id").and_then(|v| v.as_str()) == Some("state-context-reference"))
        .expect("state-context-reference response");
    let transcript_session_id = state_response["payload"]["sessionId"]
        .as_str()
        .or_else(|| state_response["sessionId"].as_str())
        .unwrap_or(session_id.as_str());

    let requests = non_title_requests(&server);
    assert_eq!(requests.len(), 1, "expected one responses API request");
    let body = extract_json_body(&requests[0]);
    let input = body["input"].as_array().expect("responses input array");
    let content = input[0]["content"]
        .as_array()
        .expect("responses content array");
    assert_eq!(content.len(), 1);
    assert_eq!(content[0]["type"].as_str(), Some("input_text"));
    assert_eq!(
        content[0]["text"].as_str(),
        Some(
            "before <selection file=\"src/lib.rs\" lines=\"10-12\">\nfn hello() {}\n</selection> after [file reference] docs/guide.md"
        )
    );

    let transcript = transcript_entries(&fx, transcript_session_id);
    let user_entry = transcript
        .iter()
        .rev()
        .find(|entry| {
            entry.get("type").and_then(|value| value.as_str()) == Some("message")
                && entry
                    .get("message")
                    .and_then(|message| message.get("role"))
                    .and_then(|value| value.as_str())
                    == Some("user")
        })
        .expect("latest user transcript entry");
    let parts = user_entry["message"]["content"]
        .as_array()
        .expect("user transcript content parts");
    assert_eq!(parts[0]["type"].as_str(), Some("input_text"));
    assert_eq!(parts[0]["text"].as_str(), Some("before "));
    assert_eq!(parts[1]["type"].as_str(), Some("input_reference"));
    assert_eq!(parts[1]["ref_kind"].as_str(), Some("selection"));
    assert_eq!(parts[1]["path"].as_str(), Some("src/lib.rs"));
    assert_eq!(parts[1]["line_start"].as_u64(), Some(10));
    assert_eq!(parts[1]["line_end"].as_u64(), Some(12));
    assert_eq!(parts[1]["text"].as_str(), Some("fn hello() {}"));
    assert_eq!(parts[2]["type"].as_str(), Some("input_text"));
    assert_eq!(parts[2]["text"].as_str(), Some(" after "));
    assert_eq!(parts[3]["type"].as_str(), Some("input_reference"));
    assert_eq!(parts[3]["ref_kind"].as_str(), Some("file"));
    assert_eq!(parts[3]["path"].as_str(), Some("docs/guide.md"));
    assert_eq!(parts[3]["label"].as_str(), Some("guide.md"));
}

#[test]
#[serial]
fn serve_prompt_with_non_pdf_file_attachment_returns_error() {
    common::setup_logging();
    let server = spawn_scripted_openai_stream_server(vec![]);
    let fx = setup_serve_fixture(&server.base_url);
    configure_openai_responses_fixture(&fx, &server.base_url);
    let mut child = spawn_serve_child(&fx);
    let session_id = initialize(&mut child);

    child.send_value(&json!({
        "type": "prompt",
        "id": "file-e2e-bad-mime",
        "sessionId": session_id,
        "text": "summarize attached file",
        "params": {
            "attachments": [
                {
                    "kind": "file",
                    "filename": "notes.md",
                    "mimeType": "text/markdown",
                    "dataBase64": "IyBoaQ=="
                }
            ]
        }
    }));

    let frames = child.recv_until(WAIT_TIMEOUT, |value| {
        value.get("id").and_then(|v| v.as_str()) == Some("file-e2e-bad-mime")
    });
    let response = frames
        .iter()
        .find(|value| value.get("id").and_then(|v| v.as_str()) == Some("file-e2e-bad-mime"))
        .expect("bad mime response");
    assert_eq!(response["success"].as_bool(), Some(false));
    assert_eq!(
        response["error"].as_str(),
        Some(
            "invalid_attachment: file attachments only support application/pdf; use kind=image for images (got text/markdown)"
        )
    );
    assert_eq!(
        non_title_requests(&server).len(),
        0,
        "non-pdf file attachments should not reach the responses API"
    );
}

#[test]
#[serial]
fn serve_prompt_with_attachment_history_then_deepseek_degrades_history_and_succeeds() {
    common::setup_logging();
    let server = spawn_scripted_openai_stream_server_with_auto_title(vec![
        response(vec![
            responses_sse_delta("vision ok"),
            responses_sse_completed(),
        ]),
        response(vec![
            responses_sse_delta("pdf ok"),
            responses_sse_completed(),
        ]),
        response(vec![
            sse_delta("history ok"),
            sse_finish("stop"),
            sse_done(),
        ]),
    ]);
    let fx = setup_serve_fixture(&server.base_url);
    configure_multimodal_history_fixture(&fx, &server.base_url);
    let mut child = spawn_serve_child(&fx);
    let session_id = initialize(&mut child);

    child.send_value(&json!({
        "type": "prompt",
        "id": "hist-1",
        "sessionId": session_id,
        "text": "describe image",
        "params": {
            "attachments": [
                {
                    "kind": "image",
                    "fileId": "file-vision"
                }
            ]
        }
    }));
    let first_frames = child.recv_until(WAIT_TIMEOUT, |value| {
        value.get("type").and_then(|v| v.as_str()) == Some("agent_idle")
    });
    assert_eq!(count_event(&first_frames, "agent_end"), 1);

    child.send_value(&json!({
        "type": "prompt",
        "id": "hist-2",
        "sessionId": session_id,
        "text": "summarize pdf",
        "params": {
            "attachments": [
                {
                    "kind": "file",
                    "filename": "notes.pdf",
                    "mimeType": "application/pdf",
                    "dataBase64": "JVBERi0xLjQK"
                }
            ]
        }
    }));
    let second_history_frames = child.recv_until(WAIT_TIMEOUT, |value| {
        value.get("type").and_then(|v| v.as_str()) == Some("agent_idle")
    });
    assert_eq!(count_event(&second_history_frames, "agent_end"), 1);

    child.send_value(&json!({
        "type": "set_model",
        "id": "set-deepseek",
        "sessionId": session_id,
        "model": "deepseek-v4-pro"
    }));
    let set_model_frames = child.recv_until(WAIT_TIMEOUT, |value| {
        value.get("id").and_then(|v| v.as_str()) == Some("set-deepseek")
    });
    let set_model_response = set_model_frames
        .iter()
        .find(|value| value.get("id").and_then(|v| v.as_str()) == Some("set-deepseek"))
        .expect("set_model response");
    assert_eq!(set_model_response["success"].as_bool(), Some(true));

    child.send_value(&json!({
        "type": "prompt",
        "id": "hist-3",
        "sessionId": session_id,
        "text": "follow up",
        "params": {}
    }));
    let second_frames = child.recv_until(WAIT_TIMEOUT, |value| {
        value.get("type").and_then(|v| v.as_str()) == Some("agent_idle")
    });
    assert_eq!(count_event(&second_frames, "agent_end"), 1);
    assert!(
        second_frames.iter().all(|value| {
            value.get("type").and_then(|v| v.as_str()) != Some("agent_end")
                || value.get("error").and_then(|v| v.as_str()).is_none()
        }),
        "history downgrade should avoid terminal errors: {second_frames:?}"
    );

    let requests = non_title_requests(&server);
    assert_eq!(requests.len(), 3, "expected three upstream requests");
    assert!(
        requests[2].contains("/v1/chat/completions"),
        "third request should switch to chat completions path: {:?}",
        requests[2]
    );
    let body = extract_json_body(&requests[2]);
    let messages = body["messages"].as_array().expect("completions messages");
    let flatten_content_text = |message: &Value| -> String {
        let Some(content) = message.get("content") else {
            return String::new();
        };
        if let Some(text) = content.as_str() {
            return text.to_owned();
        }
        content
            .as_array()
            .map(|parts| {
                parts
                    .iter()
                    .filter_map(|part| part.get("text").and_then(|value| value.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default()
    };
    assert!(
        messages.iter().any(|message| {
            let text = flatten_content_text(message);
            text.contains("[图片已省略：当前模型不支持图片输入]") && text.contains("describe image")
        }),
        "third request should carry a downgraded image placeholder instead of raw image input: {messages:?}"
    );
    assert!(
        messages.iter().any(|message| {
            let text = flatten_content_text(message);
            text.contains("[文件已省略：当前模型不支持文件输入]") && text.contains("summarize pdf")
        }),
        "third request should carry a downgraded file placeholder instead of raw file input: {messages:?}"
    );
}

#[test]
#[serial]
fn serve_prompt_with_inline_file_attachment_missing_filename_returns_error() {
    common::setup_logging();
    let fx = setup_serve_fixture("http://127.0.0.1:1");
    let mut child = spawn_serve_child(&fx);
    let session_id = initialize(&mut child);

    child.send_value(&json!({
        "type": "prompt",
        "id": "file-e2e-missing-name",
        "sessionId": session_id,
        "text": "summarize attached file",
        "params": {
            "attachments": [
                {
                    "kind": "file",
                    "mimeType": "application/pdf",
                    "dataBase64": "JVBERi0xLjQK"
                }
            ]
        }
    }));

    let frames = child.recv_until(WAIT_TIMEOUT, |value| {
        value.get("id").and_then(|v| v.as_str()) == Some("file-e2e-missing-name")
    });
    let response = frames
        .iter()
        .find(|value| value.get("id").and_then(|v| v.as_str()) == Some("file-e2e-missing-name"))
        .expect("missing filename response");
    assert_eq!(response["success"].as_bool(), Some(false));
    assert_eq!(
        response["error"].as_str(),
        Some("invalid_attachment: file attachment requires filename")
    );
}
