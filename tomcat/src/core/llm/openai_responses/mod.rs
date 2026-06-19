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
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio_stream::{Stream, StreamExt};
use tracing::warn;

use super::super::auth::Credential;
use super::super::catalog::{infer_default_base_url, ModelEntry};
use crate::core::llm::http_client::build_http_client;
use crate::infra::config::{LlmFilesConfig, LlmRuntimeConfig};
use crate::infra::error::{
    is_retryable_llm_error, llm_connect_or_network, llm_error, llm_error_with_source,
    llm_http_status_error, llm_http_status_error_with_stage, AppError, LlmErrorStage,
};

use super::super::retry_delay::provider_retry_delay;
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
#[cfg(test)]
#[allow(unused_imports)]
use stream::responses_chunk_to_events;
#[allow(unused_imports)]
use stream::{
    responses_chunk_to_events_with_state, ReasoningState, ResponsesStream, ToolCallTrack,
};
const PROVIDER_NAME: &str = "openai-responses";

fn idle_timeout_error(stream_timeout_sec: u64) -> AppError {
    llm_error(
        PROVIDER_NAME,
        LlmErrorStage::IdleTimeout,
        format!("流式空闲超时: stream_timeout_sec={}s", stream_timeout_sec),
    )
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

fn map_send_error(prefix: &str, err: reqwest::Error, http_read_timeout_sec: u64) -> AppError {
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
            LlmErrorStage::ReadTimeout,
            format!(
                "{prefix}读/空闲超时（等待响应头）: http_read_timeout_sec={}s",
                http_read_timeout_sec
            ),
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

fn gateway_http_stage(status: u16, body: &str) -> Option<LlmErrorStage> {
    if !matches!(status, 502..=504) {
        return None;
    }
    let lower = body.to_lowercase();
    if lower.contains("upstream connect")
        || lower.contains("disconnect/reset before headers")
        || lower.contains("connection timeout")
        || lower.contains("connection refused")
        || lower.contains("timed out")
        || lower.contains("reset reason")
        || lower.contains("dns")
    {
        return Some(LlmErrorStage::Connect);
    }
    None
}

fn map_http_status_error(status: reqwest::StatusCode, body: &[u8]) -> AppError {
    let message = String::from_utf8_lossy(body).into_owned();
    if let Some(stage) = gateway_http_stage(status.as_u16(), &message) {
        return llm_http_status_error_with_stage(PROVIDER_NAME, stage, status.as_u16(), message);
    }
    llm_http_status_error(PROVIDER_NAME, status.as_u16(), message)
}

/// `POST {base}/v1/responses` 适配器；与 [`OpenAiProvider`] 共享 [`LlmRuntimeConfig`] 横切字段，
/// 但模型连接信息来自 [`ModelEntry`] 与 [`Credential`]。
#[derive(Debug)]
pub struct OpenAiResponsesProvider {
    client: reqwest::Client,
    base_url: String,
    /// 主 base 不通时自动用此 URL 重试；None 表示不降级。
    api_base_fallback: Option<String>,
    api_key: String,
    /// Catalog [`ModelEntry::id`]；出站时若 request 仍带此 id，映射为 wire 名 [`default_model`]。
    catalog_model_id: String,
    default_model: String,
    /// 并发上限，None 表示不限制（仅当 max_concurrent_requests == 0）。
    semaphore: Option<Semaphore>,
    retry_count: u32,
    /// 流式空闲超时（秒）；0 表示关闭逐事件超时。
    stream_timeout_sec: u64,
    non_stream_stale_timeout_sec: u64,
    http_read_timeout_sec: u64,
    /// Files client 懒加载实例（U10）：同一 provider 生命周期只构造一次。
    files_client: std::sync::OnceLock<OpenAiFilesClient>,
    files_expires_after_seconds: u64,
    /// T2-P0-006 P5：thinking 子配置；`enabled=false` 时 build_request_body 不会写 reasoning。
    thinking_cfg: crate::infra::config::ThinkingConfig,
    /// 用户显式配置的 thinking format；`Auto` 时按请求实际 model 决定。
    configured_thinking_format: crate::core::llm::thinking_policy::ThinkingFormat,
    continuity_enabled: bool,
    use_previous_response_id: bool,
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

fn latest_openai_response_id(messages: &[ChatMessage]) -> Option<String> {
    messages.iter().rev().find_map(|msg| {
        msg.reasoning_continuation
            .as_ref()
            .and_then(|continuation| continuation.provider_refs.as_ref())
            .and_then(|refs| refs.openai_response_id.clone())
            .filter(|id| !id.is_empty())
    })
}

fn request_uses_previous_response_id(body: &Value) -> bool {
    body.get("previous_response_id")
        .and_then(Value::as_str)
        .is_some()
}

fn is_previous_response_id_error(err: &AppError) -> bool {
    let text = err.to_string().to_ascii_lowercase();
    text.contains("previous_response_id")
        || text.contains("previous response id")
        || text.contains("previous_response")
}

impl OpenAiResponsesProvider {
    /// 从模型条目 + 全局运行时配置构建。
    pub fn new(
        entry: &ModelEntry,
        runtime: &LlmRuntimeConfig,
        credential: &Credential,
    ) -> Result<Self, AppError> {
        let base_url = entry
            .base_url
            .clone()
            .or_else(|| infer_default_base_url(Some(entry.provider.as_str())))
            .or_else(|| infer_default_base_url(Some(entry.api.as_str())))
            .unwrap_or_else(|| "https://api.openai.com".to_string())
            .trim_end_matches('/')
            .to_string();

        let client = build_http_client(runtime, None)?;

        let semaphore = if runtime.max_concurrent_requests > 0 {
            Some(Semaphore::new(runtime.max_concurrent_requests as usize))
        } else {
            None
        };

        let api_base_fallback = runtime
            .api_base_fallback
            .as_deref()
            .map(|s| s.trim_end_matches('/').to_string());

        let configured_thinking_format =
            crate::core::llm::thinking_policy::ThinkingFormat::parse_or_auto(
                entry
                    .thinking_format
                    .as_deref()
                    .or(runtime.thinking.format.as_deref()),
            );
        Ok(Self {
            client,
            base_url,
            api_base_fallback,
            api_key: credential.value.clone(),
            catalog_model_id: entry.id.clone(),
            default_model: entry.request_model_name().to_string(),
            semaphore,
            retry_count: runtime.retry_count,
            stream_timeout_sec: runtime.stream_timeout_sec,
            non_stream_stale_timeout_sec: runtime.non_stream_stale_timeout_sec,
            http_read_timeout_sec: runtime.http_read_timeout_sec,
            files_client: std::sync::OnceLock::new(),
            files_expires_after_seconds: runtime.files.expires_after_seconds,
            thinking_cfg: runtime.thinking.clone(),
            configured_thinking_format,
            continuity_enabled: runtime.reasoning_continuity.enabled,
            use_previous_response_id: runtime.openai_responses.use_previous_response_id,
        })
    }

    fn effective_model(&self, request: &ChatRequest) -> String {
        if let Some(m) = request.model_override.as_deref().filter(|s| !s.is_empty()) {
            return if m == self.catalog_model_id {
                self.default_model.clone()
            } else {
                m.to_string()
            };
        }
        let req_model = request.model.trim();
        if req_model.is_empty() || req_model == self.catalog_model_id {
            self.default_model.clone()
        } else {
            req_model.to_string()
        }
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

    fn build_request_body_with_hint(
        &self,
        request: &ChatRequest,
        stream: bool,
        allow_response_id_hint: bool,
    ) -> Value {
        let model = self.effective_model(request);
        let target_profile =
            crate::core::llm::replay_policy::ProviderCompatProfile::openai_responses(&model);
        let thinking_format = self.thinking_format_for_model(&model);
        let previous_response_id = if self.continuity_enabled
            && self.use_previous_response_id
            && allow_response_id_hint
            && target_profile.supports_response_id_hint
        {
            latest_openai_response_id(&request.messages)
        } else {
            None
        };
        let explicit_replay = self.continuity_enabled && previous_response_id.is_none();
        let (instructions, input) = payload::build_responses_input(
            &request.messages,
            &target_profile,
            self.continuity_enabled,
            explicit_replay,
        );
        let tools_payload = request
            .tools
            .as_deref()
            .map(payload::convert_tools_to_responses)
            .filter(|v| !v.is_empty());

        let mut body = json!({
            "model": model,
            "input": input,
            "stream": stream,
            "store": false,
        });
        if explicit_replay {
            body["include"] = json!(["reasoning.encrypted_content"]);
        }
        if let Some(previous_response_id) = previous_response_id {
            body["store"] = json!(true);
            body["previous_response_id"] = Value::String(previous_response_id);
        }
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
            thinking_format,
        );
        let include_reasoning_summary = self.thinking_cfg.enabled;
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

    fn build_request_body(&self, request: &ChatRequest, stream: bool) -> Value {
        self.build_request_body_with_hint(request, stream, true)
    }

    /// 非流式：向给定 base_url 发起一次 `POST /v1/responses`；不含重试与 fallback。
    async fn chat_inner_with_body(
        &self,
        body: &Value,
        base_url: &str,
    ) -> Result<ChatResponse, AppError> {
        let url = format!("{}/v1/responses", base_url.trim_end_matches('/'));
        let (key, value) = self.auth_header();

        let resp = self
            .client
            .post(&url)
            .header(key, value)
            .json(&body)
            .send()
            .await
            .map_err(|e| map_send_error("请求", e, self.http_read_timeout_sec))?;

        let status = resp.status();
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| map_body_read_error("读取响应", e, self.http_read_timeout_sec))?;

        if !status.is_success() {
            return Err(map_http_status_error(status, &bytes));
        }

        let raw: Value =
            serde_json::from_slice(&bytes).map_err(|e| map_parse_error("解析响应", e))?;
        Ok(payload::responses_payload_to_chat_response(&raw))
    }

    async fn chat_with_retry(
        &self,
        body: &Value,
        base_url: &str,
    ) -> Result<ChatResponse, AppError> {
        let mut last_err = None;
        for attempt in 0..=self.retry_count {
            match self
                .run_non_stream_with_stale(self.chat_inner_with_body(body, base_url))
                .await
            {
                Ok(r) => return Ok(r),
                Err(e) => {
                    if Self::is_retriable(&e) && attempt < self.retry_count {
                        let delay = provider_retry_delay(attempt);
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
                if let Ok(r) = self
                    .run_non_stream_with_stale(self.chat_inner_with_body(body, fallback))
                    .await
                {
                    return Ok(r);
                }
            }
        }
        Err(err)
    }

    fn is_retriable(err: &AppError) -> bool {
        is_retryable_llm_error(err)
    }

    fn is_connect_or_network_error(err: &AppError) -> bool {
        llm_connect_or_network(err)
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
            .map_err(|e| map_send_error("流式请求", e, self.http_read_timeout_sec))?;
        let status = resp.status();
        if !status.is_success() {
            let bytes = resp
                .bytes()
                .await
                .map_err(|e| map_body_read_error("读取错误响应", e, self.http_read_timeout_sec))?;
            return Err(map_http_status_error(status, &bytes));
        }
        Ok(resp)
    }

    async fn stream_post_with_retry(
        &self,
        base_url: &str,
        body: &Value,
    ) -> Result<reqwest::Response, AppError> {
        let mut last_err = None;
        for attempt in 0..=self.retry_count {
            match self.stream_post_once(base_url, body).await {
                Ok(resp) => return Ok(resp),
                Err(err) if Self::is_retriable(&err) && attempt < self.retry_count => {
                    let delay = provider_retry_delay(attempt);
                    warn!(
                        "Responses 流式建连失败，{}ms 后重试 ({}/{}): {}",
                        delay.as_millis(),
                        attempt + 1,
                        self.retry_count,
                        err
                    );
                    tokio::time::sleep(delay).await;
                    last_err = Some(err);
                }
                Err(err) => return Err(err),
            }
        }
        Err(last_err.unwrap_or_else(|| AppError::Llm("Responses 流式建连重试耗尽".to_string())))
    }

    async fn stream_post_with_base_fallback(
        &self,
        body: &Value,
    ) -> Result<reqwest::Response, AppError> {
        match self.stream_post_with_retry(&self.base_url, body).await {
            Ok(resp) => Ok(resp),
            Err(err)
                if Self::is_connect_or_network_error(&err) && self.api_base_fallback.is_some() =>
            {
                warn!(
                    "流式主 API 不可达，尝试 fallback: {:?}",
                    self.api_base_fallback
                );
                self.stream_post_with_retry(self.api_base_fallback.as_deref().unwrap(), body)
                    .await
            }
            Err(err) => Err(err),
        }
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

        let body = self.build_request_body(&request, false);
        match self.chat_with_retry(&body, &self.base_url).await {
            Ok(r) => Ok(r),
            Err(err)
                if request_uses_previous_response_id(&body)
                    && is_previous_response_id_error(&err) =>
            {
                warn!(
                    error = %err,
                    "previous_response_id 被上游拒绝，退回 store=false + 显式 replay 重试一次"
                );
                let fallback_body = self.build_request_body_with_hint(&request, false, false);
                self.chat_with_retry(&fallback_body, &self.base_url).await
            }
            Err(err) => Err(err),
        }
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

        let model = self.effective_model(&request);
        let source_profile =
            crate::core::llm::replay_policy::ProviderCompatProfile::openai_responses(&model);
        let body = self.build_request_body(&request, true);
        let resp = match self.stream_post_with_base_fallback(&body).await {
            Ok(r) => r,
            Err(err)
                if request_uses_previous_response_id(&body)
                    && is_previous_response_id_error(&err) =>
            {
                warn!(
                    error = %err,
                    "previous_response_id 流式请求被上游拒绝，退回 store=false + 显式 replay 重试一次"
                );
                let fallback_body = self.build_request_body_with_hint(&request, true, false);
                self.stream_post_with_base_fallback(&fallback_body).await?
            }
            Err(err) => return Err(err),
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

        let http_read_timeout_sec = self.http_read_timeout_sec;
        let stream_timeout_sec = self.stream_timeout_sec;
        let bytes_stream = resp
            .bytes_stream()
            .map_err(move |e| map_body_read_error("流读取", e, http_read_timeout_sec));
        let bytes_stream = apply_stream_idle_timeout(bytes_stream, stream_timeout_sec);
        let event_stream = stream::ResponsesStream::new(
            bytes_stream,
            prefer_ndjson,
            source_profile,
            self.continuity_enabled,
        );
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
