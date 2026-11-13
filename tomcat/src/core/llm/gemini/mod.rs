//! Native Google Gemini `/v1beta/models` provider (`api = "google-gemini"`).

use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::Semaphore;
use tokio_stream::{Stream, StreamExt};
use tracing::warn;

use crate::core::llm::endpoint::gemini_api_root;
use crate::core::llm::provider::LlmProvider;
use crate::core::llm::replay_policy::{credential_fingerprint, ProviderCompatProfile};
use crate::core::llm::retry_delay::{provider_retry_delay, sleep_provider_retry_delay};
use crate::core::llm::thinking_policy::{
    lowest_supported_reasoning_level, thinking_config_for_request, ThinkingLevel,
};
use crate::core::llm::types::{
    ChatMessage, ChatMessageContent, ChatRequest, ChatResponse, StreamEvent,
};
use crate::infra::config::{LlmRuntimeConfig, ThinkingConfig};
use crate::infra::error::{
    is_retryable_llm_error, llm_error, llm_error_with_source, llm_http_status,
    llm_http_status_error, llm_retry_after_ms, llm_summary, AppError, LlmErrorStage,
};
use crate::infra::i18n::tr;

use super::super::auth::Credential;
use super::super::catalog::{infer_default_base_url, Capabilities, ModelEntry};

mod stream;
mod wire;

pub(super) const PROVIDER_NAME: &str = "google-gemini";

fn parse_retry_after_ms(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    let raw = headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim();
    if let Ok(seconds) = raw.parse::<u64>() {
        return Some(seconds.saturating_mul(1_000));
    }
    let retry_at = chrono::DateTime::parse_from_rfc2822(raw).ok()?;
    let remaining = retry_at
        .with_timezone(&chrono::Utc)
        .signed_duration_since(chrono::Utc::now())
        .num_milliseconds();
    Some(remaining.max(0) as u64)
}

fn is_thought_signature_rejection(error: &AppError) -> bool {
    llm_http_status(error) == Some(400)
        && llm_summary(error).is_some_and(|summary| {
            let lower = summary.to_ascii_lowercase();
            lower.contains("thought signature") || lower.contains("thought_signature")
        })
}

pub(super) struct GeminiProvider {
    client: reqwest::Client,
    semaphore: Option<Arc<Semaphore>>,
    api_root: String,
    provider_id: String,
    api_key: String,
    credential_fingerprint: String,
    default_model: String,
    catalog_model_id: String,
    retry_count: u32,
    stream_timeout_sec: u64,
    non_stream_stale_timeout_sec: u64,
    thinking_cfg: ThinkingConfig,
    supported_reasoning_levels: Vec<String>,
    lowest_reasoning_level: Option<ThinkingLevel>,
    continuity_enabled: bool,
    capabilities: Capabilities,
}

impl GeminiProvider {
    #[cfg(test)]
    pub(super) fn new(
        entry: &ModelEntry,
        runtime: &LlmRuntimeConfig,
        credential: &Credential,
    ) -> Result<Self, AppError> {
        Self::with_route(
            entry,
            runtime,
            credential,
            &super::ProviderRoute::new(runtime, entry.base_url.as_deref())?,
        )
    }

    pub(super) fn with_route(
        entry: &ModelEntry,
        runtime: &LlmRuntimeConfig,
        credential: &Credential,
        route: &super::ProviderRoute,
    ) -> Result<Self, AppError> {
        let raw_base_url = entry
            .base_url
            .clone()
            .or_else(|| infer_default_base_url(Some(entry.provider.as_str())))
            .or_else(|| infer_default_base_url(Some(entry.api.as_str())))
            .ok_or_else(|| AppError::Config(tr("llm.baseUrlMissing", &[("id", &entry.id)])))?;
        let api_root = gemini_api_root(&raw_base_url);
        let cred_fp = credential_fingerprint(&credential.value);
        Ok(Self {
            client: route.client.clone(),
            semaphore: route.semaphore.clone(),
            api_root,
            provider_id: entry.provider.clone(),
            api_key: credential.value.clone(),
            credential_fingerprint: cred_fp,
            default_model: entry.request_model_name().to_string(),
            catalog_model_id: entry.id.clone(),
            retry_count: runtime.retry_count,
            stream_timeout_sec: runtime.stream_timeout_sec,
            non_stream_stale_timeout_sec: runtime.non_stream_stale_timeout_sec,
            thinking_cfg: runtime.thinking.clone(),
            supported_reasoning_levels: entry.supported_reasoning_levels.clone(),
            lowest_reasoning_level: lowest_supported_reasoning_level(
                &entry.supported_reasoning_levels,
            ),
            continuity_enabled: runtime.reasoning_continuity.enabled,
            capabilities: entry.capabilities.clone(),
        })
    }

    fn effective_model(&self, request: &ChatRequest) -> String {
        let req_model = request.model.trim();
        let chosen = if req_model.is_empty() || req_model == self.catalog_model_id {
            self.default_model.as_str()
        } else {
            req_model
        };
        chosen.strip_prefix("models/").unwrap_or(chosen).to_string()
    }

    fn source_profile(&self, model: &str) -> ProviderCompatProfile {
        ProviderCompatProfile::google_gemini_routed(
            model,
            &self.provider_id,
            &self.api_root,
            &self.credential_fingerprint,
        )
    }

    fn thinking_cfg_for_request<'a>(&'a self, request: &ChatRequest) -> Cow<'a, ThinkingConfig> {
        thinking_config_for_request(
            &self.thinking_cfg,
            request.thinking_level,
            self.lowest_reasoning_level,
        )
    }

    fn endpoint_url(&self, model: &str, stream: bool) -> String {
        if stream {
            format!(
                "{}/models/{model}:streamGenerateContent?alt=sse",
                self.api_root
            )
        } else {
            format!("{}/models/{model}:generateContent", self.api_root)
        }
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
                tr(
                    "llm.geminiStale",
                    &[("seconds", &self.non_stream_stale_timeout_sec.to_string())],
                ),
            )),
        }
    }

    async fn chat_once(
        &self,
        request: &ChatRequest,
        stream: bool,
        replay_opaque: bool,
    ) -> Result<reqwest::Response, AppError> {
        let _permit = match &self.semaphore {
            Some(sem) => Some(sem.clone().acquire_owned().await.map_err(|e| {
                AppError::Llm(tr("llm.semaphoreClosed", &[("detail", &e.to_string())]))
            })?),
            None => None,
        };

        let model = self.effective_model(request);
        let target = self.source_profile(&model);
        let thinking_cfg = self.thinking_cfg_for_request(request);
        let body = wire::build_request_body(
            request,
            &target,
            &thinking_cfg,
            &self.supported_reasoning_levels,
            replay_opaque,
            &self.capabilities,
        );
        let url = self.endpoint_url(&model, stream);
        let response = self
            .client
            .post(url)
            .header("x-goog-api-key", &self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|error| {
                llm_error_with_source(
                    PROVIDER_NAME,
                    if error.is_connect() {
                        LlmErrorStage::Connect
                    } else if error.is_timeout() {
                        LlmErrorStage::ReadTimeout
                    } else {
                        LlmErrorStage::Send
                    },
                    tr(
                        if stream {
                            "llm.geminiStreamFailed"
                        } else {
                            "llm.geminiRequestFailed"
                        },
                        &[],
                    ),
                    anyhow::anyhow!(error),
                )
            })?;
        let status = response.status();
        if !status.is_success() {
            let retry_after_ms = parse_retry_after_ms(response.headers());
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "gemini error".to_string());
            let error = llm_http_status_error(PROVIDER_NAME, status.as_u16(), body);
            return Err(match (error, retry_after_ms) {
                (AppError::LlmDetailed(detail), Some(delay)) => {
                    AppError::LlmDetailed(Box::new((*detail).with_retry_after_ms(delay)))
                }
                (error, _) => error,
            });
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
            match self
                .chat_once(request, stream, self.continuity_enabled)
                .await
            {
                Ok(response) => return Ok(response),
                Err(error) if is_thought_signature_rejection(&error) => {
                    warn!(
                        "Gemini 思考签名被拒，剥离本次请求的原始片段后重发一次: {}",
                        error
                    );
                    // Independent of retry_count; a failed recovery is returned as-is.
                    // Only outbound replay changes: response capture stays enabled.
                    return self.chat_once(request, stream, false).await;
                }
                Err(error) if is_retryable_llm_error(&error) && attempt < self.retry_count => {
                    let delay = llm_retry_after_ms(&error)
                        .map(Duration::from_millis)
                        .unwrap_or_else(|| provider_retry_delay(attempt));
                    warn!(
                        "Gemini 请求失败，{}ms 后重试 ({}/{}): {}",
                        delay.as_millis(),
                        attempt + 1,
                        self.retry_count,
                        error
                    );
                    sleep_provider_retry_delay(delay).await?;
                    last_error = Some(error);
                }
                Err(error) => return Err(error),
            }
        }
        Err(last_error.unwrap_or_else(|| AppError::Llm(tr("llm.geminiRetries", &[]))))
    }
}

fn idle_timeout_error(stream_timeout_sec: u64) -> AppError {
    llm_error(
        PROVIDER_NAME,
        LlmErrorStage::IdleTimeout,
        tr(
            "llm.idleTimeout",
            &[("seconds", &stream_timeout_sec.to_string())],
        ),
    )
}

fn apply_stream_idle_timeout<S, T>(
    stream: S,
    stream_timeout_sec: u64,
) -> Pin<Box<dyn Stream<Item = Result<T, AppError>> + Send>>
where
    S: Stream<Item = Result<T, AppError>> + Send + 'static,
    T: Send + 'static,
{
    if stream_timeout_sec == 0 {
        return Box::pin(stream);
    }
    Box::pin(
        stream
            .timeout(Duration::from_secs(stream_timeout_sec))
            .map(move |item| match item {
                Ok(event) => event,
                Err(_) => Err(idle_timeout_error(stream_timeout_sec)),
            }),
    )
}

#[async_trait]
impl LlmProvider for GeminiProvider {
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
                    tr("llm.geminiReadFailed", &[]),
                    anyhow::anyhow!(error),
                )
            })?;
            let raw: Value = serde_json::from_slice(&bytes).map_err(|error| {
                llm_error_with_source(
                    PROVIDER_NAME,
                    LlmErrorStage::Parse,
                    tr("llm.geminiJsonFailed", &[]),
                    anyhow::anyhow!(error),
                )
            })?;
            let mut acc =
                stream::GeminiResponseAccumulator::new(source_profile, self.continuity_enabled);
            acc.feed_frame(&raw);
            acc.into_chat_response()
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
        let event_stream = stream::GeminiStream::new(
            response.bytes_stream(),
            source_profile,
            self.continuity_enabled,
        );
        Ok(Box::new(apply_stream_idle_timeout(
            event_stream,
            self.stream_timeout_sec,
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

#[cfg(test)]
#[path = "tests/suite_test.rs"]
mod tests;
