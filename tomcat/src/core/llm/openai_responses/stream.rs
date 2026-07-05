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
use std::collections::HashMap;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio_stream::Stream;

use super::payload::{infer_terminal_metadata, ResponsesTerminalMetadata};
use crate::core::llm::replay_policy::{
    replay_requirement_for_profile, CaptureMode, ProviderCompatProfile,
};
use crate::core::llm::types::{
    ContinuityMetadata, ProviderRefs, ReasoningContinuation, ReasoningFormat, StreamEvent,
    ThinkingSource,
};
use crate::infra::error::{llm_error_with_source, AppError, LlmErrorStage};

const PROVIDER_NAME: &str = "openai-responses";

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
    reasoning: ReasoningState,
    source_profile: ProviderCompatProfile,
    continuity_enabled: bool,
}

#[derive(Debug)]
pub(super) struct ToolCallTrack {
    pub(super) item_id: String,
    pub(super) call_id: String,
    pub(super) name: String,
    pub(super) arguments: String,
    pub(super) name_emitted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct ReasoningKey {
    item_id: String,
    index: u32,
}

#[derive(Debug, Default)]
pub(super) struct ReasoningState {
    summary: HashMap<ReasoningKey, String>,
    raw: HashMap<ReasoningKey, String>,
    items: Vec<Value>,
    response_id: Option<String>,
}

impl ReasoningState {
    fn ensure_started(&mut self, source: ThinkingSource, key: ReasoningKey) {
        self.buffer_mut(source).entry(key).or_default();
    }

    fn apply_delta(
        &mut self,
        source: ThinkingSource,
        key: ReasoningKey,
        delta: &str,
    ) -> Option<StreamEvent> {
        if delta.is_empty() {
            return None;
        }
        self.buffer_mut(source)
            .entry(key)
            .or_default()
            .push_str(delta);
        Some(StreamEvent::Thinking {
            delta: delta.to_string(),
            source,
            signature: None,
        })
    }

    fn apply_snapshot(
        &mut self,
        source: ThinkingSource,
        key: ReasoningKey,
        text: &str,
    ) -> Option<StreamEvent> {
        if text.is_empty() {
            return None;
        }
        let buf = self.buffer_mut(source).entry(key.clone()).or_default();
        if buf == text {
            return None;
        }
        if let Some(suffix) = text.strip_prefix(buf.as_str()) {
            if suffix.is_empty() {
                return None;
            }
            buf.push_str(suffix);
            return Some(StreamEvent::Thinking {
                delta: suffix.to_string(),
                source,
                signature: None,
            });
        }
        tracing::warn!(
            target: "tomcat::llm::openai_responses",
            item_id = %key.item_id,
            index = key.index,
            source = ?source,
            old_len = buf.len(),
            new_len = text.len(),
            "reasoning snapshot does not extend accumulated text; replacing"
        );
        *buf = text.to_string();
        Some(StreamEvent::Thinking {
            delta: text.to_string(),
            source,
            signature: None,
        })
    }

    fn buffer_mut(&mut self, source: ThinkingSource) -> &mut HashMap<ReasoningKey, String> {
        match source {
            ThinkingSource::Summary => &mut self.summary,
            ThinkingSource::Raw => &mut self.raw,
        }
    }

    fn record_reasoning_item(&mut self, item: &Value) {
        self.items.push(item.clone());
    }

    fn record_response_id(&mut self, response: Option<&Value>) {
        if let Some(id) = response
            .and_then(|resp| resp.get("id"))
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        {
            self.response_id = Some(id.to_string());
        }
    }

    fn thinking_text(&self) -> Option<String> {
        let mut summary_entries: Vec<_> = self.summary.iter().collect();
        summary_entries.sort_by_key(|(key, _)| *key);
        let summary_text = summary_entries
            .into_iter()
            .filter_map(|(_, text)| {
                let trimmed = text.trim();
                (!trimmed.is_empty()).then(|| trimmed.to_string())
            })
            .collect::<Vec<_>>();
        if !summary_text.is_empty() {
            return Some(summary_text.join("\n\n"));
        }

        let mut raw_entries: Vec<_> = self.raw.iter().collect();
        raw_entries.sort_by_key(|(key, _)| *key);
        let raw_text = raw_entries
            .into_iter()
            .filter_map(|(_, text)| {
                let trimmed = text.trim();
                (!trimmed.is_empty()).then(|| trimmed.to_string())
            })
            .collect::<Vec<_>>();
        if raw_text.is_empty() {
            None
        } else {
            Some(raw_text.join("\n\n"))
        }
    }

    fn build_snapshot(
        &self,
        source_profile: &ProviderCompatProfile,
        had_tool_call: bool,
        continuity_enabled: bool,
    ) -> Option<StreamEvent> {
        if !continuity_enabled || !matches!(source_profile.capture_mode, CaptureMode::OpaqueItems) {
            return None;
        }
        let thinking_text = self.thinking_text();
        let reasoning_continuation = if !self.items.is_empty()
            && source_profile.provider == "openai"
            && source_profile.api_family == "responses"
        {
            Some(ReasoningContinuation {
                source_provider: source_profile.provider.clone(),
                source_api: source_profile.api_family.clone(),
                source_model: source_profile.model_family.clone(),
                format: ReasoningFormat::OpenaiResponsesReasoningItems,
                opaque_payload: Value::Array(self.items.clone()),
                fallback_text: thinking_text.clone(),
                provider_refs: if source_profile.supports_response_id_hint {
                    self.response_id.clone().map(|id| ProviderRefs {
                        openai_response_id: Some(id),
                    })
                } else {
                    None
                },
            })
        } else {
            None
        };
        if thinking_text.is_none() && reasoning_continuation.is_none() {
            return None;
        }
        Some(StreamEvent::ReasoningSnapshot {
            thinking_text,
            continuity: reasoning_continuation.as_ref().map(|_| ContinuityMetadata {
                had_tool_call,
                replay_requirement: replay_requirement_for_profile(source_profile, had_tool_call),
            }),
            reasoning_continuation,
        })
    }
}

impl<S> ResponsesStream<S> {
    pub(super) fn new(
        inner: S,
        prefer_ndjson: bool,
        source_profile: ProviderCompatProfile,
        continuity_enabled: bool,
    ) -> Self {
        Self {
            inner,
            buffer: Vec::new(),
            pending: Vec::new().into_iter(),
            mode: if prefer_ndjson { Some(true) } else { None },
            tool_calls: Vec::new(),
            reasoning: ReasoningState::default(),
            source_profile,
            continuity_enabled,
        }
    }

    fn process_chunk(&mut self, raw: &str) -> Result<Vec<StreamEvent>, AppError> {
        let value: Value = serde_json::from_str(raw).map_err(|e| {
            llm_error_with_source(
                PROVIDER_NAME,
                LlmErrorStage::Parse,
                "解析 Responses chunk 失败".to_string(),
                anyhow::anyhow!("{e} | raw={raw}"),
            )
        })?;
        Ok(responses_chunk_to_events_with_state(
            &value,
            &mut self.tool_calls,
            &mut self.reasoning,
            &self.source_profile,
            self.continuity_enabled,
        ))
    }
}

impl<S> Stream for ResponsesStream<S>
where
    S: Stream<Item = Result<Bytes, AppError>> + Unpin,
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
                    return Poll::Ready(Some(Err(e)));
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
            let s = std::str::from_utf8(trimmed_end).map_err(|e| {
                llm_error_with_source(
                    PROVIDER_NAME,
                    LlmErrorStage::Parse,
                    "NDJSON UTF-8 错误".to_string(),
                    e,
                )
            })?;
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
            let s = std::str::from_utf8(&block).map_err(|e| {
                llm_error_with_source(
                    PROVIDER_NAME,
                    LlmErrorStage::Parse,
                    "SSE UTF-8 错误".to_string(),
                    e,
                )
            })?;
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

fn push_terminal_events(
    events: &mut Vec<StreamEvent>,
    meta: ResponsesTerminalMetadata,
    usage: Option<&Value>,
) {
    if let Some(message) = meta.notice_message.clone() {
        events.push(StreamEvent::LlmNotice {
            finish_reason: meta
                .finish_reason
                .clone()
                .unwrap_or_else(|| "notice".to_string()),
            message,
        });
    }
    if let Some(message) = meta.error_message.clone() {
        events.push(StreamEvent::LlmError {
            reason: meta
                .finish_reason
                .clone()
                .unwrap_or_else(|| format!("error:{message}")),
            message,
            code: meta.error_code.clone(),
        });
    }
    if let Some(reason) = meta.finish_reason {
        events.push(StreamEvent::FinishReason { reason });
    }
    if let Some(usage) = usage {
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

fn upsert_tool_call_track(
    tool_calls: &mut Vec<ToolCallTrack>,
    item_id: &str,
    call_id: &str,
    name: &str,
) -> usize {
    if let Some((index, track)) = tool_calls
        .iter_mut()
        .enumerate()
        .find(|(_, track)| track.item_id == item_id)
    {
        if !call_id.is_empty() {
            track.call_id = call_id.to_string();
        }
        if !name.is_empty() {
            track.name = name.to_string();
        }
        return index;
    }

    let index = tool_calls.len();
    tool_calls.push(ToolCallTrack {
        item_id: item_id.to_string(),
        call_id: call_id.to_string(),
        name: name.to_string(),
        arguments: String::new(),
        name_emitted: false,
    });
    index
}

fn tool_call_done_events(item: &Value, tool_calls: &mut Vec<ToolCallTrack>) -> Vec<StreamEvent> {
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
    let final_arguments = item
        .get("arguments")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    let index = upsert_tool_call_track(tool_calls, &item_id, &call_id, &name);
    let track = &mut tool_calls[index];
    let mut id_field = None;
    let mut name_field = None;
    if !track.name_emitted && (!track.call_id.is_empty() || !track.name.is_empty()) {
        if !track.call_id.is_empty() {
            id_field = Some(track.call_id.clone());
        }
        if !track.name.is_empty() {
            name_field = Some(track.name.clone());
        }
        track.name_emitted = true;
    }

    let arguments_delta = if final_arguments.is_empty() {
        None
    } else if track.arguments.is_empty() {
        track.arguments = final_arguments.clone();
        Some(final_arguments)
    } else if let Some(suffix) = final_arguments.strip_prefix(track.arguments.as_str()) {
        if suffix.is_empty() {
            None
        } else {
            track.arguments.push_str(suffix);
            Some(suffix.to_string())
        }
    } else {
        tracing::warn!(
            target: "tomcat::llm::openai_responses",
            item_id = %track.item_id,
            call_id = %track.call_id,
            old_len = track.arguments.len(),
            new_len = final_arguments.len(),
            "function_call done arguments do not extend accumulated delta; keeping final snapshot"
        );
        track.arguments = final_arguments;
        None
    };

    if id_field.is_none() && name_field.is_none() && arguments_delta.is_none() {
        return Vec::new();
    }

    vec![StreamEvent::ToolCallDelta {
        index: index as u32,
        id: id_field,
        name: name_field,
        arguments_delta,
    }]
}

/// 把单条 Responses chunk JSON 翻译为 0..N 个 [`StreamEvent`]。
#[cfg(test)]
pub(super) fn responses_chunk_to_events(
    value: &Value,
    tool_calls: &mut Vec<ToolCallTrack>,
) -> Vec<StreamEvent> {
    let mut reasoning = ReasoningState::default();
    let profile = ProviderCompatProfile::openai_responses("gpt-5");
    responses_chunk_to_events_with_state(value, tool_calls, &mut reasoning, &profile, true)
}

pub(super) fn responses_chunk_to_events_with_state(
    value: &Value,
    tool_calls: &mut Vec<ToolCallTrack>,
    reasoning: &mut ReasoningState,
    source_profile: &ProviderCompatProfile,
    continuity_enabled: bool,
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
                    let arguments = item
                        .get("arguments")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let index =
                        upsert_tool_call_track(tool_calls, &item_id, &call_id, &name) as u32;
                    if let Some(track) = tool_calls.get_mut(index as usize) {
                        track.arguments = arguments.clone();
                    }
                    events.push(StreamEvent::ToolCallDelta {
                        index,
                        id: (!call_id.is_empty()).then_some(call_id),
                        name: (!name.is_empty()).then_some(name),
                        arguments_delta: (!arguments.is_empty()).then_some(arguments),
                    });
                    if let Some(track) = tool_calls.get_mut(index as usize) {
                        track.name_emitted = true;
                    }
                }
            }
        }
        "response.function_call_arguments.delta" => {
            let item_id = value.get("item_id").and_then(Value::as_str).unwrap_or("");
            let delta = value.get("delta").and_then(Value::as_str).unwrap_or("");
            let index = upsert_tool_call_track(tool_calls, item_id, "", "");
            if let Some(track) = tool_calls.get_mut(index) {
                let mut id_field = None;
                let mut name_field = None;
                if !track.name_emitted {
                    if !track.call_id.is_empty() {
                        id_field = Some(track.call_id.clone());
                    }
                    if !track.name.is_empty() {
                        name_field = Some(track.name.clone());
                    }
                    track.name_emitted = id_field.is_some() || name_field.is_some();
                }
                if !delta.is_empty() {
                    track.arguments.push_str(delta);
                }
                events.push(StreamEvent::ToolCallDelta {
                    index: index as u32,
                    id: id_field,
                    name: name_field,
                    arguments_delta: (!delta.is_empty()).then(|| delta.to_string()),
                });
            }
        }
        "response.completed" | "response.done" => {
            let response = value.get("response");
            reasoning.record_response_id(response);
            let meta = infer_terminal_metadata(
                Some("completed"),
                response,
                None,
                None,
                !tool_calls.is_empty(),
            );
            push_terminal_events(
                &mut events,
                meta,
                response.and_then(|resp| resp.get("usage")),
            );
            if let Some(snapshot) =
                reasoning.build_snapshot(source_profile, !tool_calls.is_empty(), continuity_enabled)
            {
                events.push(snapshot);
            }
        }
        "response.incomplete" => {
            let response = value.get("response");
            reasoning.record_response_id(response);
            let meta = infer_terminal_metadata(
                Some("incomplete"),
                response,
                None,
                None,
                !tool_calls.is_empty(),
            );
            push_terminal_events(
                &mut events,
                meta,
                response.and_then(|resp| resp.get("usage")),
            );
            if let Some(snapshot) =
                reasoning.build_snapshot(source_profile, !tool_calls.is_empty(), continuity_enabled)
            {
                events.push(snapshot);
            }
        }
        "response.failed" => {
            let response = value.get("response");
            reasoning.record_response_id(response);
            let meta = infer_terminal_metadata(
                Some("failed"),
                response,
                None,
                None,
                !tool_calls.is_empty(),
            );
            push_terminal_events(
                &mut events,
                meta,
                response.and_then(|resp| resp.get("usage")),
            );
            if let Some(snapshot) =
                reasoning.build_snapshot(source_profile, !tool_calls.is_empty(), continuity_enabled)
            {
                events.push(snapshot);
            }
        }
        "error" => {
            let top_level_error = value.get("error").or(Some(value));
            let top_level_message = value.get("message").and_then(Value::as_str);
            let meta = infer_terminal_metadata(
                Some("failed"),
                None,
                top_level_error,
                top_level_message,
                !tool_calls.is_empty(),
            );
            push_terminal_events(&mut events, meta, None);
            if let Some(snapshot) =
                reasoning.build_snapshot(source_profile, !tool_calls.is_empty(), continuity_enabled)
            {
                events.push(snapshot);
            }
        }
        // T2-P1：Responses reasoning 事件按 (item_id, index) 分桶去重。
        "response.reasoning.delta" | "response.reasoning_text.delta" => {
            if let Some(delta) = value.get("delta").and_then(Value::as_str) {
                let key = raw_reasoning_key(value);
                if let Some(event) = reasoning.apply_delta(ThinkingSource::Raw, key, delta) {
                    events.push(event);
                }
            }
        }
        "response.reasoning_summary_text.delta" => {
            if let Some(delta) = value.get("delta").and_then(Value::as_str) {
                let key = summary_reasoning_key(value);
                if let Some(event) = reasoning.apply_delta(ThinkingSource::Summary, key, delta) {
                    events.push(event);
                }
            }
        }
        "response.reasoning_summary_part.added" => {
            reasoning.ensure_started(ThinkingSource::Summary, summary_reasoning_key(value));
        }
        "response.content_part.added" => {
            if value
                .get("part")
                .and_then(|part| part.get("type"))
                .and_then(Value::as_str)
                == Some("reasoning_text")
            {
                reasoning.ensure_started(ThinkingSource::Raw, raw_reasoning_key(value));
            }
        }
        "response.reasoning_summary_text.done" => {
            if let Some(text) = direct_text_field(value) {
                let key = summary_reasoning_key(value);
                if let Some(event) = reasoning.apply_snapshot(ThinkingSource::Summary, key, &text) {
                    events.push(event);
                }
            }
        }
        "response.reasoning_text.done" | "response.reasoning.done" => {
            if let Some(text) = direct_text_field(value) {
                let key = raw_reasoning_key(value);
                if let Some(event) = reasoning.apply_snapshot(ThinkingSource::Raw, key, &text) {
                    events.push(event);
                }
            }
        }
        "response.reasoning_summary_part.done" => {
            if let Some(text) = value.get("part").and_then(extract_text) {
                let key = summary_reasoning_key(value);
                if let Some(event) = reasoning.apply_snapshot(ThinkingSource::Summary, key, &text) {
                    events.push(event);
                }
            }
        }
        "response.reasoning_summary.delta" | "response.reasoning_summary.done" => {
            for (index, text) in extract_summary_entries(value) {
                let key = ReasoningKey {
                    item_id: value
                        .get("item_id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    index,
                };
                if let Some(event) = reasoning.apply_snapshot(ThinkingSource::Summary, key, &text) {
                    events.push(event);
                }
            }
        }
        "response.output_item.done" => {
            if let Some(item) = value.get("item") {
                if item
                    .get("type")
                    .and_then(Value::as_str)
                    .map(|t| t.contains("reasoning"))
                    .unwrap_or(false)
                {
                    reasoning.record_reasoning_item(item);
                    let item_id = item
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    for (index, text) in extract_item_summary_entries(item) {
                        let key = ReasoningKey {
                            item_id: item_id.clone(),
                            index,
                        };
                        if let Some(event) =
                            reasoning.apply_snapshot(ThinkingSource::Summary, key, &text)
                        {
                            events.push(event);
                        }
                    }
                    for (index, text) in extract_item_raw_entries(item) {
                        let key = ReasoningKey {
                            item_id: item_id.clone(),
                            index,
                        };
                        if let Some(event) =
                            reasoning.apply_snapshot(ThinkingSource::Raw, key, &text)
                        {
                            events.push(event);
                        }
                    }
                } else if item.get("type").and_then(Value::as_str) == Some("function_call") {
                    events.extend(tool_call_done_events(item, tool_calls));
                } else {
                    tracing::debug!(
                        target: "tomcat::llm::openai_responses",
                        event = %kind,
                        "ignoring unknown Responses SSE event"
                    );
                }
            } else {
                tracing::debug!(
                    target: "tomcat::llm::openai_responses",
                    event = %kind,
                    "ignoring unknown Responses SSE event"
                );
            }
        }
        other => {
            if other.starts_with("response.reasoning") {
                tracing::debug!(
                    target: "tomcat::llm::openai_responses",
                    event = %other,
                    payload = ?value,
                    "unhandled Responses reasoning event; ignoring"
                );
            } else {
                // 未知事件类型：trace 一行供运维排查，但**不阻断**主链路。
                // 用 `debug!` 而非 `warn!`，避免某些网关的 ping/keepalive 事件刷日志。
                tracing::debug!(
                    target: "tomcat::llm::openai_responses",
                    event = %other,
                    "ignoring unknown Responses SSE event"
                );
            }
        }
    }
    events
}

fn summary_reasoning_key(value: &Value) -> ReasoningKey {
    ReasoningKey {
        item_id: value
            .get("item_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        index: value
            .get("summary_index")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32,
    }
}

fn raw_reasoning_key(value: &Value) -> ReasoningKey {
    ReasoningKey {
        item_id: value
            .get("item_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        index: value
            .get("content_index")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32,
    }
}

fn direct_text_field(value: &Value) -> Option<String> {
    for key in ["text", "delta", "summary_text", "summary", "content"] {
        if let Some(raw) = value.get(key) {
            if let Some(text) = extract_text(raw) {
                return Some(text);
            }
        }
    }
    None
}

fn extract_summary_entries(value: &Value) -> Vec<(u32, String)> {
    let entries = extract_indexed_entries(value.get("summary"), Some("summary_text"));
    if !entries.is_empty() {
        return entries;
    }
    value
        .get("summary")
        .and_then(extract_text)
        .map(|text| {
            vec![(
                value
                    .get("summary_index")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as u32,
                text,
            )]
        })
        .unwrap_or_default()
}

fn extract_item_summary_entries(item: &Value) -> Vec<(u32, String)> {
    let entries = extract_indexed_entries(item.get("summary"), Some("summary_text"));
    if !entries.is_empty() {
        return entries;
    }
    item.get("summary_text")
        .and_then(extract_text)
        .map(|text| vec![(0, text)])
        .unwrap_or_default()
}

fn extract_item_raw_entries(item: &Value) -> Vec<(u32, String)> {
    let entries = extract_indexed_entries(item.get("content"), Some("reasoning_text"));
    if !entries.is_empty() {
        return entries;
    }
    item.get("text")
        .and_then(extract_text)
        .map(|text| vec![(0, text)])
        .unwrap_or_default()
}

fn extract_indexed_entries(
    container: Option<&Value>,
    expected_type: Option<&str>,
) -> Vec<(u32, String)> {
    let Some(items) = container.and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .enumerate()
        .filter_map(|(idx, item)| {
            let actual_type = item.get("type").and_then(Value::as_str);
            if expected_type.is_some() && actual_type.is_some() && actual_type != expected_type {
                return None;
            }
            extract_text(item).map(|text| (idx as u32, text))
        })
        .collect()
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
