//! # Responses 流式：SSE / NDJSON 双解析 → `StreamEvent`
//!
//! 默认按 SSE（`event: ...\ndata: {...}\n\n`）解码；若 Content-Type 为 NDJSON
//! 或首帧不是 SSE 形态，则切换到 **一行一条 JSON** 的 NDJSON 模式。切换决策只
//! 做一次（一次性锁定，避免每帧重判抖动）。
//!
//! 与 [`super::payload`] 同样从原 `openai_responses.rs` 切出，专管「字节流 →
//! `Vec<StreamEvent>`」翻译；HTTP 客户端 / Provider 装配仍归 [`super`]。

use bytes::Bytes;
use serde_json::Value;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio_stream::Stream;

use crate::core::llm::types::StreamEvent;
use crate::infra::error::AppError;

/// Responses 流式解析器：默认按 SSE（`event: ...\ndata: {...}\n\n`）解码；
/// 若 Content-Type 为 NDJSON 或首帧不是 SSE 形态，则切换到 **一行一条 JSON** 的 NDJSON 模式。
/// 切换决策只做一次（一次性锁定，避免每帧重判抖动）。
pub(super) struct ResponsesStream<S> {
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
pub(super) struct ToolCallTrack {
    pub(super) item_id: String,
    pub(super) call_id: String,
    pub(super) name: String,
    pub(super) name_emitted: bool,
}

impl<S> ResponsesStream<S> {
    pub(super) fn new(inner: S, prefer_ndjson: bool) -> Self {
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
pub(super) fn responses_chunk_to_events(
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
        // T2-P0-006 P2b：Reasoning / Thinking 流式事件归一映射。
        //
        // OpenAI Responses 在不同版本/网关下出现过以下事件名（均包含 reasoning 字段）：
        // - `response.reasoning.delta`           （旧 reasoning text 流）
        // - `response.reasoning_text.delta`      （reasoning text 主流命名）
        // - `response.reasoning_summary_text.delta`（reasoning summary 文本流）
        // - `response.reasoning_summary.delta`   （部分网关的 summary 事件）
        // 它们大多按 `delta: string` 形态携带增量，但也可能是对象/数组；
        // 这里尽量提取可读文本映射为 Thinking。`*.done` 事件不携带新增 delta，
        // 因此本期忽略，避免重复 emit；后续若需要 done 信号可再扩展。
        "response.reasoning.delta"
        | "response.reasoning_text.delta"
        | "response.reasoning_summary_text.delta"
        | "response.reasoning_summary.delta" => push_reasoning_delta_event(&mut events, value, kind),
        "response.reasoning.done"
        | "response.reasoning_text.done"
        | "response.reasoning_summary_text.done"
        | "response.reasoning_summary.done" => {
            // 已知的「reasoning 段结束」事件——本期不发额外 StreamEvent，仅静默吃掉，
            // 避免被下面 `_` 分支的未知事件 trace 误报。
        }
        other => {
            if other.starts_with("response.reasoning") {
                tracing::debug!(
                    target: "pi_wasm::llm::openai_responses",
                    event = %other,
                    payload = ?value,
                    "unhandled Responses reasoning event; ignoring"
                );
            } else {
                // 未知事件类型：trace 一行供运维排查，但**不阻断**主链路。
                // 用 `debug!` 而非 `warn!`，避免某些网关的 ping/keepalive 事件刷日志。
                tracing::debug!(
                    target: "pi_wasm::llm::openai_responses",
                    event = %other,
                    "ignoring unknown Responses SSE event"
                );
            }
        }
    }
    events
}

fn push_reasoning_delta_event(events: &mut Vec<StreamEvent>, value: &Value, kind: &str) {
    if let Some(delta) = extract_reasoning_delta(value) {
        events.push(StreamEvent::Thinking {
            delta,
            signature: None,
        });
    } else {
        tracing::debug!(
            target: "pi_wasm::llm::openai_responses",
            event = %kind,
            payload = ?value,
            "reasoning delta event had no extractable text"
        );
    }
}

fn extract_reasoning_delta(value: &Value) -> Option<String> {
    for key in ["delta", "summary", "text"] {
        if let Some(raw) = value.get(key) {
            if let Some(text) = extract_text(raw) {
                return Some(text);
            }
        }
    }
    None
}

fn extract_text(value: &Value) -> Option<String> {
    match value {
        // 流式 delta 必须保留原始空白（尤其 token 边界空格），否则会出现单词粘连。
        // 仅在真正空串时跳过；" " 这类空白分片也应保留。
        Value::String(text) => (!text.is_empty()).then(|| text.to_string()),
        Value::Array(items) => {
            let parts: Vec<String> = items.iter().filter_map(extract_text).collect();
            if parts.is_empty() {
                None
            } else {
                Some(parts.join(" "))
            }
        }
        Value::Object(map) => {
            for key in ["text", "delta", "summary", "summary_text", "content"] {
                if let Some(child) = map.get(key) {
                    if let Some(text) = extract_text(child) {
                        return Some(text);
                    }
                }
            }
            None
        }
        _ => None,
    }
}
