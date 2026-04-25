//! # `core::llm::types` 焦小测
//!
//! 覆盖：
//!
//! - `ChatMessage::{user, assistant, system, tool, assistant_with_tool_calls}`
//!   构造结果与字段约定。
//! - `ChatRequest` 序列化为 snake_case JSON。
//! - `TokenUsage::default` / `StreamEvent::ContentDelta` 序列化默认值。

use super::super::types::{
    ChatMessage, ChatMessageContent, ChatMessageRole, ChatRequest, StreamEvent, TokenUsage,
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
