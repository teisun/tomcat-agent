//! # LLM Provider Trait
//!
//! 与 design CODE_BLOCK_P1_005 一致，供宿主 API 与 chat 调用。

use async_trait::async_trait;
use tokio_stream::Stream;

use crate::infra::error::AppError;

use super::openai_files::{OpenAiFilesClient, OpenAiFilesProviderContext};
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

    /// 当前 provider 是否声明支持 OpenAI Files API（`POST /v1/files`）。
    ///
    /// 默认 `false`：仅文本 provider 或未显式实现 Files 能力的 provider 不可走上传路径。
    fn supports_openai_files_api(&self) -> bool {
        false
    }

    /// 暴露 OpenAI Files 所需的 provider 上下文（共享 HTTP client/base_url/api_key/retry_count）。
    ///
    /// 默认 `None`。仅当 [`Self::supports_openai_files_api`] 返回 `true` 时才应返回 `Some`。
    fn openai_files_context(&self) -> Option<OpenAiFilesProviderContext> {
        None
    }

    /// 获取 OpenAI Files 客户端。
    ///
    /// 默认实现：基于 [`Self::openai_files_context`] 即时构造一个客户端副本。
    /// 需要懒加载复用（OnceCell）时，provider 可覆盖该方法并返回同一实例 clone。
    fn openai_files_client(
        &self,
        files_cfg: &crate::infra::config::LlmFilesConfig,
    ) -> Option<OpenAiFilesClient> {
        self.openai_files_context()
            .map(|ctx| OpenAiFilesClient::from_provider_context(ctx, files_cfg))
    }
}
