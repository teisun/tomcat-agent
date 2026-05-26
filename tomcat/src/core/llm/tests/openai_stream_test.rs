//! # OpenAI 流式解析焦小测
//!
//! 覆盖：
//!
//! - `openai_chunk_to_stream_events`：含 `usage` 的 chunk 输出 `StreamEvent::Usage`，
//!   不含 `usage` 时不会生成 Usage 事件。
//! - `SseEventStream`：把多段 `data: {...}\n\n` 流式解析成 `ContentDelta` /
//!   `FinishReason` 等事件序列。

use super::*;
use crate::core::llm::types::{StreamEvent, ThinkingSource};
use crate::infra::error::{llm_stage, llm_summary, AppError, LlmErrorStage};
use bytes::Bytes;
use std::time::Duration;

#[test]
fn test_openai_chunk_with_usage_emits_usage_event() {
    let chunk: OpenAiStreamChunk = serde_json::from_str(
        r#"{"choices":[],"usage":{"prompt_tokens":150,"completion_tokens":42,"total_tokens":192}}"#,
    )
    .expect("should parse chunk with usage");
    let events = openai_chunk_to_stream_events(chunk);
    assert_eq!(events.len(), 1, "should emit exactly one Usage event");
    match &events[0] {
        StreamEvent::Usage {
            prompt_tokens,
            completion_tokens,
            total_tokens,
        } => {
            assert_eq!(*prompt_tokens, 150);
            assert_eq!(*completion_tokens, 42);
            assert_eq!(*total_tokens, Some(192));
        }
        other => panic!("expected StreamEvent::Usage, got {:?}", other),
    }
}

#[test]
fn test_openai_chunk_without_usage_no_usage_event() {
    let chunk: OpenAiStreamChunk =
        serde_json::from_str(r#"{"choices":[{"delta":{"content":"hi"}}]}"#)
            .expect("should parse chunk without usage");
    let events = openai_chunk_to_stream_events(chunk);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, StreamEvent::Usage { .. })),
        "should not contain Usage event when chunk has no usage field"
    );
    assert_eq!(events.len(), 1);
    assert!(matches!(&events[0], StreamEvent::ContentDelta { delta } if delta == "hi"));
}

#[test]
fn test_openai_chunk_reasoning_content_emits_thinking() {
    let chunk: OpenAiStreamChunk =
        serde_json::from_str(r#"{"choices":[{"delta":{"reasoning_content":"step 1"}}]}"#)
            .expect("should parse chunk with reasoning_content");
    let events = openai_chunk_to_stream_events(chunk);
    assert_eq!(events.len(), 1);
    assert!(
        matches!(
            &events[0],
            StreamEvent::Thinking {
                delta,
                source: ThinkingSource::Raw,
                signature: None
            } if delta == "step 1"
        ),
        "got: {:?}",
        events[0]
    );
}

#[test]
fn test_openai_chunk_reasoning_alias_falls_back() {
    let chunk: OpenAiStreamChunk =
        serde_json::from_str(r#"{"choices":[{"delta":{"reasoning":"alt-name"}}]}"#)
            .expect("should parse chunk with reasoning alias");
    let events = openai_chunk_to_stream_events(chunk);
    assert_eq!(events.len(), 1);
    assert!(
        matches!(
            &events[0],
            StreamEvent::Thinking {
                delta,
                source: ThinkingSource::Raw,
                ..
            } if delta == "alt-name"
        ),
        "got: {:?}",
        events[0]
    );
}

#[test]
fn test_openai_chunk_thinking_and_content_order() {
    let chunk: OpenAiStreamChunk = serde_json::from_str(
        r#"{"choices":[{"delta":{"reasoning_content":"plan","content":"answer"}}]}"#,
    )
    .expect("should parse chunk with both reasoning and content");
    let events = openai_chunk_to_stream_events(chunk);
    assert_eq!(events.len(), 2, "thinking + content should both emit");
    assert!(
        matches!(
            &events[0],
            StreamEvent::Thinking {
                delta,
                source: ThinkingSource::Raw,
                ..
            } if delta == "plan"
        ),
        "expected Thinking first; got: {:?}",
        events[0]
    );
    assert!(
        matches!(&events[1], StreamEvent::ContentDelta { delta } if delta == "answer"),
        "expected ContentDelta second; got: {:?}",
        events[1]
    );
}

#[test]
fn test_openai_chunk_empty_reasoning_is_ignored() {
    let chunk: OpenAiStreamChunk =
        serde_json::from_str(r#"{"choices":[{"delta":{"reasoning_content":""}}]}"#)
            .expect("should parse chunk with empty reasoning_content");
    let events = openai_chunk_to_stream_events(chunk);
    assert!(
        events.is_empty(),
        "empty reasoning should not emit Thinking, got {:?}",
        events
    );
}

#[test]
fn test_openai_request_body_does_not_serialize_reasoning_when_none() {
    let body = OpenAiRequestBody {
        model: "gpt-4".into(),
        messages: vec![],
        temperature: None,
        max_tokens: None,
        stream: false,
        tools: None,
        stream_options: None,
        reasoning_effort: None,
        thinking: None,
    };
    let j = serde_json::to_value(&body).unwrap();
    assert!(
        j.get("reasoning_effort").is_none(),
        "None reasoning_effort 不应进 wire JSON: {}",
        j
    );
    assert!(
        j.get("thinking").is_none(),
        "None thinking 不应进 wire JSON: {}",
        j
    );
}

#[test]
fn test_openai_request_body_serializes_reasoning_effort() {
    let body = OpenAiRequestBody {
        model: "gpt-5".into(),
        messages: vec![],
        temperature: None,
        max_tokens: None,
        stream: true,
        tools: None,
        stream_options: None,
        reasoning_effort: Some("high".into()),
        thinking: None,
    };
    let j = serde_json::to_value(&body).unwrap();
    assert_eq!(
        j.get("reasoning_effort").and_then(|v| v.as_str()),
        Some("high")
    );
    assert!(j.get("thinking").is_none());
}

#[test]
fn test_openai_request_body_serializes_thinking_object() {
    let body = OpenAiRequestBody {
        model: "doubao-seed-1.6-thinking".into(),
        messages: vec![],
        temperature: None,
        max_tokens: None,
        stream: true,
        tools: None,
        stream_options: None,
        reasoning_effort: None,
        thinking: Some(serde_json::json!({"type":"enabled"})),
    };
    let j = serde_json::to_value(&body).unwrap();
    assert_eq!(j["thinking"]["type"], "enabled");
    assert!(j.get("reasoning_effort").is_none());
}

#[test]
fn test_openai_provider_disabled_thinking_has_no_reasoning_fields_in_request() {
    use crate::core::llm::thinking_policy::{resolve_request_fields, ThinkingFormat};
    use crate::infra::config::ThinkingConfig;
    let cfg = ThinkingConfig {
        enabled: false,
        ..ThinkingConfig::default()
    };
    let r = resolve_request_fields(&cfg, ThinkingFormat::Openai);
    assert!(
        r.reasoning_effort.is_none(),
        "enabled=false 不应写 reasoning_effort"
    );
    assert!(r.thinking.is_none());
}

#[test]
fn test_openai_provider_thinking_high_writes_reasoning_effort() {
    use crate::core::llm::thinking_policy::{resolve_request_fields, ThinkingFormat};
    use crate::infra::config::ThinkingConfig;
    let cfg = ThinkingConfig {
        enabled: true,
        level: "high".into(),
        ..ThinkingConfig::default()
    };
    let r = resolve_request_fields(&cfg, ThinkingFormat::Openai);
    assert_eq!(r.reasoning_effort.as_deref(), Some("high"));
}

#[tokio::test]
async fn sse_stream_parses_and_yields_events() {
    use tokio_stream::StreamExt;

    let chunks: Vec<Result<Bytes, AppError>> = vec![
        Ok(Bytes::from(
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n",
        )),
        Ok(Bytes::from(
            "data: {\"choices\":[{\"delta\":{\"content\":\" world\"}}]}\n\n",
        )),
        Ok(Bytes::from(
            "data: {\"choices\":[{\"finish_reason\":\"stop\"}]}\n\n",
        )),
    ];
    let stream = tokio_stream::iter(chunks);
    let mut event_stream = SseEventStream::new(stream);
    let mut events = Vec::new();
    while let Some(item) = event_stream.next().await {
        events.push(item);
    }
    assert_eq!(events.len(), 3);
    assert!(matches!(&events[0], Ok(StreamEvent::ContentDelta { delta } ) if delta == "Hello"));
    assert!(matches!(&events[1], Ok(StreamEvent::ContentDelta { delta } ) if delta == " world"));
    assert!(matches!(&events[2], Ok(StreamEvent::FinishReason { reason } ) if reason == "stop"));
}

#[tokio::test(start_paused = true)]
async fn idle_timeout_errors_when_no_bytes_arrive() {
    use tokio_stream::StreamExt;

    let source = tokio_stream::pending::<Result<Bytes, AppError>>();
    let mut stream = apply_stream_idle_timeout(source, 3);
    let next_task = tokio::spawn(async move { stream.next().await });

    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(4)).await;

    let item = next_task
        .await
        .expect("join ok")
        .expect("should produce timeout error");
    match item {
        Err(err) => {
            let msg = llm_summary(&err).unwrap_or_else(|| err.to_string());
            assert_eq!(llm_stage(&err), Some(LlmErrorStage::IdleTimeout));
            assert!(msg.contains("流式空闲超时"), "unexpected msg: {}", msg);
            assert!(
                msg.contains("stream_timeout_sec=3s"),
                "unexpected msg: {}",
                msg
            );
        }
        other => panic!("expected timeout AppError, got {:?}", other),
    }
}

#[tokio::test(start_paused = true)]
async fn keepalive_bytes_do_not_trigger_idle_timeout() {
    use tokio_stream::wrappers::IntervalStream;
    use tokio_stream::StreamExt;

    let interval = tokio::time::interval(Duration::from_millis(200));
    let source = IntervalStream::new(interval)
        .take(3)
        .map(|_| Ok(Bytes::from_static(b": keepalive\n\n")));
    let mut stream = apply_stream_idle_timeout(source, 1);
    let collect_task = tokio::spawn(async move {
        let mut out = Vec::new();
        while let Some(item) = stream.next().await {
            out.push(item);
        }
        out
    });

    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(1)).await;

    let out = collect_task.await.expect("join ok");
    assert_eq!(out.len(), 3);
    assert!(out.into_iter().all(|item| item.is_ok()));
}
