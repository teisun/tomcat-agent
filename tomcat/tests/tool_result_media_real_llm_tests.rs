//! Manual relay matrix; raw HTTP/SSE, deliberately bypassing Tomcat's providers.
//! cargo test --test tool_result_media_real_llm_tests -- --ignored --nocapture --test-threads=1
//! Optional output: TOMCAT_MEDIA_PROBE_OUTPUT=/absolute/path/matrix.jsonl.
//! Overrides: TOMCAT_MEDIA_PROBE_<RELAY>_<ANTHROPIC|RESPONSES|CHAT>_{MODEL,BASE_URL,KEY_ENV}.
mod common;

use base64::Engine as _;
use serde::Serialize;
use serde_json::{json, Value};
use serial_test::serial;
use std::{io::Write, time::Duration};
use tomcat::core::llm::openai_files::{FilePurpose, FilesApiProviderContext, OpenAiFilesClient};

const IMAGE: &str = include_str!("fixtures/llm_multimodal/sample_image_b64.txt");
const PROMPT: &str = "Call take_screenshot, then answer with one English word: what animal is in the returned image? If there is no visible image, answer unavailable. Do not guess.";
const RESULT: &str = "Screenshot captured.";
const CALL_ID: &str = "call_probe_1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Api {
    Anthropic,
    Responses,
    Chat,
}
impl Api {
    fn label(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::Responses => "responses",
            Self::Chat => "chat",
        }
    }
    fn route(self) -> &'static str {
        match self {
            Self::Anthropic => "messages",
            Self::Responses => "responses",
            Self::Chat => "chat/completions",
        }
    }
}
#[derive(Clone, Copy, Debug)]
enum Shape {
    Native,
    Split,
    NativeFileId,
    SplitFileId,
}
impl Shape {
    fn native(self) -> bool {
        matches!(self, Self::Native | Self::NativeFileId)
    }
    fn file_id(self) -> bool {
        matches!(self, Self::NativeFileId | Self::SplitFileId)
    }
    fn label(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Split => "split",
            Self::NativeFileId => "native-file-id",
            Self::SplitFileId => "split-file-id",
        }
    }
}
struct Target {
    relay: &'static str,
    model: String,
    base: String,
    key_env: String,
}
fn target(api: Api, relay: &'static str) -> Target {
    let (model, base, key) = match (api, relay) {
        (Api::Anthropic, "fcodex") => (
            "claude-opus-5".into(),
            common::fcodex_test_base_url(),
            common::FCODEX_ANTHROPIC_TEST_API_KEY_ENV,
        ),
        (_, "fcodex") => (
            "gpt-6.1-sol".into(),
            common::fcodex_test_base_url(),
            common::FCODEX_TEST_API_KEY_ENV,
        ),
        (_, "idatatlas") => (
            "gpt-6.1-sol".into(),
            common::IDATATLAS_TEST_BASE_URL.into(),
            common::IDATATLAS_TEST_API_KEY_ENV,
        ),
        (_, "kimi") => (
            common::kimi_test_model(),
            common::kimi_test_base_url(),
            common::KIMI_TEST_API_KEY_ENV,
        ),
        _ => unreachable!("unknown probe target"),
    };
    let prefix = format!(
        "TOMCAT_MEDIA_PROBE_{}_{}",
        relay.to_ascii_uppercase(),
        api.label().to_ascii_uppercase()
    );
    Target {
        relay,
        model: std::env::var(format!("{prefix}_MODEL")).unwrap_or(model),
        base: std::env::var(format!("{prefix}_BASE_URL")).unwrap_or(base),
        key_env: std::env::var(format!("{prefix}_KEY_ENV")).unwrap_or_else(|_| key.into()),
    }
}
fn endpoint(api: Api, base: &str) -> String {
    let url = reqwest::Url::parse(base).expect("valid probe base URL");
    let prefix = if url.path().trim_matches('/').is_empty() {
        "/v1"
    } else {
        ""
    };
    format!("{}{prefix}/{}", base.trim_end_matches('/'), api.route())
}
fn payload(api: Api, target: &Target, shape: Shape, file_id: Option<&str>) -> Value {
    let parameters = json!({"type":"object","properties":{},"additionalProperties":false});
    match api {
        Api::Anthropic => {
            let image = json!({"type":"image","source":{"type":"base64","media_type":"image/png","data":IMAGE.trim()}});
            let mut content = vec![json!({"type":"tool_result","tool_use_id":CALL_ID,"content":
                if shape.native() { json!([{"type":"text","text":RESULT},image.clone()]) } else { json!(RESULT) }})];
            if !shape.native() {
                content.push(image);
            }
            json!({"model":target.model,"stream":true,"max_tokens":512,"thinking":{"type":"disabled"},
                "tools":[{"name":"take_screenshot","description":"Capture a screenshot","input_schema":parameters}],
                "messages":[{"role":"user","content":PROMPT},
                    {"role":"assistant","content":[{"type":"tool_use","id":CALL_ID,"name":"take_screenshot","input":{}}]},
                    {"role":"user","content":content}]})
        }
        Api::Responses => {
            let image = match file_id {
                Some(id) => json!({"type":"input_image","file_id":id}),
                None => {
                    json!({"type":"input_image","image_url":format!("data:image/png;base64,{}",IMAGE.trim())})
                }
            };
            let mut input = vec![
                json!({"role":"user","content":PROMPT}),
                // No fc_* id or fake reasoning item: those trigger unrelated signature validation.
                json!({"type":"function_call","call_id":CALL_ID,"name":"take_screenshot","arguments":"{}"}),
                json!({"type":"function_call_output","call_id":CALL_ID,"output":
                    if shape.native() { json!([{"type":"input_text","text":RESULT},image.clone()]) } else { json!(RESULT) }}),
            ];
            if !shape.native() {
                input.push(json!({"role":"user","content":[{"type":"input_text","text":"Attached image from tool result:"},image]}));
            }
            json!({"model":target.model,"stream":true,"store":false,"max_output_tokens":2048,
                "tools":[{"type":"function","name":"take_screenshot","description":"Capture a screenshot","parameters":parameters}],
                "input":input})
        }
        Api::Chat => {
            let image = json!({"type":"image_url","image_url":{"url":format!("data:image/png;base64,{}",IMAGE.trim())}});
            let mut messages = vec![
                json!({"role":"user","content":PROMPT}),
                json!({"role":"assistant","content":null,"tool_calls":[{"id":CALL_ID,"type":"function","function":{"name":"take_screenshot","arguments":"{}"}}]}),
                json!({"role":"tool","tool_call_id":CALL_ID,"content":
                    if shape.native() { json!([{"type":"text","text":RESULT},image.clone()]) } else { json!(RESULT) }}),
            ];
            if !shape.native() {
                messages.push(json!({"role":"user","content":[{"type":"text","text":"Attached image from tool result:"},image]}));
            }
            let mut body = json!({"model":target.model,"stream":true,"max_tokens":2048,
                "tools":[{"type":"function","function":{"name":"take_screenshot","description":"Capture a screenshot","parameters":parameters}}],
                "messages":messages});
            if target.relay == "kimi" {
                body["thinking"] = json!({"type":"disabled"});
            }
            body
        }
    }
}

fn visible_text(api: Api, value: &Value) -> String {
    match api {
        Api::Anthropic if value["type"] == "content_block_delta" => {
            value["delta"]["text"].as_str().unwrap_or("").into()
        }
        Api::Anthropic => value["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|p| p["type"] == "text")
            .filter_map(|p| p["text"].as_str())
            .collect(),
        Api::Responses if value["type"] == "response.output_text.delta" => {
            value["delta"].as_str().unwrap_or("").into()
        }
        Api::Responses => value["output"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|item| item["content"].as_array().into_iter().flatten())
            .filter(|p| p["type"] == "output_text")
            .filter_map(|p| p["text"].as_str())
            .collect(),
        Api::Chat => value["choices"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| {
                c["delta"]["content"]
                    .as_str()
                    .or_else(|| c["message"]["content"].as_str())
            })
            .collect(),
    }
}
fn decode_answer(api: Api, body: &str) -> (String, Option<String>) {
    if let Ok(value) = serde_json::from_str::<Value>(body) {
        return (
            visible_text(api, &value),
            value
                .get("error")
                .filter(|e| !e.is_null())
                .map(ToString::to_string),
        );
    }
    let mut text = String::new();
    let mut error = None;
    let mut completed_text = String::new();
    for line in body
        .lines()
        .filter_map(|line| line.strip_prefix("data:").map(str::trim))
    {
        if line.is_empty() || line == "[DONE]" {
            continue;
        }
        match serde_json::from_str::<Value>(line) {
            Ok(value) => {
                text.push_str(&visible_text(api, &value));
                if value["type"] == "response.completed" {
                    completed_text = visible_text(api, &value["response"]);
                }
                if value["type"] == "response.failed"
                    || value["type"] == "error"
                    || value.get("error").is_some_and(|e| !e.is_null())
                {
                    error = Some(value.to_string());
                }
            }
            Err(e) => error = Some(format!("invalid SSE JSON: {e}")),
        }
    }
    if text.is_empty() {
        text = completed_text;
    }
    (text, error)
}

#[derive(Serialize)]
struct Outcome {
    api: &'static str,
    relay: &'static str,
    model: String,
    shape: &'static str,
    http_status: Option<u16>,
    saw_image: bool,
    answer: String,
    error: Option<String>,
    skipped: bool,
}
fn record(outcome: &Outcome) {
    eprintln!(
        "{} | {} | {} | {} | {} | {} | {}",
        outcome.api,
        outcome.relay,
        outcome.model,
        outcome.shape,
        outcome
            .http_status
            .map(|s| s.to_string())
            .unwrap_or_else(|| if outcome.skipped { "SKIP" } else { "ERROR" }.into()),
        outcome.saw_image,
        outcome
            .error
            .as_ref()
            .unwrap_or(&outcome.answer)
            .chars()
            .take(80)
            .collect::<String>()
    );
    if let Ok(path) = std::env::var("TOMCAT_MEDIA_PROBE_OUTPUT") {
        let path = std::path::Path::new(&path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        writeln!(file, "{}", serde_json::to_string(outcome).unwrap()).unwrap();
    }
}
async fn probe(api: Api, relay: &'static str, shape: Shape) {
    common::load_openai_test_env();
    if let Some(home) = dirs::home_dir() {
        let _ = dotenvy::from_path(home.join(".tomcat/assets/.env"));
    }
    let target = target(api, relay);
    let mut outcome = Outcome {
        api: api.label(),
        relay,
        model: target.model.clone(),
        shape: shape.label(),
        http_status: None,
        saw_image: false,
        answer: String::new(),
        error: None,
        skipped: false,
    };
    let Some(key) = std::env::var(&target.key_env)
        .ok()
        .filter(|k| !k.trim().is_empty())
    else {
        outcome.skipped = true;
        outcome.error = Some(format!("missing {}", target.key_env));
        record(&outcome);
        return;
    };
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(180))
        .build()
        .unwrap();
    let files = OpenAiFilesClient::from_provider_context(
        FilesApiProviderContext {
            client: client.clone(),
            base_url: target.base.clone(),
            api_key: key.clone(),
            retry_count: 1,
        },
        &Default::default(),
    );
    let uploaded = if shape.file_id() {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(IMAGE.trim())
            .unwrap();
        match files
            .upload(
                FilePurpose::Vision,
                "tomcat-tool-media-probe.png",
                "image/png",
                &bytes,
            )
            .await
        {
            Ok(file) => Some(file.id),
            Err(e) => {
                outcome.error = Some(format!("upload: {e}").replace(&key, "[redacted]"));
                record(&outcome);
                panic!("{} {} {} upload failed", api.label(), relay, shape.label());
            }
        }
    } else {
        None
    };
    let body = payload(api, &target, shape, uploaded.as_deref());
    let mut request = client.post(endpoint(api, &target.base)).json(&body);
    request = if api == Api::Anthropic {
        request
            .header("x-api-key", &key)
            .header("anthropic-version", "2023-06-01")
    } else {
        request.bearer_auth(&key)
    };
    match request.send().await {
        Ok(response) => {
            let status = response.status();
            outcome.http_status = Some(status.as_u16());
            match response.text().await {
                Ok(body) => {
                    let (answer, error) = decode_answer(api, &body);
                    outcome.answer = answer
                        .replace(&key, "[redacted]")
                        .chars()
                        .take(240)
                        .collect();
                    outcome.error = error
                        .or_else(|| (!status.is_success()).then_some(body))
                        .map(|e| e.replace(&key, "[redacted]").chars().take(400).collect());
                    outcome.saw_image = status.is_success()
                        && outcome.error.is_none()
                        && outcome
                            .answer
                            .to_ascii_lowercase()
                            .split(|c: char| !c.is_ascii_alphabetic())
                            .any(|word| word == "dog" || word == "beagle");
                }
                Err(e) => outcome.error = Some(format!("body: {e}").replace(&key, "[redacted]")),
            }
        }
        Err(e) => outcome.error = Some(format!("transport: {e}").replace(&key, "[redacted]")),
    }
    if let Some(id) = uploaded {
        if let Err(e) = files.delete(&id).await {
            outcome.error =
                Some(format!("cleanup of uploaded file {id}: {e}").replace(&key, "[redacted]"));
            outcome.saw_image = false;
        }
    }
    // idatatlas's Chat endpoint is optional, not a failed image transport if absent.
    outcome.skipped = api == Api::Chat
        && relay == "idatatlas"
        && matches!(outcome.http_status, Some(404 | 405 | 501));
    record(&outcome);
    if !(api == Api::Chat && shape.native()) && !outcome.skipped {
        assert!(
            outcome.saw_image,
            "probe failed: {} {} {} status={:?} error={:?} answer={:?}",
            api.label(),
            relay,
            shape.label(),
            outcome.http_status,
            outcome.error,
            outcome.answer
        );
    }
}

macro_rules! probe_case {
    ($name:ident, $api:ident, $relay:literal, $shape:ident) => {
        #[tokio::test]
        #[ignore = "manual: real model cost and credentials"]
        #[serial]
        async fn $name() {
            probe(Api::$api, $relay, Shape::$shape).await;
        }
    };
}
probe_case!(
    anthropic_tool_result_image_native_fcodex,
    Anthropic,
    "fcodex",
    Native
);
probe_case!(
    anthropic_tool_result_image_split_fcodex,
    Anthropic,
    "fcodex",
    Split
);
probe_case!(
    responses_function_output_image_native_idatatlas,
    Responses,
    "idatatlas",
    Native
);
probe_case!(
    responses_function_output_image_split_idatatlas,
    Responses,
    "idatatlas",
    Split
);
probe_case!(
    responses_function_output_image_file_id_native_idatatlas,
    Responses,
    "idatatlas",
    NativeFileId
);
probe_case!(
    responses_function_output_image_file_id_split_idatatlas,
    Responses,
    "idatatlas",
    SplitFileId
);
probe_case!(
    responses_function_output_image_native_fcodex,
    Responses,
    "fcodex",
    Native
);
probe_case!(
    responses_function_output_image_split_fcodex,
    Responses,
    "fcodex",
    Split
);

probe_case!(
    chat_completions_tool_image_native_records_outcome_idatatlas,
    Chat,
    "idatatlas",
    Native
);
probe_case!(
    chat_completions_tool_image_split_idatatlas,
    Chat,
    "idatatlas",
    Split
);
probe_case!(
    chat_completions_tool_image_native_records_outcome_kimi,
    Chat,
    "kimi",
    Native
);
probe_case!(chat_completions_tool_image_split_kimi, Chat, "kimi", Split);

#[test]
fn parser_joins_visible_sse_text_and_rejects_stream_errors() {
    assert_eq!(decode_answer(Api::Responses,"data: {\"type\":\"response.output_text.delta\",\"delta\":\"do\"}\n\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"g\"}\n\ndata: [DONE]\n"), ("dog".into(),None));
    assert_eq!(decode_answer(Api::Anthropic,"data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"cat\"}}\n\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"dog\"}}\n"), ("dog".into(),None));
    assert_eq!(
        decode_answer(
            Api::Chat,
            "data: {\"choices\":[{\"delta\":{\"content\":\"dog\"}}]}\n\ndata: [DONE]\n"
        ),
        ("dog".into(), None)
    );
    assert!(decode_answer(
        Api::Responses,
        "data: {\"type\":\"error\",\"error\":{\"message\":\"bad image\"}}\n"
    )
    .1
    .is_some());
}

#[test]
fn parser_payload_native_and_split_only_move_media() {
    for api in [Api::Anthropic, Api::Responses, Api::Chat] {
        let t = target(api, "idatatlas");
        let native = payload(api, &t, Shape::Native, None);
        let split = payload(api, &t, Shape::Split, None);
        assert_eq!(native["model"], split["model"]);
        assert_eq!(native["stream"], true);
        assert!(!PROMPT.contains("beagle") && !RESULT.contains("beagle"));
        assert!(!PROMPT.contains("dog") && !RESULT.contains("dog"));
        assert_ne!(native, split);
        if api == Api::Responses {
            assert!(native["input"][1].get("id").is_none());
            assert!(native["input"][2]["output"].is_array());
            assert!(split["input"][2]["output"].is_string());
        }
    }
}
