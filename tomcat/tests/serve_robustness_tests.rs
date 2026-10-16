mod common;

use std::time::Duration;

use serde_json::json;
use serial_test::serial;

use common::serve::{setup_serve_fixture, spawn_scripted_openai_stream_server, spawn_serve_child};

// `tomcat serve` loads embedded assets before processing stdin. Match the other
// real-stdio suite's budget so this remains a correctness wait, not a startup
// performance assertion under the default four-way integration gate.
const SERVE_TIMEOUT: Duration = Duration::from_secs(10);

#[test]
#[serial]
fn serve_with_removed_single_model_config_stays_available_without_rewriting_it() {
    let fx = common::serve::setup_cold_serve_fixture(false);
    let path = fx.home_path.join(".tomcat/tomcat.config.toml");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let old = "[llm]\nprovider='openai'\napi_base='https://example.test'\napi_key_env='REMOVED_FAKE_KEY'\n[primitive]\npath_whitelist=[]\n[tools.bash]\ntimeout_ms=120000\n";
    std::fs::write(&path, old).unwrap();
    let mut child = common::serve::spawn_serve_child_with_env(&fx, &[("RUST_LOG", "warn")]);
    let session = initialize_cold(&mut child);
    let state = request(&mut child, json!({"type":"get_state", "sessionId":session}));
    assert_eq!(
        state["modelReady"], false,
        "old connection fields are not migrated into a model"
    );
    let models = request(&mut child, json!({"type":"list_models"}));
    assert_eq!(models["defaultModelId"], "gpt-5.4");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), old);
    let output = child.wait_for_exit(SERVE_TIMEOUT);
    assert!(output.status.success());
    let logs = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        logs.matches("ignored unsupported config keys").count(),
        1,
        "{logs}"
    );
    for key in [
        "llm.provider",
        "llm.api_base",
        "llm.api_key_env",
        "primitive.path_whitelist",
        "tools.bash.timeout_ms",
    ] {
        assert!(logs.contains(key), "{logs}");
    }
    assert!(
        !logs.contains("REMOVED_FAKE_KEY"),
        "only key paths may be logged, never their values"
    );
}

#[test]
#[serial]
fn serve_cold_start_without_init_keeps_management_available() {
    assert_cold_start_management(false);
}

#[test]
#[serial]
fn serve_cold_start_after_skipping_key_keeps_management_available() {
    assert_cold_start_management(true);
}

fn assert_cold_start_management(initialized: bool) {
    let fx = common::serve::setup_cold_serve_fixture(initialized);
    let config_path = fx.home_path.join(".tomcat/tomcat.config.toml");
    let before = std::fs::read(&config_path).ok();
    assert_eq!(before.is_some(), initialized);
    let mut child = spawn_serve_child(&fx);
    child.send_value(&json!({
        "type": "control_request", "requestId": "cold-init",
        "subtype": "initialize", "payload": {}
    }));
    let init = child.recv_until(SERVE_TIMEOUT, |v| v["requestId"] == "cold-init");
    let session_id = init.last().unwrap()["payload"]["sessionId"]
        .as_str()
        .unwrap();
    child.send_value(&json!({"type": "get_state", "id": "cold-state", "sessionId": session_id}));
    let state = child.recv_until(SERVE_TIMEOUT, |v| v["id"] == "cold-state");
    let state = state.last().unwrap();
    assert_eq!(state["success"], true, "{state}");
    assert_eq!(state["payload"]["modelReady"], false);
    child.send_value(&json!({"type": "list_models", "id": "cold-models"}));
    let models = child.recv_until(SERVE_TIMEOUT, |v| v["id"] == "cold-models");
    assert_eq!(models.last().unwrap()["success"], true);
    assert_eq!(
        models.last().unwrap()["payload"]["defaultModelId"],
        "gpt-5.4"
    );
    assert_eq!(
        std::fs::read(&config_path).ok(),
        before,
        "startup must not write configuration"
    );
    // A stale GUI may submit anyway: the existing inference path rejects it without
    // sending HTTP or taking down the management connection.
    let server = spawn_scripted_openai_stream_server(vec![]);
    upsert_local_model(&mut child, &server.base_url);
    request(
        &mut child,
        json!({"type":"set_model", "model":"local-deepseek", "sessionId":session_id}),
    );
    let blocked = request(
        &mut child,
        json!({"type":"get_state", "sessionId":session_id}),
    );
    assert_eq!(blocked["modelReady"], false);
    child.send_value(&json!({"type":"prompt", "sessionId":session_id, "text":"stale readiness submission", "params":{}}));
    let frames = child.recv_until(SERVE_TIMEOUT, |v| v["type"] == "agent_idle");
    assert!(
        frames
            .iter()
            .any(|v| v["type"] == "agent_end" && v.to_string().contains("LOCAL_DEEPSEEK_API_KEY")),
        "{frames:?}"
    );
    assert!(common::serve::captured_non_title_requests(&server).is_empty());
    child.send_value(
        &json!({"type":"set_default_model", "id":"invalid-default", "modelId":"not-in-catalog"}),
    );
    let rejected = child.recv_until(SERVE_TIMEOUT, |v| v["id"] == "invalid-default");
    assert_eq!(rejected.last().unwrap()["success"], false);
    assert_eq!(
        std::fs::read(&config_path).ok(),
        before,
        "invalid default must not create or replace configuration"
    );
    request(&mut child, json!({"type":"list_models"}));
    assert!(child.wait_for_exit(SERVE_TIMEOUT).status.success());
}

fn request(
    child: &mut common::serve::ServeChild,
    mut command: serde_json::Value,
) -> serde_json::Value {
    command["id"] = json!("setup-operation");
    child.send_value(&command);
    let frames = child.recv_until(SERVE_TIMEOUT, |v| v["id"] == "setup-operation");
    let reply = frames.last().unwrap();
    assert_eq!(reply["success"], true, "{reply}");
    reply["payload"].clone()
}

fn initialize_cold(child: &mut common::serve::ServeChild) -> String {
    child.send_value(&json!({"type":"control_request", "requestId":"setup-init", "subtype":"initialize", "payload":{}}));
    let frames = child.recv_until(SERVE_TIMEOUT, |v| v["requestId"] == "setup-init");
    frames.last().unwrap()["payload"]["sessionId"]
        .as_str()
        .unwrap()
        .to_string()
}

fn upsert_local_model(child: &mut common::serve::ServeChild, base_url: &str) {
    request(
        child,
        json!({"type":"upsert_model", "model":{
            "id":"local-deepseek", "modelName":"fixture-model", "api":"openai", "provider":"deepseek",
            "apiKeyEnv":"LOCAL_DEEPSEEK_API_KEY", "baseUrl":base_url,
            "capabilities":{"vision":false, "files":false, "tools":true, "reasoning":false, "web_search":false}
        }}),
    );
}

fn configure_local_model(child: &mut common::serve::ServeChild, base_url: &str) {
    upsert_local_model(child, base_url);
    request(
        child,
        json!({"type":"set_provider_key", "envName":"LOCAL_DEEPSEEK_API_KEY", "value":"fixture-only-key"}),
    );
    let result = request(
        child,
        json!({"type":"set_default_model", "modelId":"local-deepseek"}),
    );
    assert_eq!(result["warnings"], json!([]));
}

#[test]
#[serial]
fn serve_cold_start_configuration_is_live_persistent_and_respects_environment() {
    assert_live_configuration(false);
}

#[test]
#[serial]
fn serve_skipped_key_configuration_is_live_persistent_and_respects_environment() {
    assert_live_configuration(true);
}

fn assert_live_configuration(initialized: bool) {
    use common::serve::*;
    let server = spawn_scripted_openai_stream_server_with_auto_title(vec![response(vec![
        sse_delta("configured-without-restart"),
        sse_finish("stop"),
        sse_done(),
    ])]);
    let fx = setup_cold_serve_fixture(initialized);
    let mut child = spawn_serve_child(&fx);
    let pid = child.pid();
    let session_id = initialize_cold(&mut child);
    // Choosing another model without its credentials is a management operation, not inference.
    request(
        &mut child,
        json!({"type":"set_model", "model":"deepseek-v4-pro", "sessionId":session_id}),
    );
    assert_eq!(
        request(
            &mut child,
            json!({"type":"get_state", "sessionId":session_id})
        )["model"],
        "deepseek-v4-pro"
    );
    // A fresh session follows the new default; an explicit override is intentionally retained.
    let fresh = request(&mut child, json!({"type":"new_session", "params":{}}))["sessionId"]
        .as_str()
        .unwrap()
        .to_string();
    configure_local_model(&mut child, &server.base_url);
    assert_eq!(
        request(&mut child, json!({"type":"list_models"}))["defaultModelId"],
        "local-deepseek"
    );
    for command in [
        json!({"type":"list_models"}),
        json!({"type":"set_provider_key", "envName":"LOCAL_DEEPSEEK_API_KEY", "value":"fixture-only-key"}),
    ] {
        request(&mut child, command);
        let state = request(&mut child, json!({"type":"get_state", "sessionId":fresh}));
        assert_eq!(state["model"], "local-deepseek");
        assert_eq!(state["modelReady"], true);
    }
    request(
        &mut child,
        json!({"type":"set_ui_language", "language":"en"}),
    );
    assert_eq!(
        request(
            &mut child,
            json!({"type":"get_state", "sessionId":session_id})
        )["model"],
        "deepseek-v4-pro",
        "default changes must preserve explicit session choices"
    );
    child.send_value(&json!({"type":"prompt", "id":"local-prompt", "sessionId":fresh, "text":"hello local fixture", "params":{}}));
    let frames = child.recv_until(SERVE_TIMEOUT, |v| v["type"] == "agent_idle");
    assert!(
        frames
            .iter()
            .any(|v| v["assistantMessageEvent"]["delta"] == "configured-without-restart"),
        "{frames:?}"
    );
    assert_eq!(child.pid(), pid);
    let requests = captured_non_title_requests(&server);
    assert_eq!(requests.len(), 1);
    assert_eq!(extract_json_body(&requests[0])["model"], "fixture-model");
    let newer = request(&mut child, json!({"type":"new_session", "params":{}}))["sessionId"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        request(&mut child, json!({"type":"get_state", "sessionId":newer}))["model"],
        "local-deepseek"
    );
    assert!(child.wait_for_exit(SERVE_TIMEOUT).status.success());
    let config_path = fx.home_path.join(".tomcat/tomcat.config.toml");
    let saved = std::fs::read_to_string(&config_path).unwrap();
    assert!(saved.contains("local-deepseek") && saved.contains("language"));
    assert!(!saved.contains("fixture-only-key"));
    let mut restarted = spawn_serve_child(&fx);
    let session = initialize_cold(&mut restarted);
    assert_eq!(
        request(
            &mut restarted,
            json!({"type":"get_state", "sessionId":session})
        )["modelReady"],
        true
    );
    assert!(restarted.wait_for_exit(SERVE_TIMEOUT).status.success());
    let mut overridden =
        spawn_serve_child_with_env(&fx, &[("TOMCAT__LLM__DEFAULT_MODEL", "gpt-5.4")]);
    let session = initialize_cold(&mut overridden);
    let selected = request(
        &mut overridden,
        json!({"type":"set_default_model", "modelId":"local-deepseek"}),
    );
    assert_eq!(selected["envOverride"], true);
    assert_eq!(
        request(&mut overridden, json!({"type":"list_models"}))["defaultModelId"],
        "gpt-5.4"
    );
    request(
        &mut overridden,
        json!({"type":"upsert_model", "model":{
            "id":"unused-local", "api":"openai", "provider":"deepseek", "baseUrl":server.base_url,
            "apiKeyEnv":"LOCAL_DEEPSEEK_API_KEY"
        }}),
    );
    request(
        &mut overridden,
        json!({"type":"remove_model", "modelId":"unused-local"}),
    );
    let state = request(
        &mut overridden,
        json!({"type":"get_state", "sessionId":session}),
    );
    assert_eq!(state["model"], "gpt-5.4");
    assert_eq!(state["modelReady"], false);
    assert!(overridden.wait_for_exit(SERVE_TIMEOUT).status.success());
    assert_eq!(std::fs::read_to_string(config_path).unwrap(), saved);
}

#[test]
#[serial]
fn serve_cold_start_selected_model_with_key_is_ready_without_default() {
    assert_selected_model_ready(false);
}

#[test]
#[serial]
fn serve_skipped_key_selected_model_with_key_is_ready_without_default() {
    assert_selected_model_ready(true);
}

// Compaction falls back to the session model, so choosing a keyed model is enough to send.
fn assert_selected_model_ready(initialized: bool) {
    use common::serve::*;
    let server = spawn_scripted_openai_stream_server_with_auto_title(vec![response(vec![
        sse_delta("selected-model-ready"),
        sse_finish("stop"),
        sse_done(),
    ])]);
    let fx = setup_cold_serve_fixture(initialized);
    let mut child = spawn_serve_child(&fx);
    let session = initialize_cold(&mut child);
    upsert_local_model(&mut child, &server.base_url);
    request(
        &mut child,
        json!({"type":"set_provider_key", "envName":"LOCAL_DEEPSEEK_API_KEY", "value":"fixture-only-key"}),
    );
    assert_eq!(
        request(&mut child, json!({"type":"get_state", "sessionId":session}))["modelReady"],
        false
    );
    request(
        &mut child,
        json!({"type":"set_model", "model":"local-deepseek", "sessionId":session}),
    );
    assert_eq!(
        request(&mut child, json!({"type":"get_state", "sessionId":session}))["modelReady"],
        true
    );
    child.send_value(
        &json!({"type":"prompt", "sessionId":session, "text":"hello selected model", "params":{}}),
    );
    let frames = child.recv_until(SERVE_TIMEOUT, |v| v["type"] == "agent_idle");
    assert!(
        frames
            .iter()
            .any(|v| v["assistantMessageEvent"]["delta"] == "selected-model-ready"),
        "{frames:?}"
    );
    let requests = captured_non_title_requests(&server);
    assert_eq!(requests.len(), 1);
    assert_eq!(extract_json_body(&requests[0])["model"], "fixture-model");
    assert!(child.wait_for_exit(SERVE_TIMEOUT).status.success());
}

#[test]
#[serial]
fn serve_unavailable_default_models_keep_history_and_management_available() {
    use common::serve::*;
    for model in ["", "model-no-longer-in-catalog"] {
        let fx = setup_cold_serve_fixture(false);
        let mut child = spawn_serve_child_with_env(&fx, &[("TOMCAT__LLM__DEFAULT_MODEL", model)]);
        let session = initialize_cold(&mut child);
        let state = request(&mut child, json!({"type":"get_state", "sessionId":session}));
        assert_eq!(state["model"], model);
        assert_eq!(state["modelReady"], false);
        request(
            &mut child,
            json!({"type":"get_messages", "sessionId":session}),
        );
        request(&mut child, json!({"type":"list_models"}));
        assert!(child.wait_for_exit(SERVE_TIMEOUT).status.success());
    }
}

#[test]
#[serial]
fn serve_cold_start_provider_401_is_not_an_initialization_failure() {
    use common::serve::*;
    let server = spawn_unauthorized_openai_server();
    let fx = setup_cold_serve_fixture(false);
    let mut child = spawn_serve_child(&fx);
    let session = initialize_cold(&mut child);
    configure_local_model(&mut child, &server.base_url);
    assert_eq!(
        request(&mut child, json!({"type":"get_state", "sessionId":session}))["modelReady"],
        true
    );
    child.send_value(&json!({"type":"prompt", "id":"unauthorized-prompt", "sessionId":session, "text":"hello", "params":{}}));
    let frames = child.recv_until(SERVE_TIMEOUT, |v| v["type"] == "agent_idle");
    assert!(
        frames.iter().any(|v| v.to_string().contains("401")),
        "{frames:?}"
    );
    assert!(!captured_non_title_requests(&server).is_empty());
    assert_eq!(
        request(&mut child, json!({"type":"get_state", "sessionId":session}))["modelReady"],
        true
    );
    request(&mut child, json!({"type":"list_models"}));
    assert!(child.wait_for_exit(SERVE_TIMEOUT).status.success());
}

#[test]
#[serial]
fn serve_parse_error_does_not_break_following_initialize() {
    common::setup_logging();
    let server = spawn_scripted_openai_stream_server(vec![]);
    let fx = setup_serve_fixture(&server.base_url);
    let mut child = spawn_serve_child(&fx);

    child.send_raw("{not json");
    let parse_error = child.recv_value(SERVE_TIMEOUT);
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
    let init = child.recv_until(SERVE_TIMEOUT, |value| {
        value.get("type").and_then(|v| v.as_str()) == Some("control_response")
            && value.get("requestId").and_then(|v| v.as_str()) == Some("init-1")
    });
    assert_eq!(
        init.last()
            .and_then(|value| value["payload"]["protocolVersion"].as_i64()),
        Some(2)
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
    let response = child.recv_value(SERVE_TIMEOUT);
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
    let response = child.recv_value(SERVE_TIMEOUT);
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
    let init = child.recv_until(SERVE_TIMEOUT, |value| {
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
    let response = child.recv_value(SERVE_TIMEOUT);
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
    let state = child.recv_until(SERVE_TIMEOUT, |value| {
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
    assert_eq!(
        parsed["models"]["gpt-5.4"]["reasoning"].as_str(),
        Some("high")
    );
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
    let _ = child.recv_until(SERVE_TIMEOUT, |value| {
        value.get("type").and_then(|v| v.as_str()) == Some("control_response")
            && value.get("requestId").and_then(|v| v.as_str()) == Some("init-1")
    });

    let output = child.wait_for_exit(SERVE_TIMEOUT);
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
