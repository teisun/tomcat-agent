//! # OpenAI Responses 适配器
//!
//! 实现 [`LlmProvider`] over `POST {base}/v1/responses`：把内部 [`ChatRequest`] / [`ChatMessage`]
//! 翻译为 Responses 协议的 `input` items + 顶层 `instructions`，工具形状从 Chat Completions 的
//! `{"type": "function", "function": {...}}` 翻译为 Responses 的 `{"type": "function", "name": ...,
//! "parameters": ...}`。流式接收 SSE（默认）或 NDJSON（兼容部分网关），双路解码后输出与
//! [`OpenAiProvider`](super::openai::OpenAiProvider) 相同的 [`StreamEvent`] 序列；上层 Agent Loop
//! 完全感知不到 wire 差异。
//!
//! 设计冻结：
//! - **岔路 A**（spec §6.2）——Agent Loop 仍组一份 `ChatRequest`，wire 翻译封在本模块；
//! - HTTP 客户端、retry、proxy、fallback、信号量逻辑直接复刻 [`OpenAiProvider`] 的实现思路，
//!   保持运维行为一致；本期接受重复，待两条 Provider 都跑稳后再考虑抽 helper。
//!
//! 实现锚点参考（plan §2.1 D7）：[`pi_agent_rust/src/providers/openai_responses.rs`] 同名实现，
//! 字段 / event 命名以 OpenAI 当前官方 doc 为准。
//!
//! ## 子模块划分（L-3 拆分整改后）
//!
//! 单文件 `openai_responses.rs`（1056 行）按职责拆分到：
//!
//! - [`payload`]：`ChatRequest` ↔ `/v1/responses` 请求 / 响应翻译
//! - [`stream`]：SSE / NDJSON 双解析 + `ResponsesStream` + `ToolCallTrack`
//! - 本文件（mod.rs）：`OpenAiResponsesProvider` 定义 + HTTP 客户端 + retry / fallback +
//!   `impl LlmProvider`，每个 wire 翻译入口做一行委托。

use async_trait::async_trait;
use bytes::Bytes;
use futures_util::stream::TryStreamExt;
use serde_json::{json, Value};
use std::error::Error;
use std::pin::Pin;
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio_stream::{Stream, StreamExt};
use tracing::warn;

use crate::infra::config::LlmFilesConfig;
use crate::infra::error::AppError;
use crate::infra::LlmConfig;

use crate::core::llm::openai_files::{OpenAiFilesClient, OpenAiFilesProviderContext};
use crate::core::llm::provider::LlmProvider;
use crate::core::llm::types::{
    ChatMessage, ChatMessageContent, ChatRequest, ChatResponse, StreamEvent,
};

mod payload;
mod stream;

// 以下三个 `use` 让 `payload` / `stream` 中的私有 helper 在本 mod.rs 命名空间内可见，
// 进而通过 `#[path = "../tests/openai_responses_test.rs"] mod tests;` 的 `super::*`
// 暴露给同名单测——tests 是本模块的子模块，可以看到 mod.rs 私有的 `use` 别名。
// 不向外加 `pub`，外部仍只能通过 `OpenAiResponsesProvider` 公共 API 接触。
#[allow(unused_imports)]
use payload::{
    build_responses_input, convert_tools_to_responses, responses_payload_to_chat_response,
};
#[allow(unused_imports)]
use stream::{responses_chunk_to_events, ResponsesStream, ToolCallTrack};

/// `POST {base}/v1/responses` 适配器；与 [`OpenAiProvider`] 共享 [`LlmConfig`] 横切字段
/// （spec §6.5.2 「稳定 schema」），不为本 Provider 引入专属字段。
#[derive(Debug)]
pub struct OpenAiResponsesProvider {
    client: reqwest::Client,
    base_url: String,
    /// 主 base 不通时自动用此 URL 重试；None 表示不降级。
    api_base_fallback: Option<String>,
    api_key: String,
    default_model: String,
    /// 并发上限，None 表示不限制（仅当 max_concurrent_requests == 0）。
    semaphore: Option<Semaphore>,
    retry_count: u32,
    /// 流式空闲超时（秒）；0 表示关闭逐事件超时。
    stream_timeout_sec: u64,
    /// Files client 懒加载实例（U10）：同一 provider 生命周期只构造一次。
    files_client: std::sync::OnceLock<OpenAiFilesClient>,
    files_expires_after_seconds: u64,
    /// T2-P0-006 P5：thinking 子配置；`enabled=false` 时 build_request_body 不会写 reasoning。
    thinking_cfg: crate::infra::config::ThinkingConfig,
    thinking_format: crate::core::llm::thinking_policy::ThinkingFormat,
}

fn stream_timeout_error(stream_timeout_sec: u64) -> AppError {
    AppError::Llm(format!(
        "流式空闲超时: stream_timeout_sec={}s",
        stream_timeout_sec
    ))
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
                Err(_) => Err(stream_timeout_error(stream_timeout_sec)),
            }),
    )
}

impl OpenAiResponsesProvider {
    /// 从配置构建；与 [`OpenAiProvider::new`](super::openai::OpenAiProvider::new) 行为一致：
    /// `api_key` 从 `api_key_env` 指定环境变量读取；缺失返回 [`AppError::Config`]。
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

        let mut builder = reqwest::Client::builder().timeout(Duration::from_secs(90));
        if let Some(ref proxy_url) = config.proxy {
            let proxy = reqwest::Proxy::all(proxy_url)
                .map_err(|e| AppError::Config(format!("代理 URL 无效 {}: {}", proxy_url, e)))?;
            builder = builder.proxy(proxy);
        }
        let client = builder
            .build()
            .map_err(|e| AppError::Llm(format!("创建 HTTP 客户端失败: {}", e)))?;

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
        .resolve("openai-responses");
        Ok(Self {
            client,
            base_url,
            api_base_fallback,
            api_key,
            default_model: config.default_model.clone(),
            semaphore,
            retry_count: config.retry_count,
            stream_timeout_sec: config.stream_timeout_sec,
            files_client: std::sync::OnceLock::new(),
            files_expires_after_seconds: config.files.expires_after_seconds,
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

    fn build_request_body(&self, request: &ChatRequest, stream: bool) -> Value {
        let (instructions, input) = payload::build_responses_input(&request.messages);
        let tools_payload = request
            .tools
            .as_deref()
            .map(payload::convert_tools_to_responses)
            .filter(|v| !v.is_empty());

        let mut body = json!({
            "model": self.effective_model(request),
            "input": input,
            "stream": stream,
            "store": false,
        });
        if let Some(ins) = instructions {
            body["instructions"] = Value::String(ins);
        }
        if let Some(temp) = request.temperature {
            body["temperature"] = json!(temp);
        }
        if let Some(max) = request.max_tokens {
            // Responses 用 max_output_tokens；与 Completions 的 max_completion_tokens 概念一致。
            // 上游对 max_output_tokens 有下限（当前为 16），低于则 400。
            let max_out = max.max(16);
            body["max_output_tokens"] = json!(max_out);
        }
        if let Some(tools) = tools_payload {
            body["tools"] = Value::Array(tools);
        }
        // T2-P0-006 P5：把 ThinkingLevel/format 翻成 Responses 期望的 `reasoning.effort` 对象。
        // 与 Completions 的 `reasoning_effort` 字段不同：Responses 走 `{reasoning: {effort: "low|medium|high"}}`。
        let thinking_fields = crate::core::llm::thinking_policy::resolve_request_fields(
            &self.thinking_cfg,
            self.thinking_format,
        );
        let include_reasoning_summary =
            self.thinking_cfg.enabled && (self.thinking_cfg.show || self.thinking_cfg.persist);
        let mut reasoning = serde_json::Map::new();
        if let Some(effort) = thinking_fields.reasoning_effort {
            reasoning.insert("effort".to_string(), Value::String(effort));
        }
        if include_reasoning_summary {
            reasoning.insert("summary".to_string(), Value::String("auto".to_string()));
        }
        if !reasoning.is_empty() {
            body["reasoning"] = Value::Object(reasoning);
        }
        if let Some(thinking) = thinking_fields.thinking {
            body["thinking"] = thinking;
        }
        body
    }

    /// 非流式：向给定 base_url 发起一次 `POST /v1/responses`；不含重试与 fallback。
    async fn chat_inner(
        &self,
        request: &ChatRequest,
        base_url: &str,
    ) -> Result<ChatResponse, AppError> {
        let body = self.build_request_body(request, false);
        let url = format!("{}/v1/responses", base_url.trim_end_matches('/'));
        let (key, value) = self.auth_header();

        let resp = self
            .client
            .post(&url)
            .header(key, value)
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                let detail = e
                    .source()
                    .map(|s| format!(" 底层: {}", s))
                    .unwrap_or_default();
                AppError::Llm(format!("请求失败: {}{}", e, detail))
            })?;

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

        let raw: Value = serde_json::from_slice(&bytes)
            .map_err(|e| AppError::Llm(format!("解析响应失败: {}", e)))?;
        Ok(payload::responses_payload_to_chat_response(&raw))
    }

    fn is_retriable(err: &AppError) -> bool {
        let s = err.to_string();
        s.contains("429")
            || s.contains("500")
            || s.contains("502")
            || s.contains("503")
            || s.contains("请求失败")
            || s.contains("超时")
    }

    fn is_connect_or_network_error(err: &AppError) -> bool {
        let s = err.to_string();
        s.contains("请求失败")
            && (s.contains("Connect")
                || s.contains("connection")
                || s.contains("timed out")
                || s.contains("timeout")
                || s.contains("dns")
                || s.contains("connection refused"))
    }

    async fn stream_post_once(
        &self,
        base_url: &str,
        body: &Value,
    ) -> Result<reqwest::Response, AppError> {
        let url = format!("{}/v1/responses", base_url.trim_end_matches('/'));
        let (key, value) = self.auth_header();
        let resp = self
            .client
            .post(&url)
            .header(key, value)
            .header(
                "Accept",
                "text/event-stream, application/x-ndjson, application/ndjson",
            )
            .json(body)
            .send()
            .await
            .map_err(|e| {
                let detail = e
                    .source()
                    .map(|s| format!(" 底层: {}", s))
                    .unwrap_or_default();
                AppError::Llm(format!("流式请求失败: {}{}", e, detail))
            })?;
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
        Ok(resp)
    }
}

#[async_trait]
impl LlmProvider for OpenAiResponsesProvider {
    fn provider_name(&self) -> &str {
        "openai-responses"
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
            match self.chat_inner(&request, &self.base_url).await {
                Ok(r) => return Ok(r),
                Err(e) => {
                    if Self::is_retriable(&e) && attempt < self.retry_count {
                        let delay = Duration::from_millis(500 * 2u64.pow(attempt));
                        warn!(
                            "Responses 请求失败，{}ms 后重试 ({}/{}): {}",
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
        if Self::is_connect_or_network_error(&err) {
            if let Some(ref fallback) = self.api_base_fallback {
                warn!("主 API 不可达，尝试 fallback: {}", fallback);
                if let Ok(r) = self.chat_inner(&request, fallback).await {
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
        let _permit = if let Some(ref sem) = self.semaphore {
            Some(
                sem.acquire()
                    .await
                    .map_err(|e| AppError::Llm(format!("限流信号量关闭: {}", e)))?,
            )
        } else {
            None
        };

        let body = self.build_request_body(&request, true);
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

        // Content-Type 决定走 SSE 还是 NDJSON；优先 header，否则首帧探测兜底。
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_ascii_lowercase());
        let prefer_ndjson = content_type
            .as_deref()
            .map(|ct| ct.contains("application/x-ndjson") || ct.contains("application/ndjson"))
            .unwrap_or(false);

        let bytes_stream = resp
            .bytes_stream()
            .map_err(|e| AppError::Llm(format!("流读取错误: {}", e)));
        let bytes_stream = apply_stream_idle_timeout(bytes_stream, self.stream_timeout_sec);
        let event_stream = stream::ResponsesStream::new(bytes_stream, prefer_ndjson);
        Ok(Box::new(event_stream))
    }

    /// Trait 启发式 token 估算：`chars / 3`（与 Completions 同口径，便于上层统一近似预算）。
    ///
    /// 多模态 `Parts` 走 [`ChatMessageContentPart::estimated_chars`]
    /// (IMAGE_CHAR_ESTIMATE = 3600 / FILE_CHAR_ESTIMATE = 8000)。
    ///
    /// **业务预算请用** [`crate::core::session::manager::types::ContextState::estimated_token_count`]：
    /// 优先 OpenAI Responses stream 回填的真实 `usage.input_tokens`（写入 `last_api_usage`）；
    /// 缺失时 fallback 到 `chars / 4`，同样把 IMAGE/FILE_CHAR_ESTIMATE 计入
    /// （与 `usage_ratio()` 口径一致）。这里 `chars / 3` 仅给 trait 调用方做粗略上估，
    /// 二者有意保留不同分母。
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

    fn supports_openai_files_api(&self) -> bool {
        true
    }

    fn openai_files_context(&self) -> Option<OpenAiFilesProviderContext> {
        Some(OpenAiFilesProviderContext {
            client: self.client.clone(),
            base_url: self.base_url.clone(),
            api_key: self.api_key.clone(),
            retry_count: self.retry_count,
        })
    }

    fn openai_files_client(&self, files_cfg: &LlmFilesConfig) -> Option<OpenAiFilesClient> {
        if !self.supports_openai_files_api() {
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
        let client = self.files_client.get_or_init(|| {
            OpenAiFilesClient::from_provider_context(
                OpenAiFilesProviderContext {
                    client: self.client.clone(),
                    base_url: self.base_url.clone(),
                    api_key: self.api_key.clone(),
                    retry_count: self.retry_count,
                },
                &cfg,
            )
        });
        Some(client.clone())
    }
}

// 单测见 `core::llm::openai_responses::tests::openai_responses_test`（wire + tools +
// payload + count_tokens + 流式解析覆盖在同父目录测试目录中按 plan §5 Phase E.2 /
// E.3 拆分）。
#[cfg(test)]
#[path = "tests/openai_responses_test.rs"]
mod tests;
