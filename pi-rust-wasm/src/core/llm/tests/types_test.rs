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
    StreamEvent, TokenUsage, FILE_MAX_BYTES, IMAGE_MAX_BYTES,
};

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
        signature: None,
    };
    let j = serde_json::to_value(&e).unwrap();
    assert_eq!(j.get("type").and_then(|v| v.as_str()), Some("thinking"));
    assert_eq!(j.get("delta").and_then(|v| v.as_str()), Some("step 1"));
    assert!(
        j.get("signature").is_none(),
        "signature=None 应被 skip_serializing"
    );
    let back: StreamEvent = serde_json::from_value(j).unwrap();
    assert!(matches!(
        back,
        StreamEvent::Thinking { delta, signature: None } if delta == "step 1"
    ));
}

#[test]
fn stream_event_thinking_serde_with_signature() {
    let e = StreamEvent::Thinking {
        delta: "secret reasoning".to_string(),
        signature: Some("sig-abc".to_string()),
    };
    let j = serde_json::to_value(&e).unwrap();
    assert_eq!(j.get("type").and_then(|v| v.as_str()), Some("thinking"));
    assert_eq!(
        j.get("signature").and_then(|v| v.as_str()),
        Some("sig-abc")
    );
    let back: StreamEvent = serde_json::from_value(j).unwrap();
    assert!(matches!(
        back,
        StreamEvent::Thinking { signature: Some(s), .. } if s == "sig-abc"
    ));
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
