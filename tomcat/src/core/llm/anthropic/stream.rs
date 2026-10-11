use bytes::Bytes;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio_stream::Stream;

use crate::core::llm::replay_policy::ProviderCompatProfile;
use crate::core::llm::types::{StreamEvent, ThinkingSource};
use crate::infra::error::{
    llm_error_with_source, llm_stream_interrupted_error, AppError, LlmErrorStage,
};

use super::wire::{anthropic_token_usage, final_stream_events};

const PROVIDER_NAME: &str = "anthropic";

#[derive(Debug, Default)]
struct ThinkingTrack {
    text: String,
    signature: Option<String>,
}

#[derive(Debug)]
pub(super) struct ToolTrack {
    pub(super) arguments: String,
}

/// Provider counters are cumulative but arrive in partial frames. Keep them raw until
/// the terminal event; normalized prompt totals cannot recover disjoint input counters.
#[derive(Default)]
struct StreamUsage {
    input_tokens: u32,
    output_tokens: u32,
    cache_read_tokens: Option<u32>,
    cache_write_tokens: Option<u32>,
}

pub(super) struct AnthropicStream<S> {
    inner: S,
    buffer: Vec<u8>,
    pending: std::vec::IntoIter<StreamEvent>,
    thinking: BTreeMap<u32, ThinkingTrack>,
    tool_calls: BTreeMap<u32, ToolTrack>,
    usage: Option<StreamUsage>,
    stop_reason: Option<String>,
    source_profile: ProviderCompatProfile,
    continuity_enabled: bool,
    terminal_emitted: bool,
    finished: bool,
    /// 与 OpenAI Responses 对齐：首个有效事件前的解析失败更可能是中转流截断，可重试；
    /// 已输出事件后才失败则是协议错误，保留 Parse 阶段供诊断。
    seen_valid_event: bool,
}

impl<S> AnthropicStream<S> {
    pub(super) fn new(
        inner: S,
        source_profile: ProviderCompatProfile,
        continuity_enabled: bool,
    ) -> Self {
        Self {
            inner,
            buffer: Vec::new(),
            pending: Vec::new().into_iter(),
            thinking: BTreeMap::new(),
            tool_calls: BTreeMap::new(),
            usage: None,
            stop_reason: None,
            source_profile,
            continuity_enabled,
            terminal_emitted: false,
            finished: false,
            seen_valid_event: false,
        }
    }

    fn parse_block(&mut self, raw: &str) -> Result<Vec<StreamEvent>, AppError> {
        let mut event_name = None;
        let mut data_lines = Vec::new();
        for line in raw.lines() {
            if let Some(rest) = line.strip_prefix("event:") {
                event_name = Some(rest.trim().to_string());
            } else if let Some(rest) = line.strip_prefix("data:") {
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
            let summary = format!("解析 Anthropic SSE 失败: {error} | raw={raw}");
            if self.seen_valid_event {
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
        let event = event_name
            .or_else(|| {
                value
                    .get("type")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_default();
        let events = self.events_for_value(&event, &value)?;
        self.seen_valid_event |= !events.is_empty();
        Ok(events)
    }

    fn events_for_value(
        &mut self,
        event: &str,
        value: &Value,
    ) -> Result<Vec<StreamEvent>, AppError> {
        match event {
            "ping" => Ok(Vec::new()),
            "message_start" => {
                self.record_usage(value.get("message").and_then(|msg| msg.get("usage")));
                Ok(Vec::new())
            }
            "content_block_start" => {
                let index = value.get("index").and_then(Value::as_u64).unwrap_or(0) as u32;
                match value
                    .get("content_block")
                    .and_then(|block| block.get("type"))
                    .and_then(Value::as_str)
                {
                    Some("tool_use") => {
                        let block = value.get("content_block").cloned().unwrap_or(Value::Null);
                        let id = block
                            .get("id")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        let name = block
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        self.tool_calls.insert(
                            index,
                            ToolTrack {
                                arguments: String::new(),
                            },
                        );
                        Ok(vec![StreamEvent::ToolCallDelta {
                            index,
                            id: Some(id),
                            name: Some(name),
                            arguments_delta: None,
                        }])
                    }
                    Some("thinking") => {
                        self.thinking.entry(index).or_default();
                        Ok(Vec::new())
                    }
                    _ => Ok(Vec::new()),
                }
            }
            "content_block_delta" => {
                let index = value.get("index").and_then(Value::as_u64).unwrap_or(0) as u32;
                let delta = value.get("delta").cloned().unwrap_or(Value::Null);
                match delta.get("type").and_then(Value::as_str) {
                    Some("text_delta") => Ok(vec![StreamEvent::ContentDelta {
                        delta: delta
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    }]),
                    Some("thinking_delta") => {
                        let thinking_delta = delta
                            .get("thinking")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        if thinking_delta.is_empty() {
                            return Ok(Vec::new());
                        }
                        self.thinking
                            .entry(index)
                            .or_default()
                            .text
                            .push_str(&thinking_delta);
                        Ok(vec![StreamEvent::Thinking {
                            delta: thinking_delta,
                            source: ThinkingSource::Raw,
                            signature: None,
                        }])
                    }
                    Some("signature_delta") => {
                        let signature = delta
                            .get("signature")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        if let Some(track) = self.thinking.get_mut(&index) {
                            track.signature = (!signature.is_empty()).then_some(signature);
                        }
                        Ok(Vec::new())
                    }
                    Some("input_json_delta") => {
                        let partial_json = delta
                            .get("partial_json")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        if partial_json.is_empty() {
                            return Ok(Vec::new());
                        }
                        if let Some(track) = self.tool_calls.get_mut(&index) {
                            track.arguments.push_str(&partial_json);
                        }
                        Ok(vec![StreamEvent::ToolCallDelta {
                            index,
                            id: None,
                            name: None,
                            arguments_delta: Some(partial_json),
                        }])
                    }
                    _ => Ok(Vec::new()),
                }
            }
            "content_block_stop" => Ok(Vec::new()),
            "message_delta" => {
                self.record_usage(value.get("usage"));
                if let Some(stop_reason) = value
                    .get("delta")
                    .and_then(|delta| delta.get("stop_reason").and_then(Value::as_str))
                {
                    self.stop_reason = Some(stop_reason.to_string());
                }
                Ok(Vec::new())
            }
            "message_stop" => {
                self.terminal_emitted = true;
                Ok(self.build_terminal_events())
            }
            "error" => {
                let error = value.get("error").cloned().unwrap_or_else(|| json!({}));
                let message = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("anthropic error")
                    .to_string();
                let code = error
                    .get("type")
                    .or_else(|| error.get("code"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let reason = code
                    .as_deref()
                    .filter(|code| !code.is_empty())
                    .map(|code| format!("error:{code}"))
                    .unwrap_or_else(|| "error".to_string());
                let mut events = vec![StreamEvent::LlmError {
                    reason,
                    message,
                    code,
                }];
                self.terminal_emitted = true;
                events.extend(self.build_terminal_events());
                Ok(events)
            }
            _ => Ok(Vec::new()),
        }
    }

    fn record_usage(&mut self, usage: Option<&Value>) {
        let Some(usage) = usage else {
            return;
        };
        let counter = |name| {
            usage
                .get(name)
                .and_then(Value::as_u64)
                .map(|value| value as u32)
        };
        let input_tokens = counter("input_tokens");
        let output_tokens = counter("output_tokens");
        let cache_read_tokens = counter("cache_read_input_tokens");
        let cache_write_tokens = counter("cache_creation_input_tokens");
        if [
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
        ]
        .iter()
        .all(Option::is_none)
        {
            return;
        }
        let current = self.usage.get_or_insert_with(StreamUsage::default);
        current.input_tokens = input_tokens.unwrap_or(current.input_tokens);
        current.output_tokens = output_tokens.unwrap_or(current.output_tokens);
        current.cache_read_tokens = cache_read_tokens.or(current.cache_read_tokens);
        current.cache_write_tokens = cache_write_tokens.or(current.cache_write_tokens);
    }

    fn build_terminal_events(&self) -> Vec<StreamEvent> {
        let thinking_blocks = self
            .thinking
            .values()
            .map(|block| {
                json!({
                    "type": "thinking",
                    "thinking": block.text,
                    "signature": block.signature,
                })
            })
            .collect::<Vec<_>>();
        let thinking_text = {
            let chunks = self
                .thinking
                .values()
                .filter_map(|block| {
                    let trimmed = block.text.trim();
                    (!trimmed.is_empty()).then(|| trimmed.to_string())
                })
                .collect::<Vec<_>>();
            if chunks.is_empty() {
                None
            } else {
                Some(chunks.join("\n\n"))
            }
        };
        final_stream_events(
            &self.source_profile,
            self.continuity_enabled,
            thinking_blocks,
            thinking_text,
            !self.tool_calls.is_empty(),
            self.usage.as_ref().map(|usage| {
                anthropic_token_usage(
                    usage.input_tokens,
                    usage.output_tokens,
                    usage.cache_read_tokens,
                    usage.cache_write_tokens,
                )
            }),
            self.stop_reason.clone(),
        )
    }
}

impl<S> Stream for AnthropicStream<S>
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
                    while let Some(pos) = find_double_newline(&self.buffer) {
                        let block = self.buffer.drain(..pos + 2).collect::<Vec<_>>();
                        let raw = String::from_utf8_lossy(&block).to_string();
                        let block = raw.trim();
                        if block.is_empty() {
                            continue;
                        }
                        match self.parse_block(block) {
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
                        "读取 Anthropic 流响应失败".to_string(),
                        anyhow::anyhow!(error),
                    ))));
                }
                Poll::Ready(None) => {
                    self.finished = true;
                    // 未分隔残留沿用原行为丢弃，不能把 EOF 伪装成协议终止。
                    self.buffer.clear();
                    if !self.terminal_emitted {
                        if self.stop_reason.is_none() {
                            return Poll::Ready(Some(Err(llm_stream_interrupted_error(
                                PROVIDER_NAME,
                                "stream closed before stop_reason or message_stop",
                            ))));
                        }
                        self.terminal_emitted = true;
                        self.pending = self.build_terminal_events().into_iter();
                        continue;
                    }
                    return Poll::Ready(None);
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

fn find_double_newline(buffer: &[u8]) -> Option<usize> {
    buffer.windows(2).position(|window| window == b"\n\n")
}

#[cfg(test)]
#[path = "tests/stream_test.rs"]
mod tests;
