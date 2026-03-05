//! # LLM 请求/响应类型
//!
//! 与 OpenAI API 兼容，供宿主与插件共用；字段命名与 pi-mono/OpenAI 对齐（camelCase）。

use serde::{Deserialize, Serialize};

/// 单条对话消息，与 OpenAI chat completions messages 兼容。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatMessageRole {
    System,
    User,
    Assistant,
}

/// 消息内容：纯文本或 parts 数组（便于扩展多模态）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ChatMessageContent {
    Text(String),
    Parts(Vec<ChatMessageContentPart>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessageContentPart {
    #[serde(rename = "type")]
    pub part_type: String,
    pub text: Option<String>,
}

/// 单条对话消息（与 OpenAI API 兼容，wire 格式为 snake_case）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ChatMessage {
    pub role: ChatMessageRole,
    pub content: ChatMessageContent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl ChatMessage {
    /// 从纯文本构造一条用户消息。
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: ChatMessageRole::User,
            content: ChatMessageContent::Text(text.into()),
            name: None,
        }
    }

    /// 从纯文本构造一条助手消息。
    pub fn assistant(text: impl Into<String>) -> Self {
        Self {
            role: ChatMessageRole::Assistant,
            content: ChatMessageContent::Text(text.into()),
            name: None,
        }
    }

    /// 从纯文本构造一条系统消息。
    pub fn system(text: impl Into<String>) -> Self {
        Self {
            role: ChatMessageRole::System,
            content: ChatMessageContent::Text(text.into()),
            name: None,
        }
    }
}

/// 会话级模型覆盖；若为 None，使用全局 LlmConfig.default_model。
/// 后续 SessionManager 可用时由上层从 SessionEntry.model_override 填入。
/// 与 OpenAI API 请求体兼容（snake_case）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,
    /// 实际使用的模型由 model_override 优先，否则由调用方填入默认模型。
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    /// 会话级模型覆盖，与 SessionEntry.model_override 对应（不发给 API，仅用于选模型）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_override: Option<String>,
}

/// 单次调用的 token 使用量（与 OpenAI API 一致，snake_case）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u32>,
}

/// 非流式聊天响应，与 OpenAI 格式一致（snake_case）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ChatResponse {
    pub id: Option<String>,
    pub choices: Vec<ChatResponseChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<TokenUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ChatResponseChoice {
    pub index: u32,
    pub message: ChatMessage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
}

/// 流式事件类型，与 pi-mono 流式 API 对齐。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    /// 内容增量。
    ContentDelta { delta: String },
    /// 结束原因。
    FinishReason { reason: String },
    /// 单次调用的 usage（流结束时常在最后一条）。
    Usage {
        prompt_tokens: u32,
        completion_tokens: u32,
        total_tokens: Option<u32>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_message_constructors() {
        let u = ChatMessage::user("hello");
        assert!(matches!(u.role, ChatMessageRole::User));
        assert!(matches!(&u.content, ChatMessageContent::Text(s) if s == "hello"));

        let a = ChatMessage::assistant("hi");
        assert!(matches!(a.role, ChatMessageRole::Assistant));

        let s = ChatMessage::system("you are helpful");
        assert!(matches!(s.role, ChatMessageRole::System));
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
}
