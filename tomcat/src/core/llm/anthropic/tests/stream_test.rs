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
    assert_eq!(
        terminal
            .iter()
            .filter(|event| matches!(event,
                StreamEvent::ReasoningSnapshot { thinking_text: Some(text), .. } if text == "plan"
            ))
            .count(),
        1
    );
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

#[test]
fn partial_usage_frames_preserve_input_and_cache_but_accept_explicit_zero() {
    for clear in [false, true] {
        let mut stream = AnthropicStream::new(
            empty::<Result<Bytes, reqwest::Error>>(),
            ProviderCompatProfile::anthropic_messages("claude-opus-4-6"),
            true,
        );
        stream.parse_block("event: message_start\ndata: {\"message\":{\"usage\":{\"input_tokens\":12,\"output_tokens\":1,\"cache_read_input_tokens\":100,\"cache_creation_input_tokens\":20}}}").unwrap();
        for output in [5, 7] {
            stream
                .parse_block(&format!(
                    "event: message_delta\ndata: {{\"usage\":{{\"output_tokens\":{output}}}}}"
                ))
                .unwrap();
        }
        // Missing/null counters do not overwrite real provider accounting.
        stream
            .parse_block("event: message_delta\ndata: {\"usage\":null}")
            .unwrap();
        stream
            .parse_block("event: message_delta\ndata: {\"usage\":{\"input_tokens\":null}}")
            .unwrap();
        if clear {
            stream.parse_block("event: message_delta\ndata: {\"usage\":{\"input_tokens\":0,\"output_tokens\":0,\"cache_read_input_tokens\":0,\"cache_creation_input_tokens\":0}}").unwrap();
        }
        let terminal = stream.parse_block("event: message_stop\ndata: {}").unwrap();
        let usages: Vec<_> = terminal
            .iter()
            .filter_map(|event| match event {
                StreamEvent::Usage {
                    prompt_tokens,
                    completion_tokens,
                    cache_read_tokens,
                    cache_write_tokens,
                    total_tokens,
                    ..
                } => Some((
                    *prompt_tokens,
                    *completion_tokens,
                    *cache_read_tokens,
                    *cache_write_tokens,
                    *total_tokens,
                )),
                _ => None,
            })
            .collect();
        assert_eq!(
            usages,
            vec![if clear {
                (0, 0, Some(0), Some(0), Some(0))
            } else {
                (132, 7, Some(100), Some(20), Some(139))
            }]
        );
    }
}
