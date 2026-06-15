//! # `core::llm::types` 焦小测
//!
//! 覆盖：
//!
//! - `ChatMessage::{user, assistant, system, tool, assistant_with_tool_calls}`
//!   构造结果与字段约定。
//! - `ChatRequest` 序列化为 snake_case JSON。
//! - `TokenUsage::default` / `StreamEvent::ContentDelta` 序列化默认值。

use super::super::types::{
    ChatMessage, ChatMessageContent, ChatMessageContentPart, ChatMessageRole, ChatRequest,
    ContinuityMetadata, ReasoningContinuation, ReasoningFormat, ReplayRequirement, StreamEvent,
    ThinkingSource, TokenUsage, FILE_MAX_BYTES, IMAGE_MAX_BYTES,
};
use crate::core::llm::openai_files::OpenAiFilesClient;

#[test]
fn chat_message_constructors() {
    let u = ChatMessage::user("hello");
    assert!(matches!(u.role, ChatMessageRole::User));
    assert!(matches!(&u.content, Some(ChatMessageContent::Text(s)) if s == "hello"));

    let a = ChatMessage::assistant("hi");
    assert!(matches!(a.role, ChatMessageRole::Assistant));

    let s = ChatMessage::system("you are helpful");
    assert!(matches!(s.role, ChatMessageRole::System));
}

#[test]
fn chat_message_tool() {
    let t = ChatMessage::tool("call_1", "result");
    assert!(matches!(t.role, ChatMessageRole::Tool));
    assert_eq!(t.tool_call_id.as_deref(), Some("call_1"));
    assert_eq!(t.text_content(), Some("result"));
}

#[test]
fn chat_message_assistant_with_tool_calls() {
    let tc = vec![
        serde_json::json!({"id": "c1", "type": "function", "function": {"name": "f1", "arguments": "{}"}}),
    ];
    let m = ChatMessage::assistant_with_tool_calls(Some("thinking"), tc);
    assert!(m.tool_calls.is_some());
    assert_eq!(m.text_content(), Some("thinking"));
}

#[test]
fn chat_message_completion_metadata_roundtrip() {
    let msg = ChatMessage::assistant("oops")
        .with_completion_metadata(
            Some("error:boom".to_string()),
            Some("boom".to_string()),
            Some("bad_request".to_string()),
        )
        .with_reasoning_state(
            Some("safe summary".to_string()),
            Some(ReasoningContinuation {
                source_provider: "openai".to_string(),
                source_api: "responses".to_string(),
                source_model: "gpt-5".to_string(),
                format: ReasoningFormat::OpenaiResponsesReasoningItems,
                opaque_payload: serde_json::json!([{"encrypted_content":"enc"}]),
                fallback_text: Some("safe summary".to_string()),
                provider_refs: None,
            }),
            Some(ContinuityMetadata {
                had_tool_call: false,
                replay_requirement: ReplayRequirement::SameProfileOptional,
            }),
        );
    let json = serde_json::to_value(&msg).unwrap();
    assert_eq!(json["finish_reason"], "error:boom");
    assert_eq!(json["error_message"], "boom");
    assert_eq!(json["error_code"], "bad_request");
    assert_eq!(json["thinking_text"], "safe summary");
    assert_eq!(
        json["reasoning_continuation"]["format"],
        "openai_responses_reasoning_items"
    );

    let stripped = msg.without_completion_metadata();
    let stripped_json = serde_json::to_value(&stripped).unwrap();
    assert!(stripped_json.get("finish_reason").is_none());
    assert!(stripped_json.get("error_message").is_none());
    assert!(stripped_json.get("error_code").is_none());
    assert!(stripped_json.get("thinking_text").is_none());
    assert!(stripped_json.get("reasoning_continuation").is_none());
    assert!(stripped_json.get("continuity").is_none());
}

#[test]
fn chat_message_annotations_roundtrip_and_skip() {
    let mut msg = ChatMessage::assistant("with cites");
    msg.annotations = Some(vec![serde_json::json!({
        "type": "url_citation",
        "url": "https://example.com",
        "title": "Example"
    })]);

    let json = serde_json::to_value(&msg).unwrap();
    assert_eq!(json["annotations"][0]["type"], "url_citation");
    let roundtrip: ChatMessage = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(
        roundtrip
            .annotations
            .as_ref()
            .and_then(|v| v.first())
            .and_then(|v| v.get("url")),
        Some(&serde_json::json!("https://example.com"))
    );

    let stripped = msg.without_completion_metadata();
    let stripped_json = serde_json::to_value(&stripped).unwrap();
    assert!(
        stripped_json.get("annotations").is_none(),
        "without_completion_metadata 应剥离 provider annotations"
    );

    let plain_json = serde_json::to_value(ChatMessage::assistant("plain")).unwrap();
    assert!(
        plain_json.get("annotations").is_none(),
        "annotations=None 应被 skip_serializing"
    );
}

#[test]
fn chat_request_serialize_snake_case() {
    let req = ChatRequest {
        messages: vec![ChatMessage::user("test")],
        model: "gpt-4".to_string(),
        temperature: Some(0.5),
        max_tokens: Some(100),
        stream: Some(false),
        model_override: None,
        tools: None,
    };
    let j = serde_json::to_string(&req).unwrap();
    assert!(j.contains("model"));
    assert!(j.contains("messages"));
}

#[test]
fn chat_request_serializes_hydrate_recovered_tool_round_for_openai_wire() {
    let assistant = ChatMessage::assistant_with_tool_calls(
        Some("calling tool"),
        vec![serde_json::json!({
            "id": "call_1",
            "type": "function",
            "function": {"name": "read", "arguments": "{}"}
        })],
    );
    let req = ChatRequest {
        messages: vec![
            ChatMessage::user("resume"),
            assistant,
            ChatMessage::tool("call_1", "[interrupted]"),
        ],
        model: "gpt-4".to_string(),
        temperature: None,
        max_tokens: None,
        stream: Some(false),
        model_override: None,
        tools: None,
    };
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["messages"][1]["tool_calls"][0]["id"], "call_1");
    assert_eq!(j["messages"][2]["role"], "tool");
    assert_eq!(j["messages"][2]["tool_call_id"], "call_1");
    assert_eq!(j["messages"][2]["content"], "[interrupted]");
}

#[test]
fn token_usage_default() {
    let u = TokenUsage::default();
    assert_eq!(u.prompt_tokens, 0);
    assert_eq!(u.completion_tokens, 0);
}

#[test]
fn stream_event_serialize() {
    let e = StreamEvent::ContentDelta {
        delta: "hello".to_string(),
    };
    let j = serde_json::to_value(&e).unwrap();
    assert_eq!(
        j.get("type").and_then(|v| v.as_str()),
        Some("content_delta")
    );
    assert_eq!(j.get("delta").and_then(|v| v.as_str()), Some("hello"));
}

#[test]
fn stream_event_thinking_serde_minimal() {
    let e = StreamEvent::Thinking {
        delta: "step 1".to_string(),
        source: ThinkingSource::Raw,
        signature: None,
    };
    let j = serde_json::to_value(&e).unwrap();
    assert_eq!(j.get("type").and_then(|v| v.as_str()), Some("thinking"));
    assert_eq!(j.get("delta").and_then(|v| v.as_str()), Some("step 1"));
    assert_eq!(j.get("source").and_then(|v| v.as_str()), Some("raw"));
    assert!(
        j.get("signature").is_none(),
        "signature=None 应被 skip_serializing"
    );
    let back: StreamEvent = serde_json::from_value(j).unwrap();
    assert!(matches!(
        back,
        StreamEvent::Thinking {
            delta,
            source: ThinkingSource::Raw,
            signature: None
        } if delta == "step 1"
    ));
}

#[test]
fn stream_event_thinking_serde_with_signature() {
    let e = StreamEvent::Thinking {
        delta: "secret reasoning".to_string(),
        source: ThinkingSource::Summary,
        signature: Some("sig-abc".to_string()),
    };
    let j = serde_json::to_value(&e).unwrap();
    assert_eq!(j.get("type").and_then(|v| v.as_str()), Some("thinking"));
    assert_eq!(j.get("source").and_then(|v| v.as_str()), Some("summary"));
    assert_eq!(j.get("signature").and_then(|v| v.as_str()), Some("sig-abc"));
    let back: StreamEvent = serde_json::from_value(j).unwrap();
    assert!(matches!(
        back,
        StreamEvent::Thinking {
            source: ThinkingSource::Summary,
            signature: Some(s),
            ..
        } if s == "sig-abc"
    ));
}

#[test]
fn stream_event_thinking_serde_missing_source_is_rejected() {
    let err = serde_json::from_value::<StreamEvent>(serde_json::json!({
        "type": "thinking",
        "delta": "step 1"
    }))
    .expect_err("missing source should fail");
    assert!(
        err.to_string().contains("source"),
        "反序列化错误应指出缺少 source: {}",
        err
    );
}

#[test]
fn stream_event_llm_error_and_notice_serde() {
    let err = StreamEvent::LlmError {
        reason: "error:boom".to_string(),
        message: "boom".to_string(),
        code: Some("server_error".to_string()),
    };
    let err_json = serde_json::to_value(&err).unwrap();
    assert_eq!(err_json["type"], "llm_error");
    assert_eq!(err_json["reason"], "error:boom");
    assert_eq!(err_json["message"], "boom");
    assert_eq!(err_json["code"], "server_error");

    let notice = StreamEvent::LlmNotice {
        finish_reason: "max_output_tokens".to_string(),
        message: "达到 max_output_tokens，回答可能未完成".to_string(),
    };
    let notice_json = serde_json::to_value(&notice).unwrap();
    assert_eq!(notice_json["type"], "llm_notice");
    assert_eq!(notice_json["finish_reason"], "max_output_tokens");
    assert!(notice_json["message"]
        .as_str()
        .unwrap()
        .contains("max_output_tokens"));
}

// ============================================================================
// ChatMessageContentPart：serde 往返 + helper 校验失败用例（plan §5）
// ============================================================================

const TINY_PNG_B64: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=";

/// PR-RJ-0：把 inline base64 fixture 解码后写到 tempfile，供新签名
/// `image_b64(mime, &Path)` / `file_b64(filename, mime, &Path)` 使用。
fn write_tiny_png_tempfile() -> tempfile::NamedTempFile {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(TINY_PNG_B64)
        .expect("decode TINY_PNG_B64");
    let mut f = tempfile::NamedTempFile::new().expect("temp png");
    std::io::Write::write_all(&mut f, &bytes).expect("write png");
    f
}

fn write_oversize_tempfile(n: usize) -> tempfile::NamedTempFile {
    let mut f = tempfile::NamedTempFile::new().expect("temp oversize");
    std::io::Write::write_all(&mut f, &vec![0u8; n]).expect("write oversize");
    f
}

#[test]
fn content_part_serde_roundtrip_text() {
    let p = ChatMessageContentPart::text("hi");
    let j = serde_json::to_value(&p).unwrap();
    assert_eq!(j["type"], "input_text");
    assert_eq!(j["text"], "hi");
    let back: ChatMessageContentPart = serde_json::from_value(j).unwrap();
    assert!(matches!(back, ChatMessageContentPart::InputText { text } if text == "hi"));
}

#[test]
fn content_part_serde_roundtrip_image_b64() {
    let f = write_tiny_png_tempfile();
    let p = ChatMessageContentPart::image_b64("image/png", f.path()).unwrap();
    let j = serde_json::to_value(&p).unwrap();
    assert_eq!(j["type"], "input_image");
    assert_eq!(j["mime_type"], "image/png");
    // 新签名仍生成与 fixture 一致的 base64（标准编码、无填充差异）。
    assert_eq!(j["image_b64"], TINY_PNG_B64);
    assert!(j.get("file_id").is_none());
    let back: ChatMessageContentPart = serde_json::from_value(j).unwrap();
    assert!(matches!(
        back,
        ChatMessageContentPart::InputImage {
            mime_type: Some(_),
            data: Some(_),
            file_id: None,
            ..
        }
    ));
}

#[test]
fn content_part_serde_roundtrip_file_id() {
    let p = ChatMessageContentPart::file_file_id("file-xyz", Some("a.pdf".to_string())).unwrap();
    let j = serde_json::to_value(&p).unwrap();
    assert_eq!(j["type"], "input_file");
    assert_eq!(j["file_id"], "file-xyz");
    assert_eq!(j["filename"], "a.pdf");
    let back: ChatMessageContentPart = serde_json::from_value(j).unwrap();
    assert!(matches!(
        back,
        ChatMessageContentPart::InputFile {
            file_id: Some(id),
            filename: Some(name),
            data: None,
            mime_type: None,
        } if id == "file-xyz" && name == "a.pdf"
    ));
}

#[test]
fn image_b64_rejects_missing_path() {
    let err = ChatMessageContentPart::image_b64("image/png", "/nonexistent/never-here-xyzz.png")
        .expect_err("路径不存在应拒绝");
    let s = err.to_string();
    assert!(s.contains("无法 stat"), "错误文案应提示 stat 失败: {}", s);
}

#[test]
fn image_b64_rejects_oversize() {
    let f = write_oversize_tempfile(IMAGE_MAX_BYTES + 1);
    let err = ChatMessageContentPart::image_b64("image/png", f.path())
        .expect_err("超 IMAGE_MAX_BYTES 应拒绝");
    let s = err.to_string();
    assert!(s.contains("IMAGE_MAX_BYTES"), "错误文案不对: {}", s);
}

#[test]
fn image_b64_rejects_non_whitelisted_mime() {
    let f = write_tiny_png_tempfile();
    let err = ChatMessageContentPart::image_b64("image/svg+xml", f.path())
        .expect_err("svg 不在白名单应拒绝");
    let s = err.to_string();
    assert!(s.contains("mime_type"), "错误文案不对: {}", s);
}

#[test]
fn file_b64_rejects_missing_path() {
    let err = ChatMessageContentPart::file_b64(
        "a.pdf",
        "application/pdf",
        "/nonexistent/never-here-xyzz.pdf",
    )
    .expect_err("路径不存在应拒绝");
    let s = err.to_string();
    assert!(s.contains("无法 stat"), "错误文案应提示 stat 失败: {}", s);
}

#[test]
fn file_b64_rejects_oversize() {
    let f = write_oversize_tempfile(FILE_MAX_BYTES + 1);
    let err = ChatMessageContentPart::file_b64("a.pdf", "application/pdf", f.path())
        .expect_err("超 FILE_MAX_BYTES 应拒绝");
    let s = err.to_string();
    assert!(s.contains("FILE_MAX_BYTES"), "错误文案不对: {}", s);
}

#[test]
fn image_file_id_rejects_empty() {
    let err = ChatMessageContentPart::image_file_id("   ").expect_err("空 file_id 应拒绝");
    assert!(err.to_string().contains("不能为空"));
}

#[tokio::test]
async fn image_upload_rejects_non_whitelisted_mime_before_network() {
    let client = OpenAiFilesClient::new_for_test(
        reqwest::Client::new(),
        "http://127.0.0.1:9".to_string(),
        "stub".to_string(),
        0,
        86_400,
    );
    let err = ChatMessageContentPart::image_upload(&client, "image/svg+xml", &[1, 2], "a.svg")
        .await
        .expect_err("非白名单 mime 应在发请求前失败");
    assert!(err.to_string().contains("mime_type"));
}

#[tokio::test]
async fn file_upload_rejects_empty_bytes_before_network() {
    let client = OpenAiFilesClient::new_for_test(
        reqwest::Client::new(),
        "http://127.0.0.1:9".to_string(),
        "stub".to_string(),
        0,
        86_400,
    );
    let err = ChatMessageContentPart::file_upload(&client, "a.pdf", "application/pdf", &[])
        .await
        .expect_err("空字节应在发请求前失败");
    assert!(err.to_string().contains("为空"));
}
