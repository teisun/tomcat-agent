//! # OpenAI 格式适配器
//!
//! 实现 LlmProvider：非流式/流式调用、限流、指数退避重试、流式超时与资源释放。

use async_trait::async_trait;
use bytes::Bytes;
use futures_util::stream::TryStreamExt;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio_stream::Stream;
use tracing::warn;

use crate::infra::error::AppError;
use crate::infra::LlmConfig;

use super::provider::LlmProvider;
use super::types::{ChatMessage, ChatMessageContent, ChatRequest, ChatResponse, StreamEvent};

/// 发给 OpenAI API 的请求体（不含 model_override，stream 由调用方定）。
#[derive(serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct OpenAiRequestBody {
    model: String,
    messages: Vec<ChatMessage>,
    temperature: Option<f32>,
    max_tokens: Option<u32>,
    stream: bool,
}

/// OpenAI 兼容 API 的适配器；限流、重试、超时由本实现负责。
#[derive(Debug)]
pub struct OpenAiProvider {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    #[allow(dead_code)]
    default_model: String,
    /// 并发上限，None 表示不限制（仅当 max_concurrent_requests == 0）。
    semaphore: Option<Semaphore>,
    retry_count: u32,
    #[allow(dead_code)]
    stream_timeout_sec: u64,
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

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(90))
            .build()
            .map_err(|e| AppError::Llm(format!("创建 HTTP 客户端失败: {}", e)))?;

        let semaphore = if config.max_concurrent_requests > 0 {
            Some(Semaphore::new(config.max_concurrent_requests as usize))
        } else {
            None
        };

        Ok(Self {
            client,
            base_url,
            api_key,
            default_model: config.default_model.clone(),
            semaphore,
            retry_count: config.retry_count,
            stream_timeout_sec: config.stream_timeout_sec,
        })
    }

    fn effective_model(&self, request: &ChatRequest) -> String {
        request
            .model_override
            .as_deref()
            .unwrap_or(&request.model)
            .to_string()
    }

    fn auth_header(&self) -> (&str, String) {
        ("Authorization", format!("Bearer {}", self.api_key))
    }

    /// 非流式请求，带重试（仅对可重试错误）。
    async fn chat_inner(&self, request: &ChatRequest) -> Result<ChatResponse, AppError> {
        let body = OpenAiRequestBody {
            model: self.effective_model(request),
            messages: request.messages.clone(),
            temperature: request.temperature,
            max_tokens: request.max_tokens,
            stream: false,
        };

        let url = format!("{}/v1/chat/completions", self.base_url);
        let (key, value) = self.auth_header();

        let resp = self
            .client
            .post(&url)
            .header(key, value)
            .json(&body)
            .send()
            .await
            .map_err(|e| AppError::Llm(format!("请求失败: {}", e)))?;

        let status = resp.status();
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| AppError::Llm(format!("读取响应失败: {}", e)))?;

        if !status.is_success() {
            let msg = String::from_utf8_lossy(&bytes);
            return Err(AppError::Llm(format!(
                "API 错误 {}: {}",
                status.as_u16(),
                msg
            )));
        }

        let response: ChatResponse = serde_json::from_slice(&bytes)
            .map_err(|e| AppError::Llm(format!("解析响应失败: {}", e)))?;
        Ok(response)
    }

    /// 判断是否为可重试错误（429、5xx、超时等）。
    pub(crate) fn is_retriable(err: &AppError) -> bool {
        let s = err.to_string();
        s.contains("429")
            || s.contains("500")
            || s.contains("502")
            || s.contains("503")
            || s.contains("请求失败")
            || s.contains("超时")
    }
}

#[async_trait]
impl LlmProvider for OpenAiProvider {
    fn provider_name(&self) -> &str {
        "openai"
    }

    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, AppError> {
        let _permit = if let Some(ref sem) = self.semaphore {
            Some(
                sem.acquire()
                    .await
                    .map_err(|e| AppError::Llm(format!("限流信号量关闭: {}", e)))?,
            )
        } else {
            None
        };

        let mut last_err = None;
        for attempt in 0..=self.retry_count {
            match self.chat_inner(&request).await {
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
                        return Err(e);
                    }
                }
            }
        }
        Err(last_err.unwrap_or_else(|| AppError::Llm("重试耗尽".to_string())))
    }

    async fn chat_stream(
        &self,
        request: ChatRequest,
    ) -> Result<Box<dyn Stream<Item = Result<StreamEvent, AppError>> + Send + Unpin>, AppError>
    {
        let _permit = if let Some(ref sem) = self.semaphore {
            Some(
                sem.acquire()
                    .await
                    .map_err(|e| AppError::Llm(format!("限流信号量关闭: {}", e)))?,
            )
        } else {
            None
        };

        let body = OpenAiRequestBody {
            model: self.effective_model(&request),
            messages: request.messages.clone(),
            temperature: request.temperature,
            max_tokens: request.max_tokens,
            stream: true,
        };

        let url = format!("{}/v1/chat/completions", self.base_url);
        let (key, value) = self.auth_header();

        let resp = self
            .client
            .post(&url)
            .header(key, value)
            .json(&body)
            .send()
            .await
            .map_err(|e| AppError::Llm(format!("流式请求失败: {}", e)))?;

        let status = resp.status();
        if !status.is_success() {
            let bytes = resp
                .bytes()
                .await
                .map_err(|e| AppError::Llm(format!("读取错误响应: {}", e)))?;
            let msg = String::from_utf8_lossy(&bytes);
            return Err(AppError::Llm(format!(
                "API 错误 {}: {}",
                status.as_u16(),
                msg
            )));
        }

        let bytes_stream = resp
            .bytes_stream()
            .map_err(|e| AppError::Llm(format!("流读取错误: {}", e)));
        let event_stream = SseEventStream::new(bytes_stream);
        Ok(Box::new(event_stream))
    }

    fn count_tokens(&self, messages: &[ChatMessage]) -> Result<u32, AppError> {
        // 近似：英文约 4 字符/token，中文约 1.5 字符/token，取 3 字符/token 估算。
        let total_chars: usize = messages
            .iter()
            .map(|m| match &m.content {
                ChatMessageContent::Text(s) => s.chars().count(),
                ChatMessageContent::Parts(parts) => parts
                    .iter()
                    .map(|p| p.text.as_deref().unwrap_or("").chars().count())
                    .sum::<usize>(),
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

impl<S, E> Stream for SseEventStream<S>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
    E: std::fmt::Display,
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
                    return Poll::Ready(Some(Err(AppError::Llm(e.to_string()))));
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
    let s = std::str::from_utf8(&block).map_err(|e| AppError::Llm(format!("UTF-8 错误: {}", e)))?;
    let mut events = Vec::new();
    for line in s.lines() {
        let line = line.trim();
        if let Some(data) = line.strip_prefix("data: ") {
            if data == "[DONE]" {
                continue;
            }
            let parsed: OpenAiStreamChunk = serde_json::from_str(data)
                .map_err(|e| AppError::Llm(format!("解析 SSE 行失败: {}", e)))?;
            if let Some(evt) = openai_chunk_to_stream_event(parsed) {
                events.push(evt);
            }
        }
    }
    Ok(Some(events.into_iter()))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case")]
struct OpenAiStreamChunk {
    choices: Option<Vec<OpenAiStreamChoice>>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case")]
struct OpenAiStreamChoice {
    delta: Option<OpenAiStreamDelta>,
    finish_reason: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case")]
struct OpenAiStreamDelta {
    content: Option<String>,
    #[allow(dead_code)]
    role: Option<String>,
}

fn openai_chunk_to_stream_event(chunk: OpenAiStreamChunk) -> Option<StreamEvent> {
    let choices = chunk.choices?.into_iter().next()?;
    if let Some(reason) = choices.finish_reason {
        if !reason.is_empty() {
            return Some(StreamEvent::FinishReason { reason });
        }
    }
    if let Some(delta) = choices.delta {
        if let Some(content) = delta.content {
            if !content.is_empty() {
                return Some(StreamEvent::ContentDelta { delta: content });
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::LlmConfig;

    #[test]
    fn openai_provider_new_fails_without_api_key() {
        let config = LlmConfig {
            api_key_env: Some("PI_AWSM_TEST_NONEXISTENT_ENV_VAR_12345".to_string()),
            ..LlmConfig::default()
        };
        let r = OpenAiProvider::new(&config);
        assert!(r.is_err());
        let err = r.unwrap_err();
        assert!(err.to_string().contains("未设置"));
    }

    #[test]
    fn count_tokens_approximate() {
        let config = LlmConfig {
            api_key_env: Some("OPENAI_API_KEY".to_string()),
            ..LlmConfig::default()
        };
        if OpenAiProvider::new(&config).is_err() {
            return;
        }
        let provider = OpenAiProvider::new(&config).unwrap();
        let messages = vec![
            ChatMessage::user("hello world"),
            ChatMessage::assistant("hi there"),
        ];
        let n = provider.count_tokens(&messages).unwrap();
        assert!(n >= 1);
        assert!(n <= 20);
    }

    #[test]
    fn is_retriable_detects_429_and_5xx() {
        assert!(OpenAiProvider::is_retriable(&AppError::Llm(
            "API 错误 429: rate limit".to_string()
        )));
        assert!(OpenAiProvider::is_retriable(&AppError::Llm(
            "API 错误 502: bad gateway".to_string()
        )));
        assert!(!OpenAiProvider::is_retriable(&AppError::Llm(
            "API 错误 400: bad request".to_string()
        )));
    }

    #[tokio::test]
    async fn sse_stream_parses_and_yields_events() {
        use super::*;
        use tokio_stream::StreamExt;
        let chunks: Vec<Result<Bytes, AppError>> = vec![
            Ok(Bytes::from(
                "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n",
            )),
            Ok(Bytes::from(
                "data: {\"choices\":[{\"delta\":{\"content\":\" world\"}}]}\n\n",
            )),
            Ok(Bytes::from(
                "data: {\"choices\":[{\"finish_reason\":\"stop\"}]}\n\n",
            )),
        ];
        let stream = tokio_stream::iter(chunks);
        let mut event_stream = SseEventStream::new(stream);
        let mut events = Vec::new();
        while let Some(item) = event_stream.next().await {
            events.push(item);
        }
        assert_eq!(events.len(), 3);
        assert!(matches!(&events[0], Ok(StreamEvent::ContentDelta { delta } ) if delta == "Hello"));
        assert!(
            matches!(&events[1], Ok(StreamEvent::ContentDelta { delta } ) if delta == " world")
        );
        assert!(
            matches!(&events[2], Ok(StreamEvent::FinishReason { reason } ) if reason == "stop")
        );
    }
}
