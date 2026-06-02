//! # OpenAI 格式适配器
//!
//! 实现 LlmProvider：非流式/流式调用、限流、指数退避重试、流式超时与资源释放。

use async_trait::async_trait;
use bytes::Bytes;
use futures_util::stream::TryStreamExt;
use serde_json::{json, Value};
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio_stream::{Stream, StreamExt};
use tracing::warn;

use crate::core::llm::http_client::build_http_client;
use crate::core::llm::replay_policy::{
    apply_text_downgrade, plan_scoped, replay_requirement_for_profile, CaptureMode,
    ProviderCompatProfile, ReplayAction, ReplayDowngradeReport, ReplayWindow,
};
use crate::infra::error::AppError;
use crate::infra::error::{
    llm_connect_or_network, llm_error, llm_error_with_source, llm_stage, LlmErrorStage,
};
use crate::infra::LlmConfig;

use crate::core::llm::provider::LlmProvider;
use crate::core::llm::types::{
    ChatMessage, ChatMessageContent, ChatRequest, ChatResponse, ContinuityMetadata,
    ReasoningContinuation, ReasoningFormat, StreamEvent, ThinkingSource, TokenUsage,
};

/// 提供商不匹配的固定文案：Completions 路径不接受多模态附件，必须改用 `openai-responses`。
/// 单测和上层 UI 都会按字符串子串断言这个文案，请勿改动。
const COMPLETIONS_REJECT_MULTIMODAL_MSG: &str =
    "provider=openai 不支持多模态附件，请改用 provider=openai-responses";
const PROVIDER_NAME: &str = "openai";

fn idle_timeout_error(stream_timeout_sec: u64) -> AppError {
    llm_error(
        PROVIDER_NAME,
        LlmErrorStage::IdleTimeout,
        format!("流式空闲超时: stream_timeout_sec={}s", stream_timeout_sec),
    )
}

fn request_timeout_summary(http_timeout_sec: u64) -> String {
    format!("整次 HTTP 请求超时: http_timeout_sec={}s", http_timeout_sec)
}

fn non_stream_stale_timeout_error(non_stream_stale_timeout_sec: u64) -> AppError {
    llm_error(
        PROVIDER_NAME,
        LlmErrorStage::NonStreamStale,
        format!(
            "非流式请求长时间无响应: non_stream_stale_timeout_sec={}s",
            non_stream_stale_timeout_sec
        ),
    )
}

fn map_send_error(prefix: &str, err: reqwest::Error, http_timeout_sec: u64) -> AppError {
    if err.is_connect() {
        return llm_error_with_source(
            PROVIDER_NAME,
            LlmErrorStage::Connect,
            format!("{prefix}连接失败"),
            err,
        );
    }
    if err.is_timeout() {
        return llm_error_with_source(
            PROVIDER_NAME,
            LlmErrorStage::RequestTimeout,
            request_timeout_summary(http_timeout_sec),
            err,
        );
    }
    llm_error_with_source(
        PROVIDER_NAME,
        LlmErrorStage::Send,
        format!("{prefix}发送失败"),
        err,
    )
}

fn map_body_read_error(prefix: &str, err: reqwest::Error, http_read_timeout_sec: u64) -> AppError {
    if err.is_timeout() {
        return llm_error_with_source(
            PROVIDER_NAME,
            LlmErrorStage::ReadTimeout,
            format!(
                "{prefix}超时: http_read_timeout_sec={}s",
                http_read_timeout_sec
            ),
            err,
        );
    }
    llm_error_with_source(
        PROVIDER_NAME,
        LlmErrorStage::BodyRead,
        format!("{prefix}失败"),
        err,
    )
}

fn map_parse_error(prefix: &str, err: impl Into<anyhow::Error>) -> AppError {
    llm_error_with_source(
        PROVIDER_NAME,
        LlmErrorStage::Parse,
        format!("{prefix}失败"),
        err,
    )
}

/// 扫描 messages 是否含非 `InputText` part；返回 `Err` 即结构化非可重试错误。
///
/// Completions wire (`/v1/chat/completions`) 默认走文本 + image_url 旁路；本期不为
/// Completions 实现 vision/file 翻译，遇到多模态 part 直接拒绝并把诊断指向
/// `provider=openai-responses`。
fn reject_multimodal_parts(messages: &[ChatMessage]) -> Result<(), AppError> {
    for msg in messages {
        if let Some(ChatMessageContent::Parts(parts)) = &msg.content {
            if parts.iter().any(|p| p.is_non_text()) {
                return Err(AppError::Llm(COMPLETIONS_REJECT_MULTIMODAL_MSG.to_string()));
            }
        }
    }
    Ok(())
}

/// 提取 chat-completions `reasoning_content` continuity blob（deepseek / mimo / 未来同类共用）。
///
/// 只看 continuity 的 `format` 标签，不按厂商名硬编码——这样新增同类模型无需改本函数。
fn chat_completions_reasoning_content(message: &ChatMessage) -> Option<String> {
    let continuation = message.reasoning_continuation.as_ref()?;
    if !matches!(
        continuation.format,
        ReasoningFormat::DeepseekReasoningContent
    ) {
        return None;
    }
    continuation
        .opaque_payload
        .get("reasoning_content")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|text| !text.is_empty())
}

fn inject_reasoning_content(message: ChatMessage, reasoning_content: &str) -> Value {
    let mut value = serde_json::to_value(message).unwrap_or_else(|_| json!({}));
    if let Value::Object(ref mut obj) = value {
        obj.insert(
            "reasoning_content".to_string(),
            Value::String(reasoning_content.to_string()),
        );
    }
    value
}

fn transport_messages(
    messages: &[ChatMessage],
    model: &str,
    continuity_enabled: bool,
) -> Vec<Value> {
    let target = ProviderCompatProfile::chat_completions(model);
    let window = ReplayWindow::compute(messages);
    let mut report = ReplayDowngradeReport::default();
    let mut out = Vec::with_capacity(messages.len());
    for (idx, original) in messages.iter().enumerate() {
        let in_window = window.contains(idx);
        let action = if continuity_enabled {
            plan_scoped(&target, original, in_window)
        } else {
            ReplayAction::StripOpaque
        };
        if continuity_enabled {
            if in_window {
                report.record_in_window(&target, original, &action);
            } else {
                report.record_stripped_old_history(original);
            }
        }
        let value = match action {
            ReplayAction::KeepOpaque => {
                let message = original.without_completion_metadata();
                if let Some(reasoning_content) = chat_completions_reasoning_content(original) {
                    inject_reasoning_content(message, &reasoning_content)
                } else {
                    if continuity_enabled
                        && original.reasoning_continuation.is_some()
                        && matches!(target.capture_mode, CaptureMode::ReasoningContent)
                    {
                        warn!(
                            target_profile = %target.profile_id,
                            source_model = %target.model_family,
                            had_tool_call = original
                                .continuity
                                .as_ref()
                                .map(|meta| meta.had_tool_call)
                                .unwrap_or(false),
                            "reasoning_content continuity marked KeepOpaque but payload missing; sending request without local hard intercept"
                        );
                    }
                    serde_json::to_value(message).unwrap_or_else(|_| json!({}))
                }
            }
            ReplayAction::ConvertToText(text) => {
                serde_json::to_value(apply_text_downgrade(original, &text))
                    .unwrap_or_else(|_| json!({}))
            }
            ReplayAction::StripOpaque => serde_json::to_value(original.without_completion_metadata())
                .unwrap_or_else(|_| json!({})),
        };
        out.push(value);
    }
    if continuity_enabled {
        report.emit(&target);
    }
    out
}

const THINK_OPEN_MARKERS: [&str; 2] = ["<think>", "<reasoning>"];
const THINK_CLOSE_MARKERS: [&str; 2] = ["</think>", "</reasoning>"];

#[derive(Debug, Default)]
struct ThinkScrubber {
    pending: String,
    inside_hidden_block: bool,
}

#[derive(Debug, Default)]
struct ScrubbedDelta {
    visible: Option<String>,
    hidden: Option<String>,
}

impl ThinkScrubber {
    fn push(&mut self, delta: &str) -> ScrubbedDelta {
        self.pending.push_str(delta);
        self.drain(false)
    }

    fn finish(&mut self) -> ScrubbedDelta {
        self.drain(true)
    }

    fn drain(&mut self, flush: bool) -> ScrubbedDelta {
        let mut visible = String::new();
        let mut hidden = String::new();

        loop {
            if self.pending.is_empty() {
                break;
            }

            if self.inside_hidden_block {
                if let Some((idx, marker_len)) =
                    earliest_marker(&self.pending, &THINK_CLOSE_MARKERS)
                {
                    hidden.push_str(&self.pending[..idx]);
                    self.pending.drain(..idx + marker_len);
                    self.inside_hidden_block = false;
                    continue;
                }

                let keep = if flush {
                    0
                } else {
                    longest_partial_marker_suffix(&self.pending, &THINK_CLOSE_MARKERS)
                };
                let emit_len = self.pending.len().saturating_sub(keep);
                if emit_len == 0 {
                    break;
                }
                hidden.push_str(&self.pending[..emit_len]);
                self.pending.drain(..emit_len);
                if !flush {
                    break;
                }
                continue;
            }

            if let Some((idx, marker_len)) = earliest_marker(&self.pending, &THINK_OPEN_MARKERS) {
                visible.push_str(&self.pending[..idx]);
                self.pending.drain(..idx + marker_len);
                self.inside_hidden_block = true;
                continue;
            }

            let keep = if flush {
                0
            } else {
                longest_partial_marker_suffix(&self.pending, &THINK_OPEN_MARKERS)
            };
            let emit_len = self.pending.len().saturating_sub(keep);
            if emit_len == 0 {
                break;
            }
            visible.push_str(&self.pending[..emit_len]);
            self.pending.drain(..emit_len);
            if !flush {
                break;
            }
        }

        ScrubbedDelta {
            visible: (!visible.is_empty()).then_some(visible),
            hidden: (!hidden.is_empty()).then_some(hidden),
        }
    }
}

fn earliest_marker(buffer: &str, markers: &[&str]) -> Option<(usize, usize)> {
    markers
        .iter()
        .filter_map(|marker| buffer.find(marker).map(|idx| (idx, marker.len())))
        .min_by_key(|(idx, _)| *idx)
}

fn longest_partial_marker_suffix(buffer: &str, markers: &[&str]) -> usize {
    let max_len = markers
        .iter()
        .map(|marker| marker.len().saturating_sub(1))
        .max()
        .unwrap_or(0)
        .min(buffer.len());
    for suffix_len in (1..=max_len).rev() {
        let suffix_start = buffer.len() - suffix_len;
        if !buffer.is_char_boundary(suffix_start) {
            continue;
        }
        let suffix = &buffer[suffix_start..];
        if markers.iter().any(|marker| marker.starts_with(suffix)) {
            return suffix_len;
        }
    }
    0
}

/// 发给 OpenAI API 的请求体（不含 model_override，stream 由调用方定）。
/// 使用 max_completion_tokens 以兼容新模型（部分模型已不再接受 max_tokens）。
///
/// `reasoning_effort` / `thinking` 是 T2-P0-006 P2a 阶段引入的可选字段：
/// - `reasoning_effort`：OpenAI 系（gpt-5.x 等）与 DeepSeek thinking mode 使用；
/// - `thinking`：DeepSeek、豆包 / Moonshot / 部分网关使用 `{"type": "enabled", ...}` 对象；
/// - 大多数 provider 二选一；DeepSeek OpenAI 兼容格式会同时写这两个字段；
/// - 本期均默认 `None`，与 LlmConfig 默认行为一致（不改变现网请求体）。
///
/// 详见 `docs/architecture/llm-stream-events-cli-pipeline.md` §4.2.1 / §4.2.2。
#[derive(serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct OpenAiRequestBody {
    model: String,
    messages: Vec<Value>,
    temperature: Option<f32>,
    #[serde(rename = "max_completion_tokens")]
    max_tokens: Option<u32>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<StreamOptionsBody>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<serde_json::Value>,
}

#[derive(serde::Serialize)]
struct StreamOptionsBody {
    include_usage: bool,
}

/// OpenAI 兼容 API 的适配器；限流、重试、超时、代理与 fallback 由本实现负责。
#[derive(Debug)]
pub struct OpenAiProvider {
    client: reqwest::Client,
    base_url: String,
    /// 主 base 不通时自动用此 URL 重试；None 表示不降级。
    api_base_fallback: Option<String>,
    api_key: String,
    default_model: String,
    /// 并发上限，None 表示不限制（仅当 max_concurrent_requests == 0）。
    semaphore: Option<Semaphore>,
    retry_count: u32,
    http_timeout_sec: u64,
    /// 流式空闲超时（秒）；0 表示关闭逐事件超时。
    stream_timeout_sec: u64,
    non_stream_stale_timeout_sec: u64,
    http_read_timeout_sec: u64,
    /// T2-P0-006 P5：thinking 子配置；`enabled=false` 时 build_request 不会写任何 reasoning 字段。
    thinking_cfg: crate::infra::config::ThinkingConfig,
    /// 用户显式配置的 thinking format；`Auto` 时按请求实际 model 决定。
    configured_thinking_format: crate::core::llm::thinking_policy::ThinkingFormat,
    continuity_enabled: bool,
}

fn apply_stream_idle_timeout<S>(
    stream: S,
    stream_timeout_sec: u64,
) -> Pin<Box<dyn Stream<Item = Result<Bytes, AppError>> + Send>>
where
    S: Stream<Item = Result<Bytes, AppError>> + Send + 'static,
{
    if stream_timeout_sec == 0 {
        return Box::pin(stream);
    }

    Box::pin(
        stream
            .timeout(Duration::from_secs(stream_timeout_sec))
            .map(move |item| match item {
                Ok(chunk) => chunk,
                Err(_) => Err(idle_timeout_error(stream_timeout_sec)),
            }),
    )
}

impl OpenAiProvider {
    /// 从配置构建；api_key 从 api_key_env 指定环境变量读取，缺失则返回错误。
    pub fn new(config: &LlmConfig) -> Result<Self, AppError> {
        let base_url = config
            .api_base
            .as_deref()
            .unwrap_or("https://api.openai.com")
            .trim_end_matches('/')
            .to_string();
        let api_key_env = config.api_key_env.as_deref().unwrap_or("OPENAI_API_KEY");
        let api_key = std::env::var(api_key_env)
            .map_err(|_| AppError::Config(format!("环境变量 {} 未设置", api_key_env)))?;

        let client = build_http_client(config, None)?;

        let semaphore = if config.max_concurrent_requests > 0 {
            Some(Semaphore::new(config.max_concurrent_requests as usize))
        } else {
            None
        };

        let api_base_fallback = config
            .api_base_fallback
            .as_deref()
            .map(|s| s.trim_end_matches('/').to_string());

        let configured_thinking_format =
            crate::core::llm::thinking_policy::ThinkingFormat::parse_or_auto(
                config.thinking.format.as_deref(),
            );
        Ok(Self {
            client,
            base_url,
            api_base_fallback,
            api_key,
            default_model: config.default_model.clone(),
            semaphore,
            retry_count: config.retry_count,
            http_timeout_sec: config.http_timeout_sec,
            stream_timeout_sec: config.stream_timeout_sec,
            non_stream_stale_timeout_sec: config.non_stream_stale_timeout_sec,
            http_read_timeout_sec: config.http_read_timeout_sec,
            thinking_cfg: config.thinking.clone(),
            configured_thinking_format,
            continuity_enabled: config.reasoning_continuity.enabled,
        })
    }

    fn effective_model(&self, request: &ChatRequest) -> String {
        request
            .model_override
            .as_deref()
            .unwrap_or(if request.model.is_empty() {
                &self.default_model
            } else {
                &request.model
            })
            .to_string()
    }

    fn thinking_format_for_model(
        &self,
        model: &str,
    ) -> crate::core::llm::thinking_policy::ThinkingFormat {
        self.configured_thinking_format.resolve_for_model(model)
    }

    fn auth_header(&self) -> (&str, String) {
        ("Authorization", format!("Bearer {}", self.api_key))
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
            Err(_) => Err(non_stream_stale_timeout_error(
                self.non_stream_stale_timeout_sec,
            )),
        }
    }

    /// 非流式请求，向给定 base_url 发起一次调用（不含重试与 fallback）。
    async fn chat_inner(
        &self,
        request: &ChatRequest,
        base_url: &str,
    ) -> Result<ChatResponse, AppError> {
        let model = self.effective_model(request);
        let thinking_format = self.thinking_format_for_model(&model);
        let thinking_fields = crate::core::llm::thinking_policy::resolve_request_fields(
            &self.thinking_cfg,
            thinking_format,
        );
        let body = OpenAiRequestBody {
            model: model.clone(),
            messages: transport_messages(&request.messages, &model, self.continuity_enabled),
            temperature: request.temperature,
            max_tokens: request.max_tokens,
            stream: false,
            tools: request.tools.clone(),
            stream_options: None,
            reasoning_effort: thinking_fields.reasoning_effort,
            thinking: thinking_fields.thinking,
        };

        let url = format!("{}/v1/chat/completions", base_url.trim_end_matches('/'));
        let (key, value) = self.auth_header();

        let resp = self
            .client
            .post(&url)
            .header(key, value)
            .json(&body)
            .send()
            .await
            .map_err(|e| map_send_error("请求", e, self.http_timeout_sec))?;

        let status = resp.status();
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| map_body_read_error("读取响应", e, self.http_read_timeout_sec))?;

        if !status.is_success() {
            let msg = String::from_utf8_lossy(&bytes);
            return Err(AppError::Llm(format!(
                "API 错误 {}: {}",
                status.as_u16(),
                msg
            )));
        }

        let response: ChatResponse =
            serde_json::from_slice(&bytes).map_err(|e| map_parse_error("解析响应", e))?;
        Ok(response)
    }

    /// 判断是否为可重试错误（429、5xx、超时等）。
    fn is_retriable(err: &AppError) -> bool {
        if let Some(stage) = llm_stage(err) {
            return matches!(
                stage,
                LlmErrorStage::Connect
                    | LlmErrorStage::Send
                    | LlmErrorStage::BodyRead
                    | LlmErrorStage::IdleTimeout
                    | LlmErrorStage::ReadTimeout
            );
        }
        let s = err.to_string();
        s.contains("429")
            || s.contains("500")
            || s.contains("502")
            || s.contains("503")
            || s.contains("请求失败")
            || s.contains("超时")
    }

    /// 判断是否为连接/网络层面错误（用于触发 api_base_fallback 降级）。
    fn is_connect_or_network_error(err: &AppError) -> bool {
        llm_connect_or_network(err)
    }

    /// 流式请求：向给定 base_url 发起一次 POST，成功时返回 Response 供消费 bytes_stream。
    async fn stream_post_once(
        &self,
        base_url: &str,
        body: &OpenAiRequestBody,
    ) -> Result<reqwest::Response, AppError> {
        let url = format!("{}/v1/chat/completions", base_url.trim_end_matches('/'));
        let (key, value) = self.auth_header();
        let resp = self
            .client
            .post(&url)
            .header(key, value)
            .json(body)
            .send()
            .await
            .map_err(|e| map_send_error("流式请求", e, self.http_timeout_sec))?;
        let status = resp.status();
        if !status.is_success() {
            let bytes = resp
                .bytes()
                .await
                .map_err(|e| map_body_read_error("读取错误响应", e, self.http_read_timeout_sec))?;
            let msg = String::from_utf8_lossy(&bytes);
            return Err(AppError::Llm(format!(
                "API 错误 {}: {}",
                status.as_u16(),
                msg
            )));
        }
        Ok(resp)
    }
}

#[async_trait]
impl LlmProvider for OpenAiProvider {
    fn provider_name(&self) -> &str {
        "openai"
    }

    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, AppError> {
        reject_multimodal_parts(&request.messages)?;

        let _permit = if let Some(ref sem) = self.semaphore {
            Some(
                sem.acquire()
                    .await
                    .map_err(|e| AppError::Llm(format!("限流信号量关闭: {}", e)))?,
            )
        } else {
            None
        };

        // 先尝试主 base；若为连接/网络错误且配置了 fallback，用 fallback 重试一次。
        let mut last_err = None;
        for attempt in 0..=self.retry_count {
            match self
                .run_non_stream_with_stale(self.chat_inner(&request, &self.base_url))
                .await
            {
                Ok(r) => return Ok(r),
                Err(e) => {
                    if Self::is_retriable(&e) && attempt < self.retry_count {
                        let delay = Duration::from_millis(500 * 2u64.pow(attempt));
                        warn!(
                            "LLM 请求失败，{}ms 后重试 ({}/{}): {}",
                            delay.as_millis(),
                            attempt + 1,
                            self.retry_count,
                            e
                        );
                        tokio::time::sleep(delay).await;
                        last_err = Some(e);
                    } else {
                        last_err = Some(e);
                        break;
                    }
                }
            }
        }
        let err = last_err.unwrap_or_else(|| AppError::Llm("重试耗尽".to_string()));
        // 自动降级：连接/网络错误且配置了 fallback 时，用 fallback base 再试一次。
        if Self::is_connect_or_network_error(&err) {
            if let Some(ref fallback) = self.api_base_fallback {
                warn!("主 API 不可达，尝试 fallback: {}", fallback);
                if let Ok(r) = self
                    .run_non_stream_with_stale(self.chat_inner(&request, fallback))
                    .await
                {
                    return Ok(r);
                }
            }
        }
        Err(err)
    }

    async fn chat_stream(
        &self,
        request: ChatRequest,
    ) -> Result<Box<dyn Stream<Item = Result<StreamEvent, AppError>> + Send + Unpin>, AppError>
    {
        reject_multimodal_parts(&request.messages)?;

        let _permit = if let Some(ref sem) = self.semaphore {
            Some(
                sem.acquire()
                    .await
                    .map_err(|e| AppError::Llm(format!("限流信号量关闭: {}", e)))?,
            )
        } else {
            None
        };

        let model = self.effective_model(&request);
        let thinking_format = self.thinking_format_for_model(&model);
        let thinking_fields = crate::core::llm::thinking_policy::resolve_request_fields(
            &self.thinking_cfg,
            thinking_format,
        );
        let body = OpenAiRequestBody {
            model: model.clone(),
            messages: transport_messages(&request.messages, &model, self.continuity_enabled),
            temperature: request.temperature,
            max_tokens: request.max_tokens,
            stream: true,
            tools: request.tools.clone(),
            stream_options: Some(StreamOptionsBody {
                include_usage: true,
            }),
            reasoning_effort: thinking_fields.reasoning_effort,
            thinking: thinking_fields.thinking,
        };

        let resp = self.stream_post_once(&self.base_url, &body).await;
        let resp = match resp {
            Ok(r) => r,
            Err(e) if Self::is_connect_or_network_error(&e) && self.api_base_fallback.is_some() => {
                warn!(
                    "流式主 API 不可达，尝试 fallback: {:?}",
                    self.api_base_fallback
                );
                self.stream_post_once(self.api_base_fallback.as_deref().unwrap(), &body)
                    .await?
            }
            Err(e) => return Err(e),
        };

        let http_read_timeout_sec = self.http_read_timeout_sec;
        let stream_timeout_sec = self.stream_timeout_sec;
        let bytes_stream = resp
            .bytes_stream()
            .map_err(move |e| map_body_read_error("流读取", e, http_read_timeout_sec));
        let bytes_stream = apply_stream_idle_timeout(bytes_stream, stream_timeout_sec);
        let event_stream = SseEventStream::new(
            bytes_stream,
            ProviderCompatProfile::chat_completions(&model),
            self.continuity_enabled,
        );
        Ok(Box::new(event_stream))
    }

    /// Trait 启发式 token 估算：`chars / 3`（保守上估，留出英文场景余量）。
    ///
    /// 多模态 `Parts` 走 [`ChatMessageContentPart::estimated_chars`]
    /// (IMAGE_CHAR_ESTIMATE = 3600 / FILE_CHAR_ESTIMATE = 8000)。
    ///
    /// **业务预算请用** [`crate::core::session::manager::types::ContextState::estimated_token_count`]：
    /// 优先 OpenAI 实际返回的 `usage.prompt_tokens`；缺失时 fallback 到 `chars / 4`，
    /// 同样把 IMAGE/FILE_CHAR_ESTIMATE 计入（与 `usage_ratio()` 口径一致）。
    /// 这里 `chars / 3` 仅给 trait 调用方做粗略上估，二者有意保留不同分母。
    fn count_tokens(&self, messages: &[ChatMessage]) -> Result<u32, AppError> {
        let total_chars: usize = messages
            .iter()
            .map(|m| match &m.content {
                Some(ChatMessageContent::Text(s)) => s.chars().count(),
                Some(ChatMessageContent::Parts(parts)) => {
                    parts.iter().map(|p| p.estimated_chars()).sum::<usize>()
                }
                None => 0,
            })
            .sum();
        Ok((total_chars / 3).max(1) as u32)
    }
}

/// 将 Bytes 流解析为 StreamEvent 流；缓冲 SSE 行，按 "data: {...}\n\n" 解析。
/// 调用方可通过 drop 提前结束流以释放连接；流式超时可由上层消费时用 tokio::time::timeout 包裹。
struct SseEventStream<S> {
    inner: S,
    buffer: Vec<u8>,
    /// 已解析待输出的事件队列（同一 chunk 可能解析出多个事件）。
    pending: std::vec::IntoIter<StreamEvent>,
    reasoning: OpenAiReasoningState,
}

#[derive(Debug)]
struct OpenAiReasoningState {
    text: String,
    had_tool_call: bool,
    source_profile: ProviderCompatProfile,
    continuity_enabled: bool,
    snapshot_emitted: bool,
    scrubber: ThinkScrubber,
}

impl Default for OpenAiReasoningState {
    fn default() -> Self {
        Self {
            text: String::new(),
            had_tool_call: false,
            source_profile: ProviderCompatProfile::chat_completions("gpt-5"),
            continuity_enabled: false,
            snapshot_emitted: false,
            scrubber: ThinkScrubber::default(),
        }
    }
}

impl OpenAiReasoningState {
    fn thinking_event(&mut self, delta: String) -> Option<StreamEvent> {
        if delta.is_empty() {
            return None;
        }
        self.text.push_str(&delta);
        Some(StreamEvent::Thinking {
            delta,
            source: ThinkingSource::Raw,
            signature: None,
        })
    }

    fn scrub_content_delta(&mut self, delta: &str) -> Vec<StreamEvent> {
        let mut events = Vec::new();
        let scrubbed = self.scrubber.push(delta);
        if let Some(hidden) = scrubbed.hidden {
            if let Some(event) = self.thinking_event(hidden) {
                events.push(event);
            }
        }
        if let Some(visible) = scrubbed.visible {
            if !visible.is_empty() {
                events.push(StreamEvent::ContentDelta { delta: visible });
            }
        }
        events
    }

    fn flush_scrubber(&mut self) -> Vec<StreamEvent> {
        let mut events = Vec::new();
        let scrubbed = self.scrubber.finish();
        if let Some(hidden) = scrubbed.hidden {
            if let Some(event) = self.thinking_event(hidden) {
                events.push(event);
            }
        }
        if let Some(visible) = scrubbed.visible {
            if !visible.is_empty() {
                events.push(StreamEvent::ContentDelta { delta: visible });
            }
        }
        events
    }

    fn maybe_snapshot(&mut self) -> Option<StreamEvent> {
        if self.snapshot_emitted
            || !self.continuity_enabled
            || !matches!(
                self.source_profile.capture_mode,
                CaptureMode::ReasoningContent
            )
        {
            return None;
        }
        let trimmed = self.text.trim();
        if trimmed.is_empty() {
            return None;
        }
        // 不按厂商名硬编码：只要 profile 标记为 chat-completions 的 ReasoningContent 即抓取
        // （capture_mode 已在上方校验）。deepseek / mimo / 未来同类共用这一条路径。
        if self.source_profile.api_family != "chat_completions" {
            return None;
        }
        self.snapshot_emitted = true;
        Some(StreamEvent::ReasoningSnapshot {
            thinking_text: Some(trimmed.to_string()),
            reasoning_continuation: Some(ReasoningContinuation {
                source_provider: self.source_profile.provider.clone(),
                source_api: self.source_profile.api_family.clone(),
                source_model: self.source_profile.model_family.clone(),
                format: ReasoningFormat::DeepseekReasoningContent,
                opaque_payload: json!({
                    "reasoning_content": trimmed,
                }),
                fallback_text: Some(trimmed.to_string()),
                provider_refs: None,
            }),
            continuity: Some(ContinuityMetadata {
                had_tool_call: self.had_tool_call,
                replay_requirement: replay_requirement_for_profile(
                    &self.source_profile,
                    self.had_tool_call,
                ),
            }),
        })
    }
}

impl<S> SseEventStream<S> {
    fn new(inner: S, source_profile: ProviderCompatProfile, continuity_enabled: bool) -> Self {
        Self {
            inner,
            buffer: Vec::new(),
            pending: Vec::new().into_iter(),
            reasoning: OpenAiReasoningState {
                source_profile,
                continuity_enabled,
                ..OpenAiReasoningState::default()
            },
        }
    }
}

impl<S> Stream for SseEventStream<S>
where
    S: Stream<Item = Result<Bytes, AppError>> + Unpin,
{
    type Item = Result<StreamEvent, AppError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        use std::task::Poll;
        let this = self.as_mut().get_mut();

        // 先输出已解析的事件
        if let Some(evt) = this.pending.next() {
            return Poll::Ready(Some(Ok(evt)));
        }

        loop {
            match Pin::new(&mut this.inner).poll_next(cx) {
                Poll::Ready(Some(Ok(bytes))) => {
                    this.buffer.extend_from_slice(&bytes);
                    match parse_sse_buffer(&mut this.buffer, &mut this.reasoning) {
                        Ok(Some(mut iter)) => {
                            if let Some(evt) = iter.next() {
                                this.pending = iter;
                                return Poll::Ready(Some(Ok(evt)));
                            }
                        }
                        Ok(None) => {}
                        Err(e) => return Poll::Ready(Some(Err(e))),
                    }
                }
                Poll::Ready(Some(Err(e))) => {
                    return Poll::Ready(Some(Err(e)));
                }
                Poll::Ready(None) => {
                    if !this.buffer.is_empty() {
                        match parse_sse_buffer(&mut this.buffer, &mut this.reasoning) {
                            Ok(Some(iter)) => {
                                let vec: Vec<_> = iter.collect();
                                if let Some((first, rest)) = vec.split_first() {
                                    #[allow(clippy::unnecessary_to_owned)]
                                    let pending_vec = rest.to_vec();
                                    this.pending = pending_vec.into_iter();
                                    return Poll::Ready(Some(Ok(first.clone())));
                                }
                            }
                            Ok(None) => {}
                            Err(e) => return Poll::Ready(Some(Err(e))),
                        }
                    }
                    let flushed = this.reasoning.flush_scrubber();
                    if let Some((first, rest)) = flushed.split_first() {
                        #[allow(clippy::unnecessary_to_owned)]
                        let pending_vec = rest.to_vec();
                        this.pending = pending_vec.into_iter();
                        return Poll::Ready(Some(Ok(first.clone())));
                    }
                    if let Some(snapshot) = this.reasoning.maybe_snapshot() {
                        return Poll::Ready(Some(Ok(snapshot)));
                    }
                    return Poll::Ready(None);
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

/// 从 buffer 中解析出完整的 SSE 块（以 \n\n 分隔），返回解析到的事件序列；已消费的数据从 buffer 移除。
fn parse_sse_buffer(
    buffer: &mut Vec<u8>,
    reasoning: &mut OpenAiReasoningState,
) -> Result<Option<std::vec::IntoIter<StreamEvent>>, AppError> {
    let sep = b"\n\n";
    let pos = buffer.windows(sep.len()).position(|w| w == sep);
    let block = match pos {
        Some(p) => {
            let end = p + sep.len();
            let block: Vec<u8> = buffer.drain(..end).collect();
            block
        }
        None => return Ok(None),
    };
    let s = std::str::from_utf8(&block).map_err(|e| map_parse_error("SSE UTF-8", e))?;
    let mut events = Vec::new();
    for line in s.lines() {
        let line = line.trim();
        if let Some(data) = line.strip_prefix("data: ") {
            if data == "[DONE]" {
                continue;
            }
            let parsed: OpenAiStreamChunk =
                serde_json::from_str(data).map_err(|e| map_parse_error("解析 SSE 行", e))?;
            events.extend(openai_chunk_to_stream_events_with_state(parsed, reasoning));
        }
    }
    Ok(Some(events.into_iter()))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case")]
struct OpenAiStreamChunk {
    choices: Option<Vec<OpenAiStreamChoice>>,
    usage: Option<TokenUsage>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case")]
struct OpenAiStreamChoice {
    delta: Option<OpenAiStreamDelta>,
    finish_reason: Option<String>,
}

/// 三路 reasoning 字段检测（报告 §3.5）：
/// - `reasoning_content`：OpenAI 主线 + DeepSeek / Doubao / Moonshot 等兼容网关；
/// - `reasoning`：部分网关 / OpenRouter 派生命名；
/// - `reasoning_text`：少量历史样本（保守保留兼容性）。
///
/// 三者只要有非空值，就向上发射 `StreamEvent::Thinking { source: Raw }`；
/// 同一帧内出现多个字段时按上述顺序优先取一项，避免重复发射。
#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case")]
struct OpenAiStreamDelta {
    content: Option<String>,
    #[allow(dead_code)]
    role: Option<String>,
    tool_calls: Option<Vec<OpenAiStreamToolCallDelta>>,
    reasoning_content: Option<String>,
    reasoning: Option<String>,
    reasoning_text: Option<String>,
}

#[derive(serde::Deserialize)]
struct OpenAiStreamToolCallDelta {
    index: Option<u32>,
    id: Option<String>,
    function: Option<OpenAiStreamFunctionDelta>,
}

#[derive(serde::Deserialize)]
struct OpenAiStreamFunctionDelta {
    name: Option<String>,
    arguments: Option<String>,
}

#[cfg(test)]
fn openai_chunk_to_stream_events(chunk: OpenAiStreamChunk) -> Vec<StreamEvent> {
    let mut reasoning = OpenAiReasoningState {
        source_profile: ProviderCompatProfile::chat_completions("gpt-5"),
        continuity_enabled: true,
        ..OpenAiReasoningState::default()
    };
    openai_chunk_to_stream_events_with_state(chunk, &mut reasoning)
}

fn openai_chunk_to_stream_events_with_state(
    chunk: OpenAiStreamChunk,
    reasoning_state: &mut OpenAiReasoningState,
) -> Vec<StreamEvent> {
    let mut events = Vec::new();

    if let Some(choices) = chunk.choices {
        if let Some(choice) = choices.into_iter().next() {
            if let Some(delta) = choice.delta {
                let reasoning_delta = delta
                    .reasoning_content
                    .or(delta.reasoning)
                    .or(delta.reasoning_text)
                    .filter(|s| !s.is_empty());
                if let Some(rc) = reasoning_delta {
                    if let Some(event) = reasoning_state.thinking_event(rc) {
                        events.push(event);
                    }
                }
                if let Some(content) = delta.content {
                    events.extend(reasoning_state.scrub_content_delta(&content));
                }
                if let Some(tool_calls) = delta.tool_calls {
                    if !tool_calls.is_empty() {
                        reasoning_state.had_tool_call = true;
                    }
                    for tc in tool_calls {
                        events.push(StreamEvent::ToolCallDelta {
                            index: tc.index.unwrap_or(0),
                            id: tc.id,
                            name: tc.function.as_ref().and_then(|f| f.name.clone()),
                            arguments_delta: tc.function.and_then(|f| f.arguments),
                        });
                    }
                }
            }
            if let Some(reason) = choice.finish_reason {
                if !reason.is_empty() {
                    events.extend(reasoning_state.flush_scrubber());
                    events.push(StreamEvent::FinishReason { reason });
                    if let Some(snapshot) = reasoning_state.maybe_snapshot() {
                        events.push(snapshot);
                    }
                }
            }
        }
    }

    if let Some(usage) = chunk.usage {
        events.push(StreamEvent::Usage {
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
            total_tokens: usage.total_tokens,
        });
    }

    events
}

// 测试统一收敛到 `tests/openai_test.rs`，再在该文件内部按 provider / stream 子模块组织；
// 业务源文件保持单一 `#[path]` 挂载，符合 RUST_FILE_LINES_SPEC §A.9 的“唯一一行”要求。
#[cfg(test)]
#[path = "tests/openai_test.rs"]
mod tests;
