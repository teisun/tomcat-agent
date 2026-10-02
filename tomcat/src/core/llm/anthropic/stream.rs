use bytes::Bytes;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio_stream::Stream;

use crate::core::llm::replay_policy::ProviderCompatProfile;
use crate::core::llm::types::{StreamEvent, ThinkingSource, TokenUsage};
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

pub(super) struct AnthropicStream<S> {
    inner: S,
    buffer: Vec<u8>,
    pending: std::vec::IntoIter<StreamEvent>,
    thinking: BTreeMap<u32, ThinkingTrack>,
    tool_calls: BTreeMap<u32, ToolTrack>,
    usage: Option<TokenUsage>,
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
        let input_tokens = usage
            .get("input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32;
        let output_tokens = usage
            .get("output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32;
        let cache_read_tokens = usage
            .get("cache_read_input_tokens")
            .and_then(Value::as_u64)
            .map(|value| value as u32);
        let cache_write_tokens = usage
            .get("cache_creation_input_tokens")
            .and_then(Value::as_u64)
            .map(|value| value as u32);
        if input_tokens == 0
            && output_tokens == 0
            && cache_read_tokens.is_none()
            && cache_write_tokens.is_none()
        {
            return;
        }
        self.usage = Some(anthropic_token_usage(
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
        ));
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
            self.usage.clone(),
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
mod tests {
    use bytes::Bytes;
    use tokio_stream::empty;
    use tokio_stream::StreamExt;

    use super::AnthropicStream;
    use crate::core::llm::replay_policy::ProviderCompatProfile;
    use crate::core::llm::types::{ReasoningFormat, StreamEvent};
    use crate::infra::error::{classify_llm_failure, llm_stage, LlmErrorStage, LlmFailureKind};

    #[test]
    fn parse_block_emits_thinking_and_terminal_events() {
        let mut stream = AnthropicStream::new(
            empty::<Result<Bytes, reqwest::Error>>(),
            ProviderCompatProfile::anthropic_messages("claude-opus-4-6"),
            true,
        );

        assert!(stream
            .parse_block(
                "event: message_start\ndata: {\"message\":{\"usage\":{\"input_tokens\":12}}}\n\n",
            )
            .expect("message_start")
            .is_empty());
        assert!(stream
            .parse_block(
                "event: content_block_start\ndata: {\"index\":0,\"content_block\":{\"type\":\"thinking\"}}\n\n",
            )
            .expect("thinking start")
            .is_empty());
        let thinking_events = stream
            .parse_block(
                "event: content_block_delta\ndata: {\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"reason step\"}}\n\n",
            )
            .expect("thinking delta");
        assert!(matches!(
            thinking_events.as_slice(),
            [StreamEvent::Thinking { delta, .. }] if delta == "reason step"
        ));
        assert!(stream
            .parse_block(
                "event: content_block_delta\ndata: {\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"sig_1\"}}\n\n",
            )
            .expect("signature")
            .is_empty());
        assert!(stream
            .parse_block(
                "event: message_delta\ndata: {\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"input_tokens\":10,\"output_tokens\":1257,\"cache_read_input_tokens\":202789,\"cache_creation_input_tokens\":6801}}\n\n",
            )
            .expect("message delta")
            .is_empty());

        let terminal = stream
            .parse_block("event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n")
            .expect("message stop");

        assert!(terminal.iter().any(|event| matches!(
            event,
            StreamEvent::ReasoningSnapshot {
                reasoning_continuation: Some(continuation),
                ..
            } if continuation.format == ReasoningFormat::AnthropicThinkingBlocks
        )));
        assert!(terminal.iter().any(|event| matches!(
            event,
            StreamEvent::Usage {
                prompt_tokens: 209_600,
                completion_tokens: 1_257,
                cache_read_tokens: Some(202_789),
                cache_write_tokens: Some(6_801),
                total_tokens: Some(210_857),
                ..
            }
        )));
        assert!(terminal.iter().any(|event| matches!(
            event,
            StreamEvent::FinishReason { reason } if reason == "stop"
        )));
    }

    #[tokio::test]
    async fn eof_without_stop_signal_is_interrupted() {
        let source = tokio_stream::iter(vec![Ok::<Bytes, reqwest::Error>(Bytes::from(
            "event: content_block_delta\ndata: {\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"partial\"}}\n\n",
        ))]);
        let mut stream = AnthropicStream::new(
            source,
            ProviderCompatProfile::anthropic_messages("claude-opus-4-6"),
            true,
        );
        assert!(
            matches!(stream.next().await.expect("delta").expect("valid delta"),
                StreamEvent::ContentDelta { delta } if delta == "partial"
            )
        );
        let error = stream
            .next()
            .await
            .expect("interruption, not fabricated terminal events")
            .expect_err("missing stop signal");
        assert_eq!(
            classify_llm_failure(&error).kind,
            LlmFailureKind::StreamInterrupted
        );
        assert_eq!(llm_stage(&error), Some(LlmErrorStage::BodyRead));
        assert!(stream.next().await.is_none());
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn eof_after_stop_reason_without_message_stop_finishes_once() {
        let body = concat!(
            "event: content_block_start\ndata: {\"index\":0,\"content_block\":{\"type\":\"thinking\"}}\n\n",
            "event: content_block_delta\ndata: {\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"plan\"}}\n\n",
            "event: message_delta\ndata: {\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"input_tokens\":1,\"output_tokens\":2}}\n\n",
        );
        let source = tokio_stream::iter(vec![Ok::<Bytes, reqwest::Error>(Bytes::from(body))]);
        let mut stream = AnthropicStream::new(
            source,
            ProviderCompatProfile::anthropic_messages("claude-opus-4-6"),
            true,
        );
        assert!(
            matches!(stream.next().await.expect("thinking").expect("valid thinking"),
                StreamEvent::Thinking { delta, .. } if delta == "plan"
            )
        );
        let mut terminal = Vec::new();
        for _ in 0..3 {
            terminal.push(
                stream
                    .next()
                    .await
                    .expect("terminal event")
                    .expect("no interruption"),
            );
        }
        assert_eq!(
            terminal
                .iter()
                .filter(|event| matches!(event,
                    StreamEvent::FinishReason { reason } if reason == "stop"
                ))
                .count(),
            1
        );
        assert_eq!(
            terminal
                .iter()
                .filter(|event| matches!(
                    event,
                    StreamEvent::Usage {
                        prompt_tokens: 1,
                        completion_tokens: 2,
                        ..
                    }
                ))
                .count(),
            1
        );
        assert_eq!(terminal.iter().filter(|event| matches!(event,
            StreamEvent::ReasoningSnapshot { thinking_text: Some(text), .. } if text == "plan"
        )).count(), 1);
        assert!(stream.next().await.is_none());
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn malformed_first_sse_frame_is_retryable_transport() {
        let source = tokio_stream::iter(vec![Ok::<Bytes, reqwest::Error>(Bytes::from(
            "event: message_start\ndata: {not-json\n\n",
        ))]);
        let mut stream = AnthropicStream::new(
            source,
            ProviderCompatProfile::anthropic_messages("claude-opus-4-6"),
            true,
        );

        let error = stream
            .next()
            .await
            .expect("malformed first frame must surface")
            .expect_err("first frame cannot parse");
        assert_eq!(
            classify_llm_failure(&error).kind,
            LlmFailureKind::StreamInterrupted,
            "before any valid event, a malformed intermediary frame is transport-retryable"
        );
    }

    #[test]
    fn parse_block_error_uses_error_code_as_reason_label() {
        let mut stream = AnthropicStream::new(
            empty::<Result<Bytes, reqwest::Error>>(),
            ProviderCompatProfile::anthropic_messages("claude-opus-4-6"),
            true,
        );

        let events = stream
            .parse_block(
                "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"server_error\",\"message\":\"boom\"}}\n\n",
            )
            .expect("error event");

        assert!(matches!(
            events.first(),
            Some(StreamEvent::LlmError {
                reason,
                message,
                code: Some(code),
            }) if reason == "error:server_error" && message == "boom" && code == "server_error"
        ));
    }

    #[test]
    fn parse_block_error_without_code_uses_plain_error_label() {
        let mut stream = AnthropicStream::new(
            empty::<Result<Bytes, reqwest::Error>>(),
            ProviderCompatProfile::anthropic_messages("claude-opus-4-6"),
            true,
        );

        let events = stream
            .parse_block(
                "event: error\ndata: {\"type\":\"error\",\"error\":{\"message\":\"boom\"}}\n\n",
            )
            .expect("error event");

        assert!(matches!(
            events.first(),
            Some(StreamEvent::LlmError {
                reason,
                message,
                code: None,
            }) if reason == "error" && message == "boom"
        ));
    }
}
