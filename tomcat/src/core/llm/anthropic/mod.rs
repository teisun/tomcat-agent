use std::borrow::Cow;
use std::future::Future;
use std::time::Duration;

use async_trait::async_trait;
use tokio_stream::Stream;
use tracing::warn;

use crate::core::llm::endpoint::build_path_aware_endpoint;
use crate::core::llm::http_client::build_http_client;
use crate::core::llm::provider::LlmProvider;
use crate::core::llm::replay_policy::ProviderCompatProfile;
use crate::core::llm::retry_delay::provider_retry_delay;
use crate::core::llm::types::{
    ChatMessage, ChatMessageContent, ChatRequest, ChatResponse, StreamEvent,
};
use crate::infra::config::LlmRuntimeConfig;
use crate::infra::error::{
    is_retryable_llm_error, llm_error, llm_error_with_source, llm_http_status_error, AppError,
    LlmErrorStage,
};

use super::super::auth::Credential;
use super::super::catalog::{infer_default_base_url, ModelEntry};

mod stream;
mod wire;

const PROVIDER_NAME: &str = "anthropic";

pub(super) struct AnthropicProvider {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    default_model: String,
    catalog_model_id: String,
    retry_count: u32,
    non_stream_stale_timeout_sec: u64,
    thinking_cfg: crate::infra::config::ThinkingConfig,
    continuity_enabled: bool,
}

impl AnthropicProvider {
    pub(super) fn new(
        entry: &ModelEntry,
        runtime: &LlmRuntimeConfig,
        credential: &Credential,
    ) -> Result<Self, AppError> {
        let client = build_http_client(runtime, None)?;
        let base_url = entry
            .base_url
            .clone()
            .or_else(|| infer_default_base_url(Some(entry.provider.as_str())))
            .ok_or_else(|| AppError::Config(format!("模型 `{}` 缺少 base_url。", entry.id)))?;
        Ok(Self {
            client,
            base_url,
            api_key: credential.value.clone(),
            default_model: entry.request_model_name().to_string(),
            catalog_model_id: entry.id.clone(),
            retry_count: runtime.retry_count,
            non_stream_stale_timeout_sec: runtime.non_stream_stale_timeout_sec,
            thinking_cfg: runtime.thinking.clone(),
            continuity_enabled: runtime.reasoning_continuity.enabled,
        })
    }

    fn effective_model(&self, request: &ChatRequest) -> String {
        let req_model = request.model.trim();
        if req_model.is_empty() || req_model == self.catalog_model_id {
            self.default_model.clone()
        } else {
            req_model.to_string()
        }
    }

    fn thinking_cfg_for_request<'a>(
        &'a self,
        request: &ChatRequest,
    ) -> Cow<'a, crate::infra::config::ThinkingConfig> {
        match request.thinking_level {
            Some(level) => {
                let mut cfg = self.thinking_cfg.clone();
                cfg.level = level.as_str().to_string();
                Cow::Owned(cfg)
            }
            None => Cow::Borrowed(&self.thinking_cfg),
        }
    }

    fn source_profile(&self, model: &str) -> ProviderCompatProfile {
        ProviderCompatProfile::anthropic_messages(model)
    }

    async fn run_non_stream_with_stale<T, F>(&self, fut: F) -> Result<T, AppError>
    where
        F: Future<Output = Result<T, AppError>>,
    {
        if self.non_stream_stale_timeout_sec == 0 {
            return fut.await;
        }
        match tokio::time::timeout(Duration::from_secs(self.non_stream_stale_timeout_sec), fut)
            .await
        {
            Ok(result) => result,
            Err(_) => Err(llm_error(
                PROVIDER_NAME,
                LlmErrorStage::NonStreamStale,
                format!(
                    "Anthropic 非流式请求长时间无响应: {}s",
                    self.non_stream_stale_timeout_sec
                ),
            )),
        }
    }

    fn auth_headers(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        builder
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
    }

    async fn chat_once(
        &self,
        request: &ChatRequest,
        stream: bool,
    ) -> Result<reqwest::Response, AppError> {
        let model = self.effective_model(request);
        let thinking_cfg = self.thinking_cfg_for_request(request);
        let body = wire::build_request_body(
            request,
            &model,
            &thinking_cfg,
            self.continuity_enabled,
            stream,
        );
        let url = build_path_aware_endpoint(&self.base_url, "messages");
        let response = self
            .auth_headers(self.client.post(url))
            .json(&body)
            .send()
            .await
            .map_err(|error| {
                llm_error_with_source(
                    PROVIDER_NAME,
                    if error.is_timeout() {
                        LlmErrorStage::ReadTimeout
                    } else {
                        LlmErrorStage::Send
                    },
                    format!(
                        "Anthropic {}请求失败",
                        if stream { "流式" } else { "非流式" }
                    ),
                    anyhow::anyhow!(error),
                )
            })?;
        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "anthropic error".to_string());
            return Err(llm_http_status_error(PROVIDER_NAME, status.as_u16(), body));
        }
        Ok(response)
    }

    async fn chat_with_retry(
        &self,
        request: &ChatRequest,
        stream: bool,
    ) -> Result<reqwest::Response, AppError> {
        let mut last_error = None;
        for attempt in 0..=self.retry_count {
            match self.chat_once(request, stream).await {
                Ok(response) => return Ok(response),
                Err(error) if is_retryable_llm_error(&error) && attempt < self.retry_count => {
                    let delay = provider_retry_delay(attempt);
                    warn!(
                        "Anthropic 请求失败，{}ms 后重试 ({}/{}): {}",
                        delay.as_millis(),
                        attempt + 1,
                        self.retry_count,
                        error
                    );
                    tokio::time::sleep(delay).await;
                    last_error = Some(error);
                }
                Err(error) => return Err(error),
            }
        }
        Err(last_error.unwrap_or_else(|| AppError::Llm("Anthropic 请求重试耗尽".to_string())))
    }
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    fn provider_name(&self) -> &str {
        PROVIDER_NAME
    }

    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, AppError> {
        let model = self.effective_model(&request);
        let source_profile = self.source_profile(&model);
        self.run_non_stream_with_stale(async {
            let response = self.chat_with_retry(&request, false).await?;
            let bytes = response.bytes().await.map_err(|error| {
                llm_error_with_source(
                    PROVIDER_NAME,
                    LlmErrorStage::BodyRead,
                    "读取 Anthropic 响应失败".to_string(),
                    anyhow::anyhow!(error),
                )
            })?;
            let raw: serde_json::Value = serde_json::from_slice(&bytes).map_err(|error| {
                llm_error_with_source(
                    PROVIDER_NAME,
                    LlmErrorStage::Parse,
                    "解析 Anthropic JSON 失败".to_string(),
                    anyhow::anyhow!(error),
                )
            })?;
            Ok(wire::response_to_chat_response(
                &raw,
                &source_profile,
                self.continuity_enabled,
            ))
        })
        .await
    }

    async fn chat_stream(
        &self,
        request: ChatRequest,
    ) -> Result<Box<dyn Stream<Item = Result<StreamEvent, AppError>> + Send + Unpin>, AppError>
    {
        let model = self.effective_model(&request);
        let response = self.chat_with_retry(&request, true).await?;
        let source_profile = self.source_profile(&model);
        Ok(Box::new(stream::AnthropicStream::new(
            response.bytes_stream(),
            source_profile,
            self.continuity_enabled,
        )))
    }

    fn count_tokens(&self, messages: &[ChatMessage]) -> Result<u32, AppError> {
        let total_chars: usize = messages
            .iter()
            .map(|message| match &message.content {
                Some(ChatMessageContent::Text(text)) => text.chars().count(),
                Some(ChatMessageContent::Parts(parts)) => parts
                    .iter()
                    .map(|part| part.estimated_chars())
                    .sum::<usize>(),
                None => 0,
            })
            .sum();
        Ok((total_chars / 3).max(1) as u32)
    }
}
