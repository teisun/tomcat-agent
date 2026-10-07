use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::RwLock;
use tokio_stream::{Stream, StreamExt};
use tracing::warn;

use crate::core::llm::endpoint::build_path_aware_endpoint;
use crate::core::llm::files_api::FilesApiAdapter;
use crate::core::llm::provider::LlmProvider;
use crate::core::llm::replay_policy::ProviderCompatProfile;
use crate::core::llm::retry_delay::{provider_retry_delay, sleep_provider_retry_delay};
use crate::core::llm::types::{
    ChatMessage, ChatMessageContent, ChatMessageContentPart, ChatRequest, ChatResponse, FileSource,
    ImageSource, StreamEvent,
};
use crate::infra::config::{LlmFilesConfig, LlmRuntimeConfig};
use crate::infra::error::{
    is_retryable_llm_error, llm_error, llm_error_with_source, llm_http_status_error,
    llm_retry_after_ms, AppError, LlmErrorStage,
};

use super::super::auth::Credential;
use super::super::catalog::{infer_default_base_url, Capabilities, ModelEntry};
use super::super::thinking_policy::ThinkingFormat;
use crate::core::llm::{AnthropicFilesAdapter, FilesApiProviderContext, ANTHROPIC_FILES_BETA};

mod stream;
mod wire;

const PROVIDER_NAME: &str = "anthropic";

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

pub(super) struct AnthropicProvider {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    default_model: String,
    catalog_model_id: String,
    retry_count: u32,
    stream_timeout_sec: u64,
    non_stream_stale_timeout_sec: u64,
    files_adapter: std::sync::OnceLock<Arc<dyn FilesApiAdapter>>,
    files_expires_after_seconds: u64,
    thinking_cfg: crate::infra::config::ThinkingConfig,
    configured_thinking_format: ThinkingFormat,
    /// Session-scoped compatibility result. A successful adaptive↔budget
    /// fallback is reused by this cached provider instead of causing a 400 on
    /// every subsequent request. It deliberately is not persisted to models.toml:
    /// one transient upstream failure must not rewrite user configuration.
    learned_thinking_format: RwLock<Option<ThinkingFormat>>,
    continuity_enabled: bool,
    capabilities: Capabilities,
}

impl AnthropicProvider {
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
            &super::ProviderRoute::new(runtime)?,
        )
    }

    pub(super) fn with_route(
        entry: &ModelEntry,
        runtime: &LlmRuntimeConfig,
        credential: &Credential,
        route: &super::ProviderRoute,
    ) -> Result<Self, AppError> {
        let client = route.client.clone();
        let base_url = entry
            .base_url
            .clone()
            .or_else(|| infer_default_base_url(Some(entry.provider.as_str())))
            .ok_or_else(|| AppError::Config(format!("模型 `{}` 缺少 base_url。", entry.id)))?;
        let configured_thinking_format = ThinkingFormat::parse_or_auto(
            entry
                .thinking_format
                .as_deref()
                .or(runtime.thinking.format.as_deref()),
        );
        Ok(Self {
            client,
            base_url,
            api_key: credential.value.clone(),
            default_model: entry.request_model_name().to_string(),
            catalog_model_id: entry.id.clone(),
            retry_count: runtime.retry_count,
            stream_timeout_sec: runtime.stream_timeout_sec,
            non_stream_stale_timeout_sec: runtime.non_stream_stale_timeout_sec,
            files_adapter: std::sync::OnceLock::new(),
            files_expires_after_seconds: runtime.files.expires_after_seconds,
            thinking_cfg: runtime.thinking.clone(),
            configured_thinking_format,
            learned_thinking_format: RwLock::new(None),
            continuity_enabled: runtime.reasoning_continuity.enabled,
            capabilities: entry.capabilities.clone(),
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

    fn thinking_format_for_wire(&self) -> ThinkingFormat {
        learned_or_configured_thinking_format(
            &self.learned_thinking_format,
            self.configured_thinking_format
                .resolve_for_api(PROVIDER_NAME),
        )
    }

    fn remember_compatible_thinking_format(&self, format: ThinkingFormat) {
        *self.learned_thinking_format.write() = Some(format);
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

    fn cached_files_adapter(&self, files_cfg: &LlmFilesConfig) -> Option<Arc<dyn FilesApiAdapter>> {
        if !self.capabilities.files {
            return None;
        }
        let expires = if files_cfg.expires_after_seconds == self.files_expires_after_seconds {
            self.files_expires_after_seconds
        } else {
            files_cfg.expires_after_seconds
        };
        let cfg = LlmFilesConfig {
            expires_after_seconds: expires,
        };
        let adapter = self.files_adapter.get_or_init(|| {
            Arc::new(AnthropicFilesAdapter::from_provider_context(
                FilesApiProviderContext {
                    client: self.client.clone(),
                    base_url: self.base_url.clone(),
                    api_key: self.api_key.clone(),
                    retry_count: self.retry_count,
                },
                &cfg,
            ))
        });
        Some(adapter.clone())
    }

    fn auth_headers(
        &self,
        builder: reqwest::RequestBuilder,
        include_files_beta: bool,
    ) -> reqwest::RequestBuilder {
        let builder = builder
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01");
        if include_files_beta {
            builder.header("anthropic-beta", ANTHROPIC_FILES_BETA)
        } else {
            builder
        }
    }

    fn request_uses_uploaded_files(messages: &[ChatMessage]) -> bool {
        messages.iter().any(|message| {
            let Some(ChatMessageContent::Parts(parts)) = &message.content else {
                return false;
            };
            parts.iter().any(|part| {
                matches!(
                    part,
                    ChatMessageContentPart::InputImage {
                        source: ImageSource::Uploaded(_),
                        ..
                    } | ChatMessageContentPart::InputFile {
                        source: FileSource::Uploaded(_),
                    }
                )
            })
        })
    }

    async fn chat_once(
        &self,
        request: &ChatRequest,
        stream: bool,
    ) -> Result<reqwest::Response, AppError> {
        self.chat_once_with_thinking_format(request, stream, self.thinking_format_for_wire())
            .await
    }

    async fn chat_once_with_thinking_format(
        &self,
        request: &ChatRequest,
        stream: bool,
        thinking_format: ThinkingFormat,
    ) -> Result<reqwest::Response, AppError> {
        let model = self.effective_model(request);
        let thinking_cfg = self.thinking_cfg_for_request(request);
        let files_cfg = LlmFilesConfig {
            expires_after_seconds: self.files_expires_after_seconds,
        };
        let files_adapter = self.cached_files_adapter(&files_cfg);
        let body = wire::build_request_body(
            request,
            &model,
            &thinking_cfg,
            thinking_format,
            self.continuity_enabled,
            stream,
            &self.capabilities,
            files_adapter.as_deref(),
        );
        let url = build_path_aware_endpoint(&self.base_url, "messages");
        let include_files_beta =
            self.capabilities.files && Self::request_uses_uploaded_files(&request.messages);
        let response = self
            .auth_headers(self.client.post(url), include_files_beta)
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
            let retry_after_ms = parse_retry_after_ms(response.headers());
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "anthropic error".to_string());
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
        let mut thinking_format_fallback_used = false;
        for attempt in 0..=self.retry_count {
            match self.chat_once(request, stream).await {
                Ok(response) => return Ok(response),
                Err(error)
                    if !thinking_format_fallback_used
                        && is_thinking_format_rejection(&error)
                        && alternative_thinking_format(self.thinking_format_for_wire())
                            .is_some() =>
                {
                    thinking_format_fallback_used = true;
                    let fallback = alternative_thinking_format(self.thinking_format_for_wire())
                        .expect("guarded above");
                    warn!(
                        ?fallback,
                        "Anthropic rejected the configured thinking format; retrying once with its compatible counterpart"
                    );
                    match self
                        .chat_once_with_thinking_format(request, stream, fallback)
                        .await
                    {
                        Ok(response) => {
                            self.remember_compatible_thinking_format(fallback);
                            warn!(
                                ?fallback,
                                "Anthropic thinking format fallback succeeded; reusing it for this runtime"
                            );
                            return Ok(response);
                        }
                        Err(fallback_error) => last_error = Some(fallback_error),
                    }
                }
                Err(error) if is_retryable_llm_error(&error) && attempt < self.retry_count => {
                    let delay = llm_retry_after_ms(&error)
                        .map(Duration::from_millis)
                        .unwrap_or_else(|| provider_retry_delay(attempt));
                    warn!(
                        "Anthropic 请求失败，{}ms 后重试 ({}/{}): {}",
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
        Err(last_error.unwrap_or_else(|| AppError::Llm("Anthropic 请求重试耗尽".to_string())))
    }
}

fn alternative_thinking_format(format: ThinkingFormat) -> Option<ThinkingFormat> {
    match format {
        ThinkingFormat::AnthropicAdaptive => Some(ThinkingFormat::Anthropic),
        ThinkingFormat::Anthropic => Some(ThinkingFormat::AnthropicAdaptive),
        _ => None,
    }
}

fn learned_or_configured_thinking_format(
    learned: &RwLock<Option<ThinkingFormat>>,
    configured: ThinkingFormat,
) -> ThinkingFormat {
    (*learned.read()).unwrap_or(configured)
}

fn is_thinking_format_rejection(error: &AppError) -> bool {
    let text = error.to_string().to_ascii_lowercase();
    (text.contains("adaptive")
        && (text.contains("unsupported")
            || text.contains("not supported")
            || text.contains("not available")))
        || (text.contains("budget_tokens")
            && (text.contains("unsupported")
                || text.contains("not supported")
                || text.contains("not accepted")
                || text.contains("invalid")))
}

fn idle_timeout_error(stream_timeout_sec: u64) -> AppError {
    llm_error(
        PROVIDER_NAME,
        LlmErrorStage::IdleTimeout,
        format!("流式空闲超时: stream_timeout_sec={}s", stream_timeout_sec),
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
        let event_stream = stream::AnthropicStream::new(
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

    fn files_adapter(&self, files_cfg: &LlmFilesConfig) -> Option<Arc<dyn FilesApiAdapter>> {
        self.cached_files_adapter(files_cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::llm::tests::mocks::{MockHttpServer, ScriptedHttpResponse};
    use crate::core::llm::{ChatRequest, Credential, LlmProvider};
    use crate::infra::LlmConfig;
    use bytes::Bytes;
    use tokio_stream::wrappers::IntervalStream;
    use tokio_stream::StreamExt;

    #[test]
    fn parse_retry_after_ms_converts_integer_seconds() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::RETRY_AFTER,
            reqwest::header::HeaderValue::from_static("17"),
        );

        assert_eq!(parse_retry_after_ms(&headers), Some(17_000));
    }

    #[test]
    fn parse_retry_after_ms_converts_rfc2822_http_date() {
        let retry_at = chrono::Utc::now() + chrono::Duration::hours(1);
        let raw = retry_at.format("%a, %d %b %Y %H:%M:%S GMT").to_string();
        let retry_at = chrono::DateTime::parse_from_rfc2822(&raw)
            .expect("formatted HTTP date must remain RFC2822-compatible")
            .with_timezone(&chrono::Utc);
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::RETRY_AFTER,
            raw.parse().expect("valid HTTP header value"),
        );

        let before = chrono::Utc::now();
        let delay_ms = parse_retry_after_ms(&headers).expect("HTTP date must produce a delay");
        let after = chrono::Utc::now();
        let min_delay_ms = retry_at
            .signed_duration_since(after)
            .num_milliseconds()
            .max(0) as u64;
        let max_delay_ms = retry_at
            .signed_duration_since(before)
            .num_milliseconds()
            .max(0) as u64;

        assert!(
            (min_delay_ms..=max_delay_ms).contains(&delay_ms),
            "parsed delay {delay_ms}ms must be bounded by its two observed UTC instants \
             [{min_delay_ms}, {max_delay_ms}]ms"
        );
    }

    #[test]
    fn switches_only_between_the_two_anthropic_thinking_formats() {
        assert_eq!(
            alternative_thinking_format(ThinkingFormat::AnthropicAdaptive),
            Some(ThinkingFormat::Anthropic)
        );
        assert_eq!(
            alternative_thinking_format(ThinkingFormat::Anthropic),
            Some(ThinkingFormat::AnthropicAdaptive)
        );
        assert_eq!(alternative_thinking_format(ThinkingFormat::Openai), None);
        assert!(is_thinking_format_rejection(&AppError::Llm(
            "400 adaptive thinking is not available for this model".to_string()
        )));
        assert!(is_thinking_format_rejection(&AppError::Llm(
            "400 adaptive thinking is not supported for this model".to_string()
        )));
        assert!(is_thinking_format_rejection(&AppError::Llm(
            "400 budget_tokens is not accepted".to_string()
        )));
        assert!(!is_thinking_format_rejection(&AppError::Llm(
            "400 malformed tool input".to_string()
        )));
    }

    #[test]
    fn learned_thinking_format_overrides_only_the_current_runtime() {
        let learned = RwLock::new(None);
        assert_eq!(
            learned_or_configured_thinking_format(&learned, ThinkingFormat::AnthropicAdaptive),
            ThinkingFormat::AnthropicAdaptive
        );

        *learned.write() = Some(ThinkingFormat::Anthropic);
        assert_eq!(
            learned_or_configured_thinking_format(&learned, ThinkingFormat::AnthropicAdaptive),
            ThinkingFormat::Anthropic
        );
    }

    #[tokio::test]
    async fn rejected_thinking_format_is_retried_once_and_cached_for_this_provider() {
        let server = MockHttpServer::start(vec![
            ScriptedHttpResponse::json(
                400,
                r#"{"error":{"message":"adaptive thinking is not supported for this model"}}"#,
            ),
            ScriptedHttpResponse::json(
                200,
                r#"{"id":"msg_1","type":"message","role":"assistant","content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}}"#,
            ),
            ScriptedHttpResponse::json(
                200,
                r#"{"id":"msg_2","type":"message","role":"assistant","content":[{"type":"text","text":"still ok"}],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}}"#,
            ),
            ScriptedHttpResponse::json(
                200,
                r#"{"id":"msg_b","type":"message","role":"assistant","content":[{"type":"text","text":"B keeps adaptive"}],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}}"#,
            ),
        ])
        .await;
        let entry = ModelEntry {
            id: "thinking-fallback-test".to_string(),
            model_name: Some("thinking-fallback-test".to_string()),
            api: "anthropic-messages".to_string(),
            provider: "anthropic".to_string(),
            api_key_env: None,
            base_url: Some(server.base_url.clone()),
            capabilities: Capabilities {
                reasoning: true,
                ..Capabilities::default()
            },
            context_window: Some(200_000),
            context_window_options: Vec::new(),
            max_output_tokens: Some(32_000),
            description: None,
            supported_speeds: Vec::new(),
            thinking_format: Some("anthropic-adaptive".to_string()),
            supported_reasoning_levels: vec!["high".to_string()],
        };
        let runtime = LlmConfig::default().runtime();
        let credential = Credential {
            provider: "anthropic".to_string(),
            env_name: "TEST_KEY".to_string(),
            value: "stub".to_string(),
        };
        let route = super::super::ProviderRoute::new(&runtime).expect("shared route");
        let provider = AnthropicProvider::with_route(&entry, &runtime, &credential, &route)
            .expect("provider A");
        let mut entry_b = entry.clone();
        entry_b.id = "thinking-model-b".into();
        entry_b.model_name = Some(entry_b.id.clone());
        let provider_b = AnthropicProvider::with_route(&entry_b, &runtime, &credential, &route)
            .expect("provider B on the same route");
        let request = ChatRequest {
            messages: vec![ChatMessage::user("hello")],
            model: entry.id.clone(),
            temperature: None,
            max_tokens: None,
            resolved_output_limit: None,
            diagnostic_request_id: None,
            stream: Some(false),
            model_override: None,
            speed: None,
            thinking_level: None,
            cache_key: None,
            tools: None,
        };

        assert_eq!(
            provider
                .chat(request.clone())
                .await
                .expect("fallback request")
                .choices[0]
                .message
                .text_content(),
            Some("ok")
        );
        provider
            .chat(request.clone())
            .await
            .expect("cached fallback request");
        provider_b
            .chat(ChatRequest {
                model: entry_b.id,
                ..request
            })
            .await
            .expect("B request");

        assert_eq!(server.request_count(), 4);
        let requests = server.request_texts();
        assert!(requests[0].contains(r#""type":"adaptive""#));
        assert!(requests[1].contains(r#""type":"enabled""#));
        assert!(
            requests[2].contains(r#""type":"enabled""#),
            "the succeeding fallback must be reused instead of repeating the rejected adaptive format"
        );
        assert!(
            requests[3].contains(r#""type":"adaptive""#),
            "A's learned fallback must not leak through the shared route to B"
        );
        server.shutdown().await;
    }

    #[tokio::test]
    async fn cached_provider_uses_each_requests_resolved_output_limit() {
        let server = MockHttpServer::start(vec![
            ScriptedHttpResponse::json(
                200,
                r#"{"id":"msg_1","type":"message","role":"assistant","content":[{"type":"text","text":"first"}],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}}"#,
            ),
            ScriptedHttpResponse::json(
                200,
                r#"{"id":"msg_2","type":"message","role":"assistant","content":[{"type":"text","text":"second"}],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}}"#,
            ),
        ])
        .await;
        let entry = ModelEntry {
            id: "anthropic-small".to_string(),
            model_name: Some("anthropic-small".to_string()),
            api: "anthropic-messages".to_string(),
            provider: "anthropic".to_string(),
            api_key_env: None,
            base_url: Some(server.base_url.clone()),
            capabilities: Capabilities::default(),
            context_window: Some(200_000),
            context_window_options: Vec::new(),
            max_output_tokens: Some(8_192),
            description: None,
            supported_speeds: Vec::new(),
            thinking_format: Some("anthropic-adaptive".to_string()),
            supported_reasoning_levels: vec!["high".to_string()],
        };
        let provider = AnthropicProvider::new(
            &entry,
            &LlmConfig::default().runtime(),
            &Credential {
                provider: "anthropic".to_string(),
                env_name: "TEST_KEY".to_string(),
                value: "stub".to_string(),
            },
        )
        .expect("provider");

        for (model, resolved_output_limit) in
            [("anthropic-small", 8_192), ("anthropic-large", 16_384)]
        {
            provider
                .chat(ChatRequest {
                    messages: vec![ChatMessage::user("hello")],
                    model: model.to_string(),
                    resolved_output_limit: Some(resolved_output_limit),
                    stream: Some(false),
                    ..Default::default()
                })
                .await
                .expect("request succeeds");
        }

        let requests = server.request_texts();
        assert!(requests[0].contains(r#""max_tokens":8192"#));
        assert!(
            requests[1].contains(r#""max_tokens":16384"#),
            "the second model's resolved cap must not inherit the provider construction entry"
        );
        server.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn keepalive_bytes_still_trigger_idle_timeout_when_no_events_arrive() {
        let interval = tokio::time::interval(Duration::from_millis(200));
        let source =
            IntervalStream::new(interval).map(|_| Ok(Bytes::from_static(b": keepalive\n\n")));
        let event_stream = stream::AnthropicStream::new(
            source,
            ProviderCompatProfile::anthropic_messages("claude-opus-4-8"),
            true,
        );
        let mut stream = apply_stream_idle_timeout(event_stream, 1);
        let next_task = tokio::spawn(async move { stream.next().await });

        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(2)).await;

        let item = next_task
            .await
            .expect("join ok")
            .expect("should produce timeout error");
        match item {
            Err(err) => {
                assert_eq!(
                    crate::infra::error::llm_stage(&err),
                    Some(LlmErrorStage::IdleTimeout)
                );
                let msg = crate::infra::error::llm_summary(&err).unwrap_or_else(|| err.to_string());
                assert!(
                    msg.contains("stream_timeout_sec=1s"),
                    "unexpected msg: {}",
                    msg
                );
            }
            other => panic!("expected timeout AppError, got {:?}", other),
        }
    }
}
