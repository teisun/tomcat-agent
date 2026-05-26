//! # OpenAI 格式适配器
//!
//! 实现 LlmProvider：非流式/流式调用、限流、指数退避重试、流式超时与资源释放。

use async_trait::async_trait;
use bytes::Bytes;
use futures_util::stream::TryStreamExt;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio_stream::{Stream, StreamExt};
use tracing::warn;

use crate::core::llm::http_client::build_http_client;
use crate::infra::error::AppError;
use crate::infra::error::{
    llm_connect_or_network, llm_error, llm_error_with_source, llm_stage, LlmErrorStage,
};
use crate::infra::LlmConfig;

use crate::core::llm::provider::LlmProvider;
use crate::core::llm::types::{
    ChatMessage, ChatMessageContent, ChatRequest, ChatResponse, StreamEvent, ThinkingSource,
    TokenUsage,
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

/// 发给 OpenAI API 的请求体（不含 model_override，stream 由调用方定）。
/// 使用 max_completion_tokens 以兼容新模型（部分模型已不再接受 max_tokens）。
///
/// `reasoning_effort` / `thinking` 是 T2-P0-006 P2a 阶段引入的可选字段：
/// - `reasoning_effort`：OpenAI 系（gpt-5.x 等）走 `low/medium/high/...` 字符串档位；
/// - `thinking`：豆包 / Moonshot / 部分网关使用 `{"type": "enabled", ...}` 对象；
/// - 两者**互斥**，由 P5 阶段的 `thinking_policy` 映射决定使用哪一个；
/// - 本期均默认 `None`，与 LlmConfig 默认行为一致（不改变现网请求体）。
///
/// 详见 `docs/architecture/llm-stream-events-cli-pipeline.md` §4.2.1 / §4.2.2。
#[derive(serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct OpenAiRequestBody {
    model: String,
    messages: Vec<ChatMessage>,
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
    /// `provider id`，给 ThinkingFormat::Auto 推断使用；`OpenAiProvider` 固定为 `"openai"`。
    thinking_format: crate::core::llm::thinking_policy::ThinkingFormat,
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

        let thinking_format = crate::core::llm::thinking_policy::ThinkingFormat::parse_or_auto(
            config.thinking.format.as_deref(),
        )
        .resolve("openai");
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
            thinking_format,
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
        let thinking_fields = crate::core::llm::thinking_policy::resolve_request_fields(
            &self.thinking_cfg,
            self.thinking_format,
        );
        let body = OpenAiRequestBody {
            model: self.effective_model(request),
            messages: request.messages.clone(),
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

        let thinking_fields = crate::core::llm::thinking_policy::resolve_request_fields(
            &self.thinking_cfg,
            self.thinking_format,
        );
        let body = OpenAiRequestBody {
            model: self.effective_model(&request),
            messages: request.messages.clone(),
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
        let event_stream = SseEventStream::new(bytes_stream);
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
}

impl<S> SseEventStream<S> {
    fn new(inner: S) -> Self {
        Self {
            inner,
            buffer: Vec::new(),
            pending: Vec::new().into_iter(),
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

        // 先输出已解析的事件
        if let Some(evt) = self.pending.next() {
            return Poll::Ready(Some(Ok(evt)));
        }

        loop {
            match Pin::new(&mut self.inner).poll_next(cx) {
                Poll::Ready(Some(Ok(bytes))) => {
                    self.buffer.extend_from_slice(&bytes);
                    match parse_sse_buffer(&mut self.buffer) {
                        Ok(Some(mut iter)) => {
                            if let Some(evt) = iter.next() {
                                self.pending = iter;
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
                    if !self.buffer.is_empty() {
                        match parse_sse_buffer(&mut self.buffer) {
                            Ok(Some(iter)) => {
                                let vec: Vec<_> = iter.collect();
                                if let Some((first, rest)) = vec.split_first() {
                                    #[allow(clippy::unnecessary_to_owned)]
                                    let pending_vec = rest.to_vec();
                                    self.pending = pending_vec.into_iter();
                                    return Poll::Ready(Some(Ok(first.clone())));
                                }
                            }
                            Ok(None) => {}
                            Err(e) => return Poll::Ready(Some(Err(e))),
                        }
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
            events.extend(openai_chunk_to_stream_events(parsed));
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

fn openai_chunk_to_stream_events(chunk: OpenAiStreamChunk) -> Vec<StreamEvent> {
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
                    events.push(StreamEvent::Thinking {
                        delta: rc,
                        source: ThinkingSource::Raw,
                        signature: None,
                    });
                }
                if let Some(content) = delta.content {
                    if !content.is_empty() {
                        events.push(StreamEvent::ContentDelta { delta: content });
                    }
                }
                if let Some(tool_calls) = delta.tool_calls {
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
                    events.push(StreamEvent::FinishReason { reason });
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
