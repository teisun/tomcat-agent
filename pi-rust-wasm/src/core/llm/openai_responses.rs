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

use async_trait::async_trait;
use bytes::Bytes;
use futures_util::stream::TryStreamExt;
use serde_json::{json, Value};
use std::error::Error;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio_stream::Stream;
use tracing::warn;

use crate::infra::error::AppError;
use crate::infra::LlmConfig;

use crate::core::llm::provider::LlmProvider;
use crate::core::llm::types::{
    ChatMessage, ChatMessageContent, ChatMessageRole, ChatRequest, ChatResponse,
    ChatResponseChoice, StreamEvent,
};

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
    /// TODO（与 Completions 对齐）：接 `tokio::time::timeout` 实现流式心跳超时
    #[allow(dead_code)]
    stream_timeout_sec: u64,
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

        Ok(Self {
            client,
            base_url,
            api_base_fallback,
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
        let (instructions, input) = build_responses_input(&request.messages);
        let tools_payload = request
            .tools
            .as_deref()
            .map(convert_tools_to_responses)
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
        Ok(responses_payload_to_chat_response(&raw))
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
        let event_stream = ResponsesStream::new(bytes_stream, prefer_ndjson);
        Ok(Box::new(event_stream))
    }

    fn count_tokens(&self, messages: &[ChatMessage]) -> Result<u32, AppError> {
        // 启发式：与 Completions 同口径（chars / 3），便于上下文预算共用统一估算。
        let total_chars: usize = messages
            .iter()
            .map(|m| match &m.content {
                Some(ChatMessageContent::Text(s)) => s.chars().count(),
                Some(ChatMessageContent::Parts(parts)) => parts
                    .iter()
                    .map(|p| p.text.as_deref().unwrap_or("").chars().count())
                    .sum::<usize>(),
                None => 0,
            })
            .sum();
        Ok((total_chars / 3).max(1) as u32)
    }
}

// ============================================================================
// Wire 翻译：ChatMessage → Responses input items / instructions
// ============================================================================

/// 把内部 [`ChatMessage`] 序列翻译为 Responses 的 `(instructions, input items)`。
///
/// 规则（与 plan §5 Phase B 表 + pi_agent_rust 同名实现一致）：
/// - 序列首条 `role=System` 文本 → 顶层 `instructions`，**不**进 input；
/// - 后续 `role=System` → 退化到 `input` 中的 `message` 项（Responses 通常允许，但少数 Codex
///   端点会拒绝；本期不做特殊处理）；
/// - `User` → `{ type: "message", role: "user", content: [input_text] }`；
/// - `Assistant` 纯文本 → `{ type: "message", role: "assistant", content: [output_text] }`；
/// - `Assistant` 带 `tool_calls` → 文本部分单独发一条 message item，每个 tool_call 翻成
///   `{ type: "function_call", call_id, name, arguments }`；
/// - `Tool` → `{ type: "function_call_output", call_id: tool_call_id, output: text }`。
fn build_responses_input(messages: &[ChatMessage]) -> (Option<String>, Vec<Value>) {
    let mut instructions: Option<String> = None;
    let mut input: Vec<Value> = Vec::with_capacity(messages.len());
    let mut first_seen = false;

    for msg in messages {
        match msg.role {
            ChatMessageRole::System => {
                let text = extract_text(&msg.content).unwrap_or_default();
                if !first_seen && instructions.is_none() {
                    instructions = Some(text);
                    first_seen = true;
                    continue;
                }
                first_seen = true;
                input.push(json!({
                    "type": "message",
                    "role": "system",
                    "content": [{"type": "input_text", "text": text}],
                }));
            }
            ChatMessageRole::User => {
                first_seen = true;
                let parts = user_content_parts(&msg.content);
                input.push(json!({
                    "type": "message",
                    "role": "user",
                    "content": parts,
                }));
            }
            ChatMessageRole::Assistant => {
                first_seen = true;
                let text = extract_text(&msg.content).unwrap_or_default();
                let tool_calls = msg.tool_calls.as_deref().unwrap_or(&[]);
                if tool_calls.is_empty() {
                    if !text.is_empty() {
                        input.push(json!({
                            "type": "message",
                            "role": "assistant",
                            "content": [{"type": "output_text", "text": text}],
                        }));
                    }
                } else {
                    if !text.is_empty() {
                        input.push(json!({
                            "type": "message",
                            "role": "assistant",
                            "content": [{"type": "output_text", "text": text}],
                        }));
                    }
                    for tc in tool_calls {
                        let id = tc
                            .get("id")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        let func = tc.get("function").cloned().unwrap_or(Value::Null);
                        let name = func
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        let args = func
                            .get("arguments")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        input.push(json!({
                            "type": "function_call",
                            "call_id": id,
                            "name": name,
                            "arguments": args,
                        }));
                    }
                }
            }
            ChatMessageRole::Tool => {
                first_seen = true;
                let call_id = msg.tool_call_id.clone().unwrap_or_default();
                let output = extract_text(&msg.content).unwrap_or_default();
                input.push(json!({
                    "type": "function_call_output",
                    "call_id": call_id,
                    "output": output,
                }));
            }
        }
    }

    (instructions, input)
}

/// Chat Completions 的 function tool（`{"type":"function","function":{name,description,parameters}}`）
/// → Responses 顶层 `{"type":"function","name":..,"description":..,"parameters":..}`。
/// 输入若不是 function 类型则原样保留（向前兼容用户/插件已声明的 Responses 形状）。
fn convert_tools_to_responses(tools: &[Value]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            let kind = t.get("type").and_then(Value::as_str);
            if kind != Some("function") {
                return t.clone();
            }
            let func = match t.get("function") {
                Some(f) => f,
                None => return t.clone(),
            };
            let mut out = json!({"type": "function"});
            if let Some(name) = func.get("name").and_then(Value::as_str) {
                out["name"] = Value::String(name.to_string());
            }
            if let Some(desc) = func.get("description").and_then(Value::as_str) {
                if !desc.trim().is_empty() {
                    out["description"] = Value::String(desc.to_string());
                }
            }
            if let Some(params) = func.get("parameters") {
                out["parameters"] = params.clone();
            } else {
                out["parameters"] = json!({"type": "object"});
            }
            out
        })
        .collect()
}

fn extract_text(content: &Option<ChatMessageContent>) -> Option<String> {
    match content {
        Some(ChatMessageContent::Text(s)) => Some(s.clone()),
        Some(ChatMessageContent::Parts(parts)) => {
            let s: String = parts
                .iter()
                .filter_map(|p| p.text.clone())
                .collect::<Vec<_>>()
                .join("");
            if s.is_empty() {
                None
            } else {
                Some(s)
            }
        }
        None => None,
    }
}

fn user_content_parts(content: &Option<ChatMessageContent>) -> Vec<Value> {
    match content {
        Some(ChatMessageContent::Text(s)) => {
            vec![json!({"type": "input_text", "text": s})]
        }
        Some(ChatMessageContent::Parts(parts)) => {
            let mut out = Vec::with_capacity(parts.len());
            for p in parts {
                let text = p.text.clone().unwrap_or_default();
                out.push(json!({"type": "input_text", "text": text}));
            }
            if out.is_empty() {
                out.push(json!({"type": "input_text", "text": ""}));
            }
            out
        }
        None => vec![json!({"type": "input_text", "text": ""})],
    }
}

// ============================================================================
// 非流式响应 → 内部 ChatResponse
// ============================================================================

/// 把 Responses `POST /v1/responses` 的非流式 JSON 翻译为内部 [`ChatResponse`]，
/// 与 Completions choices[0] 形状对齐（`message.content` + `finish_reason` + `usage`）。
fn responses_payload_to_chat_response(raw: &Value) -> ChatResponse {
    let id = raw.get("id").and_then(Value::as_str).map(str::to_string);

    // 拼合 output[].content[].text 中所有 output_text 片段，作为 assistant 的可见内容。
    let mut text_buf = String::new();
    let mut tool_calls: Vec<Value> = Vec::new();
    if let Some(items) = raw.get("output").and_then(Value::as_array) {
        for item in items {
            let kind = item.get("type").and_then(Value::as_str);
            match kind {
                Some("message") => {
                    if let Some(parts) = item.get("content").and_then(Value::as_array) {
                        for part in parts {
                            if part.get("type").and_then(Value::as_str) == Some("output_text") {
                                if let Some(t) = part.get("text").and_then(Value::as_str) {
                                    text_buf.push_str(t);
                                }
                            }
                        }
                    }
                }
                Some("function_call") => {
                    let call_id = item
                        .get("call_id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let name = item
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let args = item
                        .get("arguments")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    tool_calls.push(json!({
                        "id": call_id,
                        "type": "function",
                        "function": {
                            "name": name,
                            "arguments": args,
                        }
                    }));
                }
                _ => {}
            }
        }
    }

    let finish_reason = raw
        .get("status")
        .and_then(Value::as_str)
        .map(|s| match s {
            "completed" => "stop".to_string(),
            "incomplete" => raw
                .get("incomplete_details")
                .and_then(|d| d.get("reason"))
                .and_then(Value::as_str)
                .unwrap_or("incomplete")
                .to_string(),
            other => other.to_string(),
        })
        .or_else(|| {
            if !tool_calls.is_empty() {
                Some("tool_calls".to_string())
            } else {
                None
            }
        });

    let usage = raw
        .get("usage")
        .map(|u| crate::core::llm::types::TokenUsage {
            prompt_tokens: u.get("input_tokens").and_then(Value::as_u64).unwrap_or(0) as u32,
            completion_tokens: u.get("output_tokens").and_then(Value::as_u64).unwrap_or(0) as u32,
            total_tokens: u
                .get("total_tokens")
                .and_then(Value::as_u64)
                .map(|v| v as u32),
        });

    let message = if tool_calls.is_empty() {
        ChatMessage::assistant(text_buf)
    } else if text_buf.is_empty() {
        ChatMessage::assistant_with_tool_calls(None, tool_calls)
    } else {
        ChatMessage::assistant_with_tool_calls(Some(&text_buf), tool_calls)
    };

    ChatResponse {
        id,
        choices: vec![ChatResponseChoice {
            index: 0,
            message,
            finish_reason,
        }],
        usage,
    }
}

// ============================================================================
// 流式：SSE / NDJSON 双解析 → StreamEvent
// ============================================================================

/// Responses 流式解析器：默认按 SSE（`event: ...\ndata: {...}\n\n`）解码；
/// 若 Content-Type 为 NDJSON 或首帧不是 SSE 形态，则切换到 **一行一条 JSON** 的 NDJSON 模式。
/// 切换决策只做一次（一次性锁定，避免每帧重判抖动）。
struct ResponsesStream<S> {
    inner: S,
    buffer: Vec<u8>,
    pending: std::vec::IntoIter<StreamEvent>,
    /// `Some(true)` = NDJSON, `Some(false)` = SSE, `None` = 未决（首帧探测）
    mode: Option<bool>,
    /// 累积 tool_call arguments：`(item_id, output_index, name)` → `(index, accum_args)`；
    /// 用于判定每个分片对应的 `ToolCallDelta.index` 与首帧 `name` 触发时机。
    tool_calls: Vec<ToolCallTrack>,
}

#[derive(Debug)]
struct ToolCallTrack {
    item_id: String,
    call_id: String,
    name: String,
    name_emitted: bool,
}

impl<S> ResponsesStream<S> {
    fn new(inner: S, prefer_ndjson: bool) -> Self {
        Self {
            inner,
            buffer: Vec::new(),
            pending: Vec::new().into_iter(),
            mode: if prefer_ndjson { Some(true) } else { None },
            tool_calls: Vec::new(),
        }
    }

    fn process_chunk(&mut self, raw: &str) -> Result<Vec<StreamEvent>, AppError> {
        let value: Value = serde_json::from_str(raw).map_err(|e| {
            AppError::Llm(format!("解析 Responses chunk 失败: {} | raw={}", e, raw))
        })?;
        Ok(responses_chunk_to_events(&value, &mut self.tool_calls))
    }
}

impl<S, E> Stream for ResponsesStream<S>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
    E: std::fmt::Display,
{
    type Item = Result<StreamEvent, AppError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if let Some(evt) = self.pending.next() {
            return Poll::Ready(Some(Ok(evt)));
        }

        loop {
            match Pin::new(&mut self.inner).poll_next(cx) {
                Poll::Ready(Some(Ok(bytes))) => {
                    self.buffer.extend_from_slice(&bytes);
                    if self.mode.is_none() {
                        // 首次探测：若 buffer 含 "data: " 字面量则按 SSE，否则若已经看到 \n 则按 NDJSON。
                        if buffer_starts_with_sse(&self.buffer) {
                            self.mode = Some(false);
                        } else if self.buffer.contains(&b'\n') {
                            self.mode = Some(true);
                        } else {
                            // 数据不够判，继续读
                            continue;
                        }
                    }
                    let is_ndjson = self.mode.unwrap();
                    match drain_buffer(&mut self.buffer, is_ndjson) {
                        Ok(chunks) => {
                            let mut events = Vec::new();
                            for raw in chunks {
                                match self.process_chunk(&raw) {
                                    Ok(mut evs) => events.append(&mut evs),
                                    Err(e) => return Poll::Ready(Some(Err(e))),
                                }
                            }
                            if let Some((first, rest)) = events.split_first() {
                                let first = first.clone();
                                #[allow(clippy::unnecessary_to_owned)]
                                let pending_vec = rest.to_vec();
                                self.pending = pending_vec.into_iter();
                                return Poll::Ready(Some(Ok(first)));
                            }
                        }
                        Err(e) => return Poll::Ready(Some(Err(e))),
                    }
                }
                Poll::Ready(Some(Err(e))) => {
                    return Poll::Ready(Some(Err(AppError::Llm(e.to_string()))));
                }
                Poll::Ready(None) => {
                    // 流结束：把残留 buffer 当作最后一帧再尝试解析。
                    if !self.buffer.is_empty() {
                        let is_ndjson = self.mode.unwrap_or(true);
                        let drained = drain_buffer(&mut self.buffer, is_ndjson);
                        if let Ok(chunks) = drained {
                            let mut events = Vec::new();
                            for raw in chunks {
                                if let Ok(mut evs) = self.process_chunk(&raw) {
                                    events.append(&mut evs);
                                }
                            }
                            if let Some((first, rest)) = events.split_first() {
                                let first = first.clone();
                                #[allow(clippy::unnecessary_to_owned)]
                                let pending_vec = rest.to_vec();
                                self.pending = pending_vec.into_iter();
                                return Poll::Ready(Some(Ok(first)));
                            }
                        }
                    }
                    return Poll::Ready(None);
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

fn buffer_starts_with_sse(buf: &[u8]) -> bool {
    // 任一 SSE 帧都包含 `data: ` 行；`event: ` 也是 SSE 标志。
    let prefix_data = b"data: ";
    let prefix_event = b"event: ";
    buf.windows(prefix_data.len()).any(|w| w == prefix_data)
        || buf.windows(prefix_event.len()).any(|w| w == prefix_event)
}

/// 从 buffer 中按当前模式榨取已完成的 chunk JSON 字符串列表。
fn drain_buffer(buffer: &mut Vec<u8>, ndjson: bool) -> Result<Vec<String>, AppError> {
    let mut out = Vec::new();
    if ndjson {
        // NDJSON：按 \n 切，单行非空即一条 JSON。
        while let Some(pos) = buffer.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = buffer.drain(..=pos).collect();
            let trimmed_end = if line.last() == Some(&b'\n') {
                &line[..line.len() - 1]
            } else {
                &line[..]
            };
            let s = std::str::from_utf8(trimmed_end)
                .map_err(|e| AppError::Llm(format!("UTF-8 错误: {}", e)))?;
            let s = s.trim();
            if s.is_empty() {
                continue;
            }
            out.push(s.to_string());
        }
    } else {
        // SSE：以 \n\n 分块，块内逐行抓 `data: ...`。
        let sep = b"\n\n";
        while let Some(pos) = buffer.windows(sep.len()).position(|w| w == sep) {
            let end = pos + sep.len();
            let block: Vec<u8> = buffer.drain(..end).collect();
            let s = std::str::from_utf8(&block)
                .map_err(|e| AppError::Llm(format!("UTF-8 错误: {}", e)))?;
            for line in s.lines() {
                let line = line.trim();
                if let Some(data) = line.strip_prefix("data: ") {
                    if data == "[DONE]" {
                        continue;
                    }
                    out.push(data.to_string());
                }
            }
        }
    }
    Ok(out)
}

/// 把单条 Responses chunk JSON 翻译为 0..N 个 [`StreamEvent`]。
fn responses_chunk_to_events(
    value: &Value,
    tool_calls: &mut Vec<ToolCallTrack>,
) -> Vec<StreamEvent> {
    let mut events = Vec::new();
    let kind = value.get("type").and_then(Value::as_str).unwrap_or("");
    match kind {
        "response.output_text.delta" => {
            if let Some(delta) = value.get("delta").and_then(Value::as_str) {
                if !delta.is_empty() {
                    events.push(StreamEvent::ContentDelta {
                        delta: delta.to_string(),
                    });
                }
            }
        }
        "response.output_item.added" => {
            if let Some(item) = value.get("item") {
                if item.get("type").and_then(Value::as_str) == Some("function_call") {
                    let item_id = item
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let call_id = item
                        .get("call_id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let name = item
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let index = tool_calls.len() as u32;
                    tool_calls.push(ToolCallTrack {
                        item_id,
                        call_id: call_id.clone(),
                        name: name.clone(),
                        name_emitted: false,
                    });
                    events.push(StreamEvent::ToolCallDelta {
                        index,
                        id: Some(call_id),
                        name: Some(name),
                        arguments_delta: None,
                    });
                    if let Some(track) = tool_calls.last_mut() {
                        track.name_emitted = true;
                    }
                }
            }
        }
        "response.function_call_arguments.delta" => {
            let item_id = value.get("item_id").and_then(Value::as_str).unwrap_or("");
            let delta = value.get("delta").and_then(Value::as_str).unwrap_or("");
            if let Some((idx, track)) = tool_calls
                .iter_mut()
                .enumerate()
                .find(|(_, t)| t.item_id == item_id)
            {
                let index = idx as u32;
                let mut id_field = None;
                let mut name_field = None;
                if !track.name_emitted {
                    id_field = Some(track.call_id.clone());
                    name_field = Some(track.name.clone());
                    track.name_emitted = true;
                }
                events.push(StreamEvent::ToolCallDelta {
                    index,
                    id: id_field,
                    name: name_field,
                    arguments_delta: if delta.is_empty() {
                        None
                    } else {
                        Some(delta.to_string())
                    },
                });
            }
        }
        "response.completed" | "response.done" => {
            // 优先展开 incomplete reason；否则按 stop / tool_calls 结尾。
            let response = value.get("response");
            let reason = response
                .and_then(|r| r.get("incomplete_details"))
                .and_then(|d| d.get("reason"))
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| "stop".to_string());
            events.push(StreamEvent::FinishReason { reason });
            if let Some(usage) = response.and_then(|r| r.get("usage")) {
                let prompt = usage
                    .get("input_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as u32;
                let completion = usage
                    .get("output_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as u32;
                let total = usage
                    .get("total_tokens")
                    .and_then(Value::as_u64)
                    .map(|v| v as u32);
                events.push(StreamEvent::Usage {
                    prompt_tokens: prompt,
                    completion_tokens: completion,
                    total_tokens: total,
                });
            }
        }
        "response.failed" | "error" => {
            // 错误事件不在此处转 AppError（流向外的 Result Err 由 process_chunk 上游决定）；
            // 这里映射为一个 `FinishReason { reason: "error" }`，便于上层统一处理。
            let msg = value
                .get("response")
                .and_then(|r| r.get("error"))
                .and_then(|e| e.get("message"))
                .and_then(Value::as_str)
                .or_else(|| value.get("message").and_then(Value::as_str))
                .unwrap_or("unknown");
            events.push(StreamEvent::FinishReason {
                reason: format!("error:{}", msg),
            });
        }
        _ => {
            // 其它 event 暂忽略（reasoning / output_item.done 等），不影响主链路。
        }
    }
    events
}

// 单测见 `core::llm::tests::openai_responses_test`（wire + tools + payload + count_tokens
// + 流式解析覆盖在外部测试目录中按 plan §5 Phase E.2 / E.3 拆分）。
#[cfg(test)]
#[path = "tests/openai_responses_test.rs"]
mod tests;
