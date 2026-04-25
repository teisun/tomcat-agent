//! # OpenAI 流式解析焦小测
//!
//! 覆盖：
//!
//! - `openai_chunk_to_stream_events`：含 `usage` 的 chunk 输出 `StreamEvent::Usage`，
//!   不含 `usage` 时不会生成 Usage 事件。
//! - `SseEventStream`：把多段 `data: {...}\n\n` 流式解析成 `ContentDelta` /
//!   `FinishReason` 等事件序列。

use super::super::openai::{openai_chunk_to_stream_events, OpenAiStreamChunk, SseEventStream};
use super::super::types::StreamEvent;
use crate::infra::error::AppError;
use bytes::Bytes;

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
