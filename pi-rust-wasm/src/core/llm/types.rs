//! # LLM 请求/响应类型
//!
//! 与 OpenAI API 兼容，供宿主与插件共用；字段命名与 pi-mono/OpenAI 对齐（camelCase）。

use serde::{Deserialize, Serialize};

/// 单条对话消息，与 OpenAI chat completions messages 兼容。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatMessageRole {
    System,
    User,
    Assistant,
    Tool,
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

/// Internal semantic tag for messages that share the same LLM wire role.
/// `#[serde(skip)]` — never serialized; defaults to `Normal` on deserialization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MessageKind {
    #[default]
    Normal,
    /// Steering instruction injected mid-turn; LLM sees `role: user`.
    Steering,
    /// Compaction summary replacing older messages; LLM sees `role: user`.
    CompactionSummary,
}

/// 单条对话消息（与 OpenAI API 兼容，wire 格式为 snake_case）。
///
/// Three `#[serde(skip)]` metadata fields (`msg_id`, `kind`, `timestamp`) carry
/// internal bookkeeping that never leaves the process boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ChatMessage {
    pub role: ChatMessageRole,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<ChatMessageContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,

    /// Transcript `MessageEntry.id` — set during hydration or after `append_message`.
    #[serde(skip)]
    pub msg_id: Option<String>,
    /// Semantic tag distinguishing steering / compaction-summary from normal messages.
    #[serde(skip)]
    pub kind: MessageKind,
    /// ISO-8601 timestamp from the transcript, used for day-based filtering.
    #[serde(skip)]
    pub timestamp: Option<String>,
}

impl ChatMessage {
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: ChatMessageRole::User,
            content: Some(ChatMessageContent::Text(text.into())),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            msg_id: None,
            kind: MessageKind::Normal,
            timestamp: None,
        }
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self {
            role: ChatMessageRole::Assistant,
            content: Some(ChatMessageContent::Text(text.into())),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            msg_id: None,
            kind: MessageKind::Normal,
            timestamp: None,
        }
    }

    pub fn assistant_with_tool_calls(
        content: Option<&str>,
        tool_calls: Vec<serde_json::Value>,
    ) -> Self {
        Self {
            role: ChatMessageRole::Assistant,
            content: content.map(|s| ChatMessageContent::Text(s.to_string())),
            name: None,
            tool_calls: Some(tool_calls),
            tool_call_id: None,
            msg_id: None,
            kind: MessageKind::Normal,
            timestamp: None,
        }
    }

    pub fn tool(tool_call_id: &str, content: &str) -> Self {
        Self {
            role: ChatMessageRole::Tool,
            content: Some(ChatMessageContent::Text(content.to_string())),
            name: None,
            tool_calls: None,
            tool_call_id: Some(tool_call_id.to_string()),
            msg_id: None,
            kind: MessageKind::Normal,
            timestamp: None,
        }
    }

    pub fn system(text: impl Into<String>) -> Self {
        Self {
            role: ChatMessageRole::System,
            content: Some(ChatMessageContent::Text(text.into())),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            msg_id: None,
            kind: MessageKind::Normal,
            timestamp: None,
        }
    }

    pub fn steering(text: impl Into<String>) -> Self {
        Self {
            role: ChatMessageRole::User,
            content: Some(ChatMessageContent::Text(text.into())),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            msg_id: None,
            kind: MessageKind::Steering,
            timestamp: None,
        }
    }

    pub fn compaction_summary(text: impl Into<String>) -> Self {
        Self {
            role: ChatMessageRole::User,
            content: Some(ChatMessageContent::Text(text.into())),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            msg_id: None,
            kind: MessageKind::CompactionSummary,
            timestamp: None,
        }
    }

    /// Replace the text content in-place (used by L0/L1 compaction on tool results).
    pub fn set_text_content(&mut self, text: String) {
        self.content = Some(ChatMessageContent::Text(text));
    }

    /// Helper to extract text content (for backward compat).
    pub fn text_content(&self) -> Option<&str> {
        match &self.content {
            Some(ChatMessageContent::Text(s)) => Some(s),
            _ => None,
        }
    }
}

/// 会话级模型覆盖；若为 None，使用全局 LlmConfig.default_model。
/// 后续 SessionManager 可用时由上层从 SessionEntry.model_override 填入。
/// 与 OpenAI API 请求体兼容（snake_case）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    /// 会话级模型覆盖（不发给 API，仅用于选模型）。
    #[serde(skip)]
    pub model_override: Option<String>,
    /// OpenAI function calling: tool definitions sent to the model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<serde_json::Value>>,
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
    ContentDelta {
        delta: String,
    },
    /// Tool call 增量（OpenAI streaming 格式）。
    ToolCallDelta {
        index: u32,
        id: Option<String>,
        name: Option<String>,
        arguments_delta: Option<String>,
    },
    FinishReason {
        reason: String,
    },
    Usage {
        prompt_tokens: u32,
        completion_tokens: u32,
        total_tokens: Option<u32>,
    },
}
