//! Shared `GenerateContentResponse` accumulator for both SSE streaming and
//! non-streaming `:generateContent` calls.

use std::collections::HashSet;
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use serde_json::{json, Value};
use tokio_stream::Stream;

use crate::core::llm::replay_policy::{replay_requirement_for_profile, ProviderCompatProfile};
use crate::core::llm::types::{
    ChatMessage, ChatResponse, ChatResponseChoice, ContinuityMetadata, ProviderRefs,
    ReasoningContinuation, ReasoningFormat, StreamEvent, ThinkingSource, TokenUsage,
};
use crate::infra::error::{
    llm_error_with_source, llm_stream_interrupted_error, llm_stream_terminal_error, AppError,
    LlmErrorStage,
};
use crate::infra::i18n::tr;

use super::PROVIDER_NAME;

fn next_synthetic_call_id() -> String {
    format!("gemini_call_{}", uuid::Uuid::new_v4().simple())
}

#[derive(Debug, Clone)]
struct AccumulatedToolCall {
    id: String,
    name: String,
    args: Value,
}

#[derive(Debug, Clone)]
struct TerminalError {
    reason: String,
    message: String,
    code: Option<String>,
}

pub(super) struct GeminiResponseAccumulator {
    source_profile: ProviderCompatProfile,
    continuity_enabled: bool,
    response_id: Option<String>,
    thinking_text: String,
    visible_text: String,
    tool_calls: Vec<AccumulatedToolCall>,
    parts: Vec<Value>,
    upstream_call_ids: HashSet<String>,
    usage: Option<TokenUsage>,
    finish_reason: Option<String>,
    terminal_error: Option<TerminalError>,
}

impl GeminiResponseAccumulator {
    pub(super) fn new(source_profile: ProviderCompatProfile, continuity_enabled: bool) -> Self {
        Self {
            source_profile,
            continuity_enabled,
            response_id: None,
            thinking_text: String::new(),
            visible_text: String::new(),
            tool_calls: Vec::new(),
            parts: Vec::new(),
            upstream_call_ids: HashSet::new(),
            usage: None,
            finish_reason: None,
            terminal_error: None,
        }
    }

    pub(super) fn has_terminal_signal(&self) -> bool {
        self.finish_reason.is_some() || self.terminal_error.is_some()
    }

    /// Feeds one `GenerateContentResponse` JSON object and returns immediate
    /// incremental events (`Thinking` / `ContentDelta`). Tool calls, reasoning
    /// snapshot, final usage, and finish/error events are emitted once in
    /// [`Self::build_terminal_events`].
    pub(super) fn feed_frame(&mut self, value: &Value) -> Vec<StreamEvent> {
        let mut events = Vec::new();

        if let Some(resp_id) = value
            .get("responseId")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            self.response_id = Some(resp_id.to_string());
        }

        if let Some(err_obj) = value.get("error") {
            let message = err_obj
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Gemini stream error")
                .to_string();
            let status = err_obj
                .get("status")
                .and_then(Value::as_str)
                .map(str::to_string);
            let code = map_upstream_error_code(status.as_deref());
            self.terminal_error = Some(TerminalError {
                reason: status
                    .as_deref()
                    .map(|s| format!("error:{s}"))
                    .unwrap_or_else(|| "error".to_string()),
                message,
                code,
            });
            return events;
        }

        if let Some(parsed_usage) = value.get("usageMetadata").and_then(parse_usage_metadata) {
            self.usage = Some(parsed_usage);
        }

        if let Some(block_reason) = value
            .get("promptFeedback")
            .and_then(|pf| pf.get("blockReason"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            self.terminal_error = Some(TerminalError {
                reason: format!("block_reason:{block_reason}"),
                message: format!("Gemini blocked prompt: {block_reason}"),
                code: Some("content_filter".to_string()),
            });
            return events;
        }

        let Some(candidate) = value
            .get("candidates")
            .and_then(Value::as_array)
            .and_then(|cands| cands.first())
        else {
            return events;
        };

        if let Some(parts) = candidate
            .get("content")
            .and_then(|c| c.get("parts"))
            .and_then(Value::as_array)
        {
            for part in parts {
                self.record_part(part);
                if let Some(fc) = part.get("functionCall") {
                    self.record_function_call(fc);
                    continue;
                }
                let is_thought = part.get("thought").and_then(Value::as_bool) == Some(true);
                let Some(text) = part.get("text").and_then(Value::as_str) else {
                    continue;
                };
                if text.is_empty() {
                    continue;
                }
                if is_thought {
                    self.thinking_text.push_str(text);
                    events.push(StreamEvent::Thinking {
                        delta: text.to_string(),
                        source: ThinkingSource::Summary,
                        signature: None,
                    });
                } else {
                    self.visible_text.push_str(text);
                    events.push(StreamEvent::ContentDelta {
                        delta: text.to_string(),
                    });
                }
            }
        }

        if let Some(reason) = candidate
            .get("finishReason")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            self.finish_reason = Some(reason.to_string());
        }

        events
    }

    /// Coalesce only unsigned, otherwise field-free text deltas. Signed parts,
    /// unknown fields and call boundaries must survive capture unchanged.
    fn record_part(&mut self, part: &Value) {
        if !self.continuity_enabled {
            return;
        }
        if let Some(thought) = plain_text_kind(part) {
            let text = part["text"].as_str().unwrap();
            if text.is_empty() {
                return;
            }
            if let Some(previous) = self.parts.last_mut() {
                if plain_text_kind(previous) == Some(thought) {
                    if let Some(Value::String(previous_text)) = previous.get_mut("text") {
                        previous_text.push_str(text);
                    }
                    return;
                }
            }
        }
        self.parts.push(part.clone());
    }

    fn record_function_call(&mut self, fc: &Value) {
        let name = fc
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let args = fc.get("args").cloned().unwrap_or_else(|| json!({}));
        let id = fc
            .get("id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(next_synthetic_call_id);

        // Parameters are not streamed: one native call part is one complete call.
        if fc
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty())
            && !self.upstream_call_ids.insert(id.clone())
        {
            self.terminal_error = Some(TerminalError {
                reason: "function_call:duplicate_id".into(),
                message: "Gemini returned a duplicate functionCall id".into(),
                code: Some("upstream_error".into()),
            });
            return;
        }
        self.tool_calls.push(AccumulatedToolCall { id, name, args });
    }

    fn build_continuity_state(
        &self,
    ) -> (
        Option<String>,
        Option<ReasoningContinuation>,
        Option<ContinuityMetadata>,
    ) {
        let thinking_text = {
            let trimmed = self.thinking_text.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        };
        let had_tool_call = !self.tool_calls.is_empty();

        let has_signature = self.parts.iter().any(|part| {
            part.get("thoughtSignature")
                .and_then(Value::as_str)
                .is_some_and(|s| !s.is_empty())
        });
        let reasoning_continuation = if self.continuity_enabled && has_signature {
            Some(ReasoningContinuation {
                source_provider: self.source_profile.provider.clone(),
                source_api: self.source_profile.api_family.clone(),
                source_model: self.source_profile.model_family.clone(),
                format: ReasoningFormat::GeminiThoughtSignatures,
                opaque_payload: Value::Array(self.parts.clone()),
                fallback_text: None,
                provider_refs: Some(ProviderRefs {
                    openai_response_id: None,
                    replay_profile_id: Some(self.source_profile.profile_id.clone()),
                }),
            })
        } else {
            None
        };

        let continuity = reasoning_continuation.as_ref().map(|_| ContinuityMetadata {
            had_tool_call,
            replay_requirement: replay_requirement_for_profile(&self.source_profile, had_tool_call),
        });

        (thinking_text, reasoning_continuation, continuity)
    }

    fn classify_finish(&self) -> Result<&'static str, TerminalError> {
        if let Some(err) = &self.terminal_error {
            return Err(err.clone());
        }
        match self.finish_reason.as_deref() {
            Some("STOP") => Ok(if self.tool_calls.is_empty() {
                "stop"
            } else {
                "tool_calls"
            }),
            Some("MAX_TOKENS") => Ok("max_tokens"),
            None => Err(TerminalError {
                reason: "finish_reason:missing".to_string(),
                message: "Gemini response is missing finishReason".to_string(),
                code: Some("upstream_error".to_string()),
            }),
            Some(
                raw @ ("SAFETY"
                | "PROHIBITED_CONTENT"
                | "BLOCKLIST"
                | "SPII"
                | "RECITATION"
                | "IMAGE_SAFETY"
                | "IMAGE_PROHIBITED_CONTENT"),
            ) => Err(TerminalError {
                reason: format!("finish_reason:{raw}"),
                message: format!("Gemini blocked response: {raw}"),
                code: Some("content_filter".to_string()),
            }),
            Some(raw @ ("MALFORMED_FUNCTION_CALL" | "UNEXPECTED_TOOL_CALL")) => {
                Err(TerminalError {
                    reason: format!("finish_reason:{raw}"),
                    message: format!("Gemini tool call generation failed: {raw}"),
                    code: Some("upstream_error".to_string()),
                })
            }
            Some(other) => Err(TerminalError {
                reason: format!("finish_reason:{other}"),
                message: format!("Gemini finished with abnormal reason: {other}"),
                code: None,
            }),
        }
    }

    pub(super) fn build_terminal_events(&self) -> Vec<StreamEvent> {
        let mut events = Vec::new();
        match self.classify_finish() {
            Err(err) => {
                if let Some(usage) = &self.usage {
                    events.push(usage_event(usage));
                }
                events.push(StreamEvent::LlmError {
                    reason: err.reason,
                    message: err.message,
                    code: err.code,
                });
            }
            Ok(reason) => {
                for (idx, tc) in self.tool_calls.iter().enumerate() {
                    events.push(StreamEvent::ToolCallDelta {
                        index: idx as u32,
                        id: Some(tc.id.clone()),
                        name: Some(tc.name.clone()),
                        arguments_delta: Some(serialize_args(&tc.args)),
                    });
                }
                let (thinking_text, reasoning_continuation, continuity) =
                    self.build_continuity_state();
                if thinking_text.is_some() || reasoning_continuation.is_some() {
                    events.push(StreamEvent::ReasoningSnapshot {
                        thinking_text,
                        reasoning_continuation,
                        continuity,
                    });
                }
                if let Some(usage) = &self.usage {
                    events.push(usage_event(usage));
                }
                if reason == "max_tokens" {
                    events.push(StreamEvent::LlmNotice {
                        finish_reason: reason.to_string(),
                        message: tr("llm.maxOutputNotice", &[]),
                    });
                }
                events.push(StreamEvent::FinishReason {
                    reason: reason.to_string(),
                });
            }
        }
        events
    }

    pub(super) fn into_chat_response(self) -> Result<ChatResponse, AppError> {
        let finish_reason = self
            .classify_finish()
            .map_err(|err| llm_stream_terminal_error(PROVIDER_NAME, err.message, err.code))?
            .to_string();

        let (thinking_text, reasoning_continuation, continuity) = self.build_continuity_state();
        let tool_calls_json: Vec<Value> = self
            .tool_calls
            .iter()
            .map(|tc| {
                json!({
                    "id": tc.id,
                    "type": "function",
                    "function": {
                        "name": tc.name,
                        "arguments": serialize_args(&tc.args),
                    }
                })
            })
            .collect();

        let message = if tool_calls_json.is_empty() {
            ChatMessage::assistant(self.visible_text)
        } else {
            ChatMessage::assistant_with_tool_calls(
                (!self.visible_text.is_empty()).then_some(self.visible_text.as_str()),
                tool_calls_json,
            )
        }
        .with_reasoning_state(thinking_text, reasoning_continuation, continuity);

        Ok(ChatResponse {
            id: self.response_id,
            choices: vec![ChatResponseChoice {
                index: 0,
                message,
                finish_reason: Some(finish_reason),
            }],
            usage: self.usage,
        })
    }
}

fn plain_text_kind(part: &Value) -> Option<bool> {
    let fields = part.as_object()?;
    if !part.get("text")?.is_string()
        || fields.keys().any(|key| key != "text" && key != "thought")
        || fields.get("thought").is_some_and(|v| !v.is_boolean())
    {
        return None;
    }
    Some(
        part.get("thought")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    )
}

fn serialize_args(args: &Value) -> String {
    serde_json::to_string(args).unwrap_or_else(|_| "{}".to_string())
}

fn usage_event(usage: &TokenUsage) -> StreamEvent {
    StreamEvent::Usage {
        prompt_tokens: usage.prompt_tokens,
        completion_tokens: usage.completion_tokens,
        total_tokens: usage.total_tokens,
        cache_read_tokens: usage.cache_read_tokens,
        cache_write_tokens: usage.cache_write_tokens,
        reasoning_tokens: usage.reasoning_tokens,
        text_tokens: usage.text_tokens,
    }
}

fn parse_usage_metadata(usage: &Value) -> Option<TokenUsage> {
    let prompt_raw = opt_u32(usage, "promptTokenCount");
    let tool_prompt = opt_u32(usage, "toolUsePromptTokenCount");
    let cache_read = opt_u32(usage, "cachedContentTokenCount");
    let candidates = opt_u32(usage, "candidatesTokenCount");
    let thoughts = opt_u32(usage, "thoughtsTokenCount");
    let total = opt_u32(usage, "totalTokenCount");

    if [
        prompt_raw,
        tool_prompt,
        cache_read,
        candidates,
        thoughts,
        total,
    ]
    .iter()
    .all(Option::is_none)
    {
        return None;
    }

    let prompt_tokens = prompt_raw
        .unwrap_or(0)
        .saturating_add(tool_prompt.unwrap_or(0));
    let text_tokens = candidates.unwrap_or(0);
    let completion_tokens = text_tokens.saturating_add(thoughts.unwrap_or(0));
    Some(TokenUsage {
        prompt_tokens,
        completion_tokens,
        cache_read_tokens: cache_read,
        cache_write_tokens: None,
        total_tokens: Some(
            total.unwrap_or_else(|| prompt_tokens.saturating_add(completion_tokens)),
        ),
        reasoning_tokens: thoughts,
        text_tokens: Some(text_tokens),
    })
}

fn opt_u32(obj: &Value, key: &str) -> Option<u32> {
    obj.get(key).and_then(Value::as_u64).map(|n| n as u32)
}

fn map_upstream_error_code(status: Option<&str>) -> Option<String> {
    match status {
        Some("RESOURCE_EXHAUSTED") => Some("rate_limit_exceeded".to_string()),
        Some("UNAUTHENTICATED" | "PERMISSION_DENIED") => Some("authentication_error".to_string()),
        Some("INTERNAL" | "UNAVAILABLE" | "DEADLINE_EXCEEDED") => {
            Some("upstream_error".to_string())
        }
        _ => None,
    }
}

pub(super) struct GeminiStream<S> {
    inner: S,
    buffer: Vec<u8>,
    pending: std::vec::IntoIter<StreamEvent>,
    accumulator: GeminiResponseAccumulator,
    finished: bool,
    seen_valid_frame: bool,
}

impl<S> GeminiStream<S> {
    pub(super) fn new(
        inner: S,
        source_profile: ProviderCompatProfile,
        continuity_enabled: bool,
    ) -> Self {
        Self {
            inner,
            buffer: Vec::new(),
            pending: Vec::new().into_iter(),
            accumulator: GeminiResponseAccumulator::new(source_profile, continuity_enabled),
            finished: false,
            seen_valid_frame: false,
        }
    }

    fn parse_block(&mut self, raw: &str) -> Result<Vec<StreamEvent>, AppError> {
        let mut data_lines = Vec::new();
        for line in raw.lines() {
            let line = line.trim_end_matches('\r');
            if let Some(rest) = line.strip_prefix("data:") {
                data_lines.push(rest.trim_start().to_string());
            }
        }
        if data_lines.is_empty() {
            return Ok(Vec::new());
        }
        let data = data_lines.join("\n");
        if data == "[DONE]" {
            return Ok(Vec::new());
        }
        let value: Value = serde_json::from_str(&data).map_err(|error| {
            let summary = format!("解析 Gemini SSE 失败: {error} | raw={raw}");
            if self.seen_valid_frame {
                llm_error_with_source(
                    PROVIDER_NAME,
                    LlmErrorStage::Parse,
                    summary,
                    anyhow::anyhow!("{error} | raw={raw}"),
                )
            } else {
                llm_stream_interrupted_error(PROVIDER_NAME, summary)
            }
        })?;
        self.seen_valid_frame = true;
        Ok(self.accumulator.feed_frame(&value))
    }
}

impl<S> Stream for GeminiStream<S>
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Unpin,
{
    type Item = Result<StreamEvent, AppError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            if let Some(event) = self.pending.next() {
                return Poll::Ready(Some(Ok(event)));
            }
            if self.finished {
                return Poll::Ready(None);
            }
            match Pin::new(&mut self.inner).poll_next(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    self.buffer.extend_from_slice(&chunk);
                    let mut parsed = Vec::new();
                    while let Some((pos, delim_len)) = next_sse_boundary(&self.buffer) {
                        let block = self.buffer.drain(..pos + delim_len).collect::<Vec<_>>();
                        let raw = String::from_utf8_lossy(&block);
                        let trimmed = raw.trim();
                        if trimmed.is_empty() {
                            continue;
                        }
                        match self.parse_block(trimmed) {
                            Ok(events) => parsed.extend(events),
                            Err(error) => return Poll::Ready(Some(Err(error))),
                        }
                    }
                    if !parsed.is_empty() {
                        self.pending = parsed.into_iter();
                    }
                }
                Poll::Ready(Some(Err(error))) => {
                    return Poll::Ready(Some(Err(llm_error_with_source(
                        PROVIDER_NAME,
                        if error.is_timeout() {
                            LlmErrorStage::ReadTimeout
                        } else {
                            LlmErrorStage::BodyRead
                        },
                        tr("llm.geminiReadFailed", &[]),
                        anyhow::anyhow!(error),
                    ))));
                }
                Poll::Ready(None) => {
                    self.finished = true;
                    let trailing = String::from_utf8_lossy(&self.buffer).trim().to_string();
                    self.buffer.clear();
                    let mut final_events = Vec::new();
                    if !trailing.is_empty() {
                        match self.parse_block(&trailing) {
                            Ok(events) => final_events.extend(events),
                            Err(_) if !self.accumulator.has_terminal_signal() => {
                                return Poll::Ready(Some(Err(llm_stream_interrupted_error(
                                    PROVIDER_NAME,
                                    "stream closed with incomplete SSE frame before finishReason",
                                ))));
                            }
                            Err(_) => {}
                        }
                    }
                    if !self.accumulator.has_terminal_signal() {
                        return Poll::Ready(Some(Err(llm_stream_interrupted_error(
                            PROVIDER_NAME,
                            "stream closed before finishReason",
                        ))));
                    }
                    final_events.extend(self.accumulator.build_terminal_events());
                    if !final_events.is_empty() {
                        self.pending = final_events.into_iter();
                        continue;
                    }
                    return Poll::Ready(None);
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

/// Finds the earliest SSE frame boundary (`\r\n\r\n` or `\n\n`), returning `(offset, len)`.
fn next_sse_boundary(buffer: &[u8]) -> Option<(usize, usize)> {
    let mut i = 0;
    while i + 1 < buffer.len() {
        if buffer[i] == b'\n' && buffer[i + 1] == b'\n' {
            return Some((i, 2));
        }
        if i + 3 < buffer.len()
            && buffer[i] == b'\r'
            && buffer[i + 1] == b'\n'
            && buffer[i + 2] == b'\r'
            && buffer[i + 3] == b'\n'
        {
            return Some((i, 4));
        }
        i += 1;
    }
    None
}

#[cfg(test)]
#[path = "tests/stream_test.rs"]
mod tests;
