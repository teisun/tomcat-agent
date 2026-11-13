use super::*;
use bytes::Bytes;
use tokio_stream::StreamExt;

fn test_profile() -> ProviderCompatProfile {
    ProviderCompatProfile::google_gemini_routed(
        "gemini-4-carbon",
        "fcodex",
        "https://hk.fcodex.top/v1beta",
        "cred_1234567890abcdef",
    )
}

async fn collect_sse(raw_sse: &[u8]) -> Result<Vec<StreamEvent>, AppError> {
    let chunks: Vec<Result<Bytes, reqwest::Error>> = raw_sse
        .chunks(97)
        .map(|slice| Ok(Bytes::copy_from_slice(slice)))
        .collect();
    let stream = GeminiStream::new(tokio_stream::iter(chunks), test_profile(), true);
    tokio::pin!(stream);
    let mut out = Vec::new();
    while let Some(item) = stream.next().await {
        out.push(item?);
    }
    Ok(out)
}

#[tokio::test]
async fn replays_recorded_parallel_tool_calls_fixture_with_crlf_and_lf() {
    let lf_fixture = include_str!(
        "../../../../../tests/fixtures/llm_stream/gemini_native_parallel_tool_calls.sse"
    );
    let crlf_fixture = lf_fixture.replace('\n', "\r\n");

    for payload in [lf_fixture.as_bytes(), crlf_fixture.as_bytes()] {
        let events = collect_sse(payload).await.expect("valid SSE stream");
        let calls: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                StreamEvent::ToolCallDelta {
                    index,
                    id,
                    name,
                    arguments_delta,
                } => Some((
                    *index,
                    id.clone().unwrap(),
                    name.clone().unwrap(),
                    arguments_delta.clone().unwrap(),
                )),
                _ => None,
            })
            .collect();
        assert_eq!(
            calls,
            vec![
                (
                    0,
                    "call_12516051982034679107".to_string(),
                    "list_dir".to_string(),
                    r#"{"path":"."}"#.to_string(),
                ),
                (
                    1,
                    "call_12516051982034680294".to_string(),
                    "list_dir".to_string(),
                    r#"{"path":"tomcat"}"#.to_string(),
                ),
            ]
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, StreamEvent::ContentDelta { .. })),
            "empty text in STOP frame must not emit ContentDelta"
        );

        let snapshot = events
            .iter()
            .find_map(|e| match e {
                StreamEvent::ReasoningSnapshot {
                    reasoning_continuation,
                    continuity,
                    ..
                } => Some((
                    reasoning_continuation.clone().unwrap(),
                    continuity.clone().unwrap(),
                )),
                _ => None,
            })
            .expect("ReasoningSnapshot emitted");
        assert_eq!(snapshot.0.format, ReasoningFormat::GeminiThoughtSignatures);
        assert!(snapshot.0.opaque_payload[0]["thoughtSignature"]
            .as_str()
            .unwrap()
            .starts_with("AY89a1/"));
        assert!(snapshot.0.opaque_payload[1]
            .get("thoughtSignature")
            .is_none());
        assert_eq!(
            snapshot
                .0
                .provider_refs
                .as_ref()
                .and_then(|r| r.replay_profile_id.as_deref()),
            Some(test_profile().profile_id.as_str())
        );
        assert!(snapshot.1.had_tool_call);
        assert_eq!(
            snapshot.1.replay_requirement,
            crate::core::llm::ReplayRequirement::SameProfileRequired
        );

        assert!(events.iter().any(|e| matches!(
            e,
            StreamEvent::Usage {
                prompt_tokens: 9276,
                completion_tokens: 40,
                text_tokens: Some(40),
                reasoning_tokens: None,
                total_tokens: Some(9316),
                ..
            }
        )));
        assert!(events.iter().any(|e| matches!(
            e,
            StreamEvent::FinishReason { reason } if reason == "tool_calls"
        )));
    }
}

#[tokio::test]
async fn replays_recorded_thinking_and_tool_call_fixture() {
    let fixture = include_bytes!(
        "../../../../../tests/fixtures/llm_stream/gemini_native_thinking_tool_call.sse"
    );
    let events = collect_sse(fixture).await.expect("valid SSE stream");

    assert!(events.iter().any(|e| matches!(
        e,
        StreamEvent::Thinking {
            source: ThinkingSource::Summary,
            delta,
            ..
        } if delta.contains("three-box logic puzzle")
    )));
    assert!(events.iter().any(|e| matches!(
        e,
        StreamEvent::Usage {
            prompt_tokens: 187,
            completion_tokens: 440,
            text_tokens: Some(24),
            reasoning_tokens: Some(416),
            total_tokens: Some(627),
            ..
        }
    )));
}

#[tokio::test]
async fn rejects_repeated_upstream_id_without_publishing_any_call() {
    let sse = concat!(
        "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"functionCall\":{\"id\":\"dup_1\",\"name\":\"write\",\"args\":{\"path\":\"a.txt\",\"content\":\"draft\"}},\"thoughtSignature\":\"SIG_DUP\"}]}}],\"usageMetadata\":{\"promptTokenCount\":10,\"toolUsePromptTokenCount\":5,\"cachedContentTokenCount\":4,\"candidatesTokenCount\":2,\"thoughtsTokenCount\":3}}\n\n",
        "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"functionCall\":{\"id\":\"dup_1\",\"name\":\"write\",\"args\":{\"path\":\"a.txt\",\"content\":\"final\"}}}]},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"trafficType\":\"ON_DEMAND\"}}\n\n"
    );
    let events = collect_sse(sse.as_bytes()).await.unwrap();
    let calls: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::ToolCallDelta {
                id,
                arguments_delta,
                ..
            } => Some((id.clone().unwrap(), arguments_delta.clone().unwrap())),
            _ => None,
        })
        .collect();
    assert!(calls.is_empty(), "duplicate IDs must not publish calls");
    assert!(events.iter().any(|e| matches!(e,
        StreamEvent::LlmError { code: Some(code), .. } if code == "upstream_error"
    )));

    // Trailing `trafficType`-only frame must not wipe out the real token counts from frame 0.
    assert!(events.iter().any(|e| matches!(
        e,
        StreamEvent::Usage {
            prompt_tokens: 15,
            cache_read_tokens: Some(4),
            completion_tokens: 5,
            text_tokens: Some(2),
            reasoning_tokens: Some(3),
            ..
        }
    )));
}

#[tokio::test]
async fn maps_max_tokens_safety_malformed_call_and_truncated_stream() {
    let max_sse = "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"cut\"}]},\"finishReason\":\"MAX_TOKENS\"}],\"usageMetadata\":{\"promptTokenCount\":5,\"candidatesTokenCount\":10}}\n\n";
    let max_events = collect_sse(max_sse.as_bytes()).await.unwrap();
    assert!(max_events.iter().any(|e| matches!(
        e,
        StreamEvent::LlmNotice { finish_reason, .. } if finish_reason == "max_tokens"
    )));
    assert!(max_events.iter().any(|e| matches!(
        e,
        StreamEvent::FinishReason { reason } if reason == "max_tokens"
    )));

    for safety_reason in [
        "SAFETY",
        "PROHIBITED_CONTENT",
        "BLOCKLIST",
        "SPII",
        "RECITATION",
    ] {
        let sse =
            format!("data: {{\"candidates\":[{{\"finishReason\":\"{safety_reason}\"}}]}}\n\n");
        let events = collect_sse(sse.as_bytes()).await.unwrap();
        assert!(events.iter().any(|e| matches!(
            e,
            StreamEvent::LlmError { code: Some(code), .. } if code == "content_filter"
        )));
    }

    let blocked_prompt = "data: {\"promptFeedback\":{\"blockReason\":\"PROHIBITED_CONTENT\"}}\n\n";
    let blocked_events = collect_sse(blocked_prompt.as_bytes()).await.unwrap();
    assert!(blocked_events.iter().any(|e| matches!(
        e,
        StreamEvent::LlmError { code: Some(code), .. } if code == "content_filter"
    )));

    for retryable_reason in ["MALFORMED_FUNCTION_CALL", "UNEXPECTED_TOOL_CALL"] {
        let sse =
            format!("data: {{\"candidates\":[{{\"finishReason\":\"{retryable_reason}\"}}]}}\n\n");
        let events = collect_sse(sse.as_bytes()).await.unwrap();
        assert!(events.iter().any(|e| matches!(
            e,
            StreamEvent::LlmError { code: Some(code), .. } if code == "upstream_error"
        )));
    }

    let truncated = "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"half\"}]}}]}\n\n";
    let err = collect_sse(truncated.as_bytes())
        .await
        .expect_err("EOF before finishReason must fail as interrupted stream");
    assert!(crate::infra::error::is_retryable_llm_error(&err));
}

#[tokio::test]
async fn done_marker_does_not_bypass_missing_finish_reason() {
    let sse = concat!(
        "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"half\"}]}}]}\n\n",
        "data: [DONE]\n\n"
    );
    let err = collect_sse(sse.as_bytes())
        .await
        .expect_err("[DONE] must not turn an incomplete stream into success");
    assert!(crate::infra::error::is_retryable_llm_error(&err));
    assert!(err
        .to_string()
        .contains("stream closed before finishReason"));
}

#[test]
fn non_streaming_missing_finish_reason_is_retryable_error() {
    let mut accumulator = GeminiResponseAccumulator::new(test_profile(), true);
    accumulator.feed_frame(&json!({"candidates": [{"content": {"parts": [{"text": "half"}]}}]}));
    assert!(matches!(
        &accumulator.build_terminal_events()[0],
        StreamEvent::LlmError { reason, code: Some(code), .. }
            if reason == "finish_reason:missing" && code == "upstream_error"
    ));
    let err = accumulator
        .into_chat_response()
        .expect_err("missing finishReason must fail");
    assert!(crate::infra::error::is_retryable_llm_error(&err));
}
