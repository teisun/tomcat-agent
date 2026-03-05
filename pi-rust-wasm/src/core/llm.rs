//! # LLM 统一接入 Trait 与类型（与 design CODE_BLOCK_P1_005 一致）

use crate::infra::error::AppError;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::pin::Pin;

/// 单条聊天消息。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

/// 聊天请求。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,
    pub model: Option<String>,
    pub stream: Option<bool>,
}

/// 聊天响应（非流式）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatResponse {
    pub content: String,
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

/// 流式事件（单块）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamEvent {
    pub content: Option<String>,
    pub done: Option<bool>,
}

/// 统一 LLM Provider Trait（与 design CODE_BLOCK_P1_005 一致）。
#[async_trait]
pub trait LlmProvider: Send + Sync + 'static {
    fn provider_name(&self) -> &str;
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, AppError>;
    async fn chat_stream(
        &self,
        request: ChatRequest,
    ) -> Result<
        Pin<Box<dyn tokio_stream::Stream<Item = Result<StreamEvent, AppError>> + Send>>,
        AppError,
    >;
    fn count_tokens(&self, messages: &[ChatMessage]) -> Result<u32, AppError>;
}
