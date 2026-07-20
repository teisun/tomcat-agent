//! # LLM Provider Trait
//!
//! 与 design CODE_BLOCK_P1_005 一致，供宿主 API 与 chat 调用。

use async_trait::async_trait;
use std::sync::Arc;
use tokio_stream::Stream;

use crate::infra::error::AppError;

use super::files_api::FilesApiAdapter;
use super::types::{ChatMessage, ChatRequest, ChatResponse, StreamEvent};

/// 统一 LLM 接入 Trait：非流式/流式调用、Token 统计。
#[async_trait]
pub trait LlmProvider: Send + Sync + 'static {
    /// 提供商名称，如 "openai"。
    fn provider_name(&self) -> &str;

    /// 非流式对话，返回完整响应。
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, AppError>;

    /// 流式对话，返回 StreamEvent 流；流式中断/超时时产生错误并释放资源。
    async fn chat_stream(
        &self,
        request: ChatRequest,
    ) -> Result<Box<dyn Stream<Item = Result<StreamEvent, AppError>> + Send + Unpin>, AppError>;

    /// Token 计数（近似实现），用于上下文窗口估算等。
    fn count_tokens(&self, messages: &[ChatMessage]) -> Result<u32, AppError>;

    /// 当前 provider 是否声明支持 Files API 上传能力，并返回对应适配器。
    ///
    /// 默认 `None`：仅文本 provider 或未显式实现 Files 能力的 provider 不可走上传路径。
    fn files_adapter(
        &self,
        _files_cfg: &crate::infra::config::LlmFilesConfig,
    ) -> Option<Arc<dyn FilesApiAdapter>> {
        None
    }
}
