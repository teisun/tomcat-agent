//! # OpenAI 流式解析焦小测
//!
//! 覆盖：
//!
//! - `openai_chunk_to_stream_events`：含 `usage` 的 chunk 输出 `StreamEvent::Usage`，
//!   不含 `usage` 时不会生成 Usage 事件。
//! - `SseEventStream`：把多段 `data: {...}\n\n` 流式解析成 `ContentDelta` /
//!   `FinishReason` 等事件序列。

use super::*;
use crate::core::llm::provider::LlmProvider;
use crate::core::llm::tests::mocks::{MockHttpServer, ScriptedHttpResponse};
use crate::core::llm::types::{
    ChatMessage, ChatRequest, ContinuityMetadata, ReasoningContinuation, ReasoningFormat,
    ReplayRequirement, StreamEvent, ThinkingSource,
};
use crate::infra::error::{llm_http_status, llm_stage, llm_summary, AppError, LlmErrorStage};
use crate::infra::LlmConfig;
use bytes::Bytes;
use std::time::Duration;

const STREAM_TEST_KEY_ENV: &str = "__OPENAI_STREAM_TEST_KEY__";

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
    // chat-completions 单一 reasoning 流归类为 Summary（无独立 summary/raw 双流），
    // 以便默认 `show="summary"` 档位可显示。
    assert!(
        matches!(
            &events[0],
            StreamEvent::Thinking {
                delta,
                source: ThinkingSource::Summary,
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
                source: ThinkingSource::Summary,
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
                source: ThinkingSource::Summary,
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

/// 回归：chat-completions 类 provider 的 reasoning 单流必须发成 `ThinkingSource::Summary`。
///
/// 这些模型（deepseek/mimo/doubao 等）没有 OpenAI Responses 的独立 summary/raw 双流，
/// 其唯一 reasoning 面应在默认 `show="summary"` 档位可见；若退回 `Raw` 会被
/// `CliTurnRenderer` 的 raw 过滤吞掉、导致 thinking UI 空白（本次修复锁死的契约）。
#[test]
fn test_chat_completions_reasoning_classified_as_summary_source() {
    for field in ["reasoning_content", "reasoning", "reasoning_text"] {
        let chunk: OpenAiStreamChunk = serde_json::from_str(&format!(
            r#"{{"choices":[{{"delta":{{"{field}":"thinking"}}}}]}}"#
        ))
        .expect("should parse reasoning chunk");
        let events = openai_chunk_to_stream_events(chunk);
        assert!(
            matches!(
                events.first(),
                Some(StreamEvent::Thinking {
                    source: ThinkingSource::Summary,
                    ..
                })
            ),
            "字段 {field} 的 reasoning 应归类为 Summary，实际: {:?}",
            events
        );
    }
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
fn test_openai_request_body_serializes_deepseek_thinking_fields_together() {
    let body = OpenAiRequestBody {
        model: "deepseek-v4-pro".into(),
        messages: vec![],
        temperature: None,
        max_tokens: None,
        stream: true,
        tools: None,
        stream_options: None,
        reasoning_effort: Some("high".into()),
        thinking: Some(serde_json::json!({"type":"enabled"})),
    };
    let j = serde_json::to_value(&body).unwrap();
    assert_eq!(
        j.get("reasoning_effort").and_then(|v| v.as_str()),
        Some("high")
    );
    assert_eq!(j["thinking"]["type"], "enabled");
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
    let mut event_stream = SseEventStream::new(
        stream,
        ProviderCompatProfile::chat_completions("gpt-5"),
        true,
    );
    let mut events = Vec::new();
    while let Some(item) = event_stream.next().await {
        events.push(item);
    }
    assert_eq!(events.len(), 3);
    assert!(matches!(&events[0], Ok(StreamEvent::ContentDelta { delta } ) if delta == "Hello"));
    assert!(matches!(&events[1], Ok(StreamEvent::ContentDelta { delta } ) if delta == " world"));
    assert!(matches!(&events[2], Ok(StreamEvent::FinishReason { reason } ) if reason == "stop"));
}

#[test]
fn test_openai_chunk_deepseek_finish_emits_reasoning_snapshot() {
    let mut state = OpenAiReasoningState {
        source_profile: ProviderCompatProfile::chat_completions("deepseek-v4-pro"),
        continuity_enabled: true,
        ..OpenAiReasoningState::default()
    };
    let chunk: OpenAiStreamChunk = serde_json::from_str(
        r#"{"choices":[{"delta":{"reasoning_content":"step 1","tool_calls":[{"index":0,"id":"call_1","function":{"name":"read","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#,
    )
    .expect("parse deepseek chunk");
    let events = openai_chunk_to_stream_events_with_state(chunk, &mut state);
    assert!(events.iter().any(|event| matches!(
        event,
        StreamEvent::ReasoningSnapshot {
            thinking_text: Some(text),
            reasoning_continuation: Some(continuation),
            continuity: Some(continuity)
        } if text == "step 1"
            && continuity.had_tool_call
            && continuity.replay_requirement == ReplayRequirement::SameProfileRequired
            && continuation.opaque_payload["reasoning_content"] == serde_json::json!("step 1")
    )));
}

#[test]
fn test_openai_chunk_deepseek_tool_turn_without_reasoning_does_not_emit_snapshot() {
    let mut state = OpenAiReasoningState {
        source_profile: ProviderCompatProfile::chat_completions("deepseek-v4-pro"),
        continuity_enabled: true,
        ..OpenAiReasoningState::default()
    };
    let chunk: OpenAiStreamChunk = serde_json::from_str(
        r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"read","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#,
    )
    .expect("parse deepseek chunk without reasoning");
    let events = openai_chunk_to_stream_events_with_state(chunk, &mut state);
    assert!(
        events
            .iter()
            .all(|event| !matches!(event, StreamEvent::ReasoningSnapshot { .. })),
        "非思考模式或无 reasoning_content 的 tool turn 不应伪造 continuity snapshot: {:?}",
        events
    );
    assert!(events.iter().any(|event| matches!(
        event,
        StreamEvent::ToolCallDelta {
            index: 0,
            name: Some(name),
            ..
        } if name == "read"
    )));
}

#[test]
fn deepseek_tool_turn_replays_reasoning_content() {
    let message = ChatMessage::assistant_with_tool_calls(
        None,
        vec![serde_json::json!({
            "id":"call_1",
            "type":"function",
            "function":{"name":"read","arguments":"{}"}
        })],
    )
    .with_reasoning_state(
        Some("safe summary".to_string()),
        Some(ReasoningContinuation {
            source_provider: "deepseek".to_string(),
            source_api: "chat_completions".to_string(),
            source_model: "deepseek-v4-pro".to_string(),
            format: ReasoningFormat::DeepseekReasoningContent,
            opaque_payload: serde_json::json!({"reasoning_content":"internal plan"}),
            fallback_text: Some("safe summary".to_string()),
            provider_refs: None,
        }),
        Some(ContinuityMetadata {
            had_tool_call: true,
            replay_requirement: ReplayRequirement::SameProfileRequired,
        }),
    );
    let wire = transport_messages(&[message], "deepseek-v4-flash", true);
    assert_eq!(wire[0]["reasoning_content"], "internal plan");
}

#[test]
fn test_transport_messages_deepseek_non_tool_turn_replays_reasoning_content() {
    let message = ChatMessage::assistant("answer").with_reasoning_state(
        Some("safe summary".to_string()),
        Some(ReasoningContinuation {
            source_provider: "deepseek".to_string(),
            source_api: "chat_completions".to_string(),
            source_model: "deepseek-v4-pro".to_string(),
            format: ReasoningFormat::DeepseekReasoningContent,
            opaque_payload: serde_json::json!({"reasoning_content":"internal plan"}),
            fallback_text: Some("safe summary".to_string()),
            provider_refs: None,
        }),
        Some(ContinuityMetadata {
            had_tool_call: false,
            replay_requirement: ReplayRequirement::SameProfileOptional,
        }),
    );
    let wire = transport_messages(&[message], "deepseek-v4-pro", true);
    assert_eq!(wire[0]["reasoning_content"], "internal plan");
}

#[test]
fn test_transport_messages_deepseek_tool_turn_without_reasoning_state_keeps_plain_tool_call() {
    let message = ChatMessage::assistant_with_tool_calls(
        Some("calling tool"),
        vec![serde_json::json!({
            "id":"call_1",
            "type":"function",
            "function":{"name":"read","arguments":"{}"}
        })],
    );
    let wire = transport_messages(&[message], "deepseek-v4-pro", true);
    assert_eq!(wire[0]["content"], "calling tool");
    assert_eq!(wire[0]["tool_calls"][0]["id"], "call_1");
    assert!(wire[0].get("reasoning_content").is_none());
}

#[test]
fn test_transport_messages_deepseek_post_tool_final_assistant_replays_reasoning_content() {
    let tool_turn = ChatMessage::assistant_with_tool_calls(
        None,
        vec![serde_json::json!({
            "id":"call_1",
            "type":"function",
            "function":{"name":"read","arguments":"{}"}
        })],
    )
    .with_reasoning_state(
        Some("tool summary".to_string()),
        Some(ReasoningContinuation {
            source_provider: "deepseek".to_string(),
            source_api: "chat_completions".to_string(),
            source_model: "deepseek-v4-pro".to_string(),
            format: ReasoningFormat::DeepseekReasoningContent,
            opaque_payload: serde_json::json!({"reasoning_content":"tool plan"}),
            fallback_text: Some("tool summary".to_string()),
            provider_refs: None,
        }),
        Some(ContinuityMetadata {
            had_tool_call: true,
            replay_requirement: ReplayRequirement::SameProfileRequired,
        }),
    );
    let final_assistant = ChatMessage::assistant("done").with_reasoning_state(
        Some("final summary".to_string()),
        Some(ReasoningContinuation {
            source_provider: "deepseek".to_string(),
            source_api: "chat_completions".to_string(),
            source_model: "deepseek-v4-pro".to_string(),
            format: ReasoningFormat::DeepseekReasoningContent,
            opaque_payload: serde_json::json!({"reasoning_content":"final plan"}),
            fallback_text: Some("final summary".to_string()),
            provider_refs: None,
        }),
        Some(ContinuityMetadata {
            had_tool_call: false,
            replay_requirement: ReplayRequirement::SameProfileOptional,
        }),
    );

    let wire = transport_messages(
        &[
            ChatMessage::user("lookup weather"),
            tool_turn,
            ChatMessage::tool("call_1", r#"{"forecast":"sunny"}"#),
            ChatMessage::user("answer directly"),
            final_assistant,
        ],
        "deepseek-v4-flash",
        true,
    );

    // 历史 tool turn（wire[1]）在「answer directly」这条真实 user 之前，落在可 replay 窗口外，
    // 按续传文档 §4.2.6 与 replay_window_strips_older_history_but_keeps_latest_assistant 一致：
    // 一律静默 StripOpaque——丢弃隐藏 reasoning_content，只保留可见 tool_call。
    assert!(wire[1].get("reasoning_content").is_none());
    assert_eq!(wire[1]["tool_calls"][0]["id"], "call_1");
    // 最新 assistant turn（wire[4]）在窗口内，同 profile → 高保真回放 reasoning_content。
    assert_eq!(wire[4]["reasoning_content"], "final plan");
}

#[test]
fn test_openai_chunk_mimo_finish_emits_reasoning_snapshot() {
    // MiMo 复用同一条 reasoning_content 抓取路径：profile 标了 ReasoningContent 即抓 snapshot，
    // 不再按厂商名硬编码（snapshot 的 source_provider 来自 profile = mimo）。
    let mut state = OpenAiReasoningState {
        source_profile: ProviderCompatProfile::chat_completions("mimo-v2.5-pro"),
        continuity_enabled: true,
        ..OpenAiReasoningState::default()
    };
    let chunk: OpenAiStreamChunk = serde_json::from_str(
        r#"{"choices":[{"delta":{"reasoning_content":"mimo step","tool_calls":[{"index":0,"id":"call_1","function":{"name":"read","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#,
    )
    .expect("parse mimo chunk");
    let events = openai_chunk_to_stream_events_with_state(chunk, &mut state);
    assert!(events.iter().any(|event| matches!(
        event,
        StreamEvent::ReasoningSnapshot {
            thinking_text: Some(text),
            reasoning_continuation: Some(continuation),
            continuity: Some(continuity)
        } if text == "mimo step"
            && continuity.had_tool_call
            && continuation.source_provider == "mimo"
            && continuation.opaque_payload["reasoning_content"] == serde_json::json!("mimo step")
    )));
}

#[test]
fn mimo_tool_turn_replays_reasoning_content() {
    let message = ChatMessage::assistant_with_tool_calls(
        None,
        vec![serde_json::json!({
            "id":"call_1",
            "type":"function",
            "function":{"name":"read","arguments":"{}"}
        })],
    )
    .with_reasoning_state(
        Some("mimo summary".to_string()),
        Some(ReasoningContinuation {
            source_provider: "mimo".to_string(),
            source_api: "chat_completions".to_string(),
            source_model: "mimo-v2.5-pro".to_string(),
            format: ReasoningFormat::DeepseekReasoningContent,
            opaque_payload: serde_json::json!({"reasoning_content":"mimo plan"}),
            fallback_text: Some("mimo summary".to_string()),
            provider_refs: None,
        }),
        Some(ContinuityMetadata {
            had_tool_call: true,
            replay_requirement: ReplayRequirement::SameProfileRequired,
        }),
    );
    let wire = transport_messages(&[message], "mimo-v2.5-pro", true);
    assert_eq!(wire[0]["reasoning_content"], "mimo plan");
}

#[tokio::test]
async fn streaming_think_scrubber_hides_split_tags() {
    use tokio_stream::StreamExt;

    let chunks: Vec<Result<Bytes, AppError>> = vec![
        Ok(Bytes::from(
            "data: {\"choices\":[{\"delta\":{\"content\":\"<th\"}}]}\n\n",
        )),
        Ok(Bytes::from(
            "data: {\"choices\":[{\"delta\":{\"content\":\"ink>pla\"}}]}\n\n",
        )),
        Ok(Bytes::from(
            "data: {\"choices\":[{\"delta\":{\"content\":\"n</thi\"}}]}\n\n",
        )),
        Ok(Bytes::from(
            "data: {\"choices\":[{\"delta\":{\"content\":\"nk>Final answer\"}}]}\n\n",
        )),
        Ok(Bytes::from(
            "data: {\"choices\":[{\"finish_reason\":\"stop\"}]}\n\n",
        )),
    ];
    let stream = tokio_stream::iter(chunks);
    let mut event_stream = SseEventStream::new(
        stream,
        ProviderCompatProfile::chat_completions("deepseek-v4-pro"),
        true,
    );
    let mut visible = String::new();
    let mut thinking = String::new();
    let mut events = Vec::new();

    while let Some(item) = event_stream.next().await {
        let event = item.expect("stream event");
        match &event {
            StreamEvent::ContentDelta { delta } => visible.push_str(delta),
            StreamEvent::Thinking { delta, .. } => thinking.push_str(delta),
            _ => {}
        }
        events.push(event);
    }

    assert_eq!(visible, "Final answer");
    assert_eq!(thinking, "plan");
    assert!(events.iter().any(|event| matches!(
        event,
        StreamEvent::ReasoningSnapshot {
            thinking_text: Some(text),
            ..
        } if text == "plan"
    )));
    assert!(events.iter().all(|event| !matches!(
        event,
        StreamEvent::ContentDelta { delta }
            if delta.contains("<think")
                || delta.contains("</think")
                || delta.contains("plan")
    )));
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

fn stream_test_provider(
    base_url: String,
    api_base_fallback: Option<String>,
    retry_count: u32,
) -> OpenAiProvider {
    // SAFETY: 单测内短生命周期环境变量；每个用例独立设置/清理。
    unsafe { std::env::set_var(STREAM_TEST_KEY_ENV, "stub-key") };
    let cfg = LlmConfig {
        api_key_env: Some(STREAM_TEST_KEY_ENV.to_string()),
        api_base: Some(base_url),
        api_base_fallback,
        retry_count,
        stream_timeout_sec: 0,
        ..LlmConfig::default()
    };
    let mut provider = OpenAiProvider::new(&cfg).expect("应该能构造 openai provider");
    // SAFETY: 避免污染其它测试。
    unsafe { std::env::remove_var(STREAM_TEST_KEY_ENV) };
    provider.client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("build no-proxy reqwest client");
    provider
}

fn stream_test_request() -> ChatRequest {
    ChatRequest {
        messages: vec![ChatMessage::user("hi")],
        model: "gpt-4.1".to_string(),
        temperature: Some(0.0),
        max_tokens: Some(16),
        stream: Some(true),
        model_override: None,
        tools: None,
    }
}

fn sse_body(events: &[&str]) -> String {
    let mut body = String::new();
    for event in events {
        body.push_str("data: ");
        body.push_str(event);
        body.push_str("\n\n");
    }
    body.push_str("data: [DONE]\n\n");
    body
}

#[tokio::test]
async fn stream_post_once_gateway_503_sets_connect_stage() {
    let server = MockHttpServer::start(vec![ScriptedHttpResponse::json(
        503,
        r#"{"error":"upstream connect error or disconnect/reset before headers. reset reason: connection timeout"}"#,
    )])
    .await;
    let provider = stream_test_provider(server.base_url.clone(), None, 0);
    let body = OpenAiRequestBody {
        model: "gpt-4.1".to_string(),
        messages: transport_messages(&stream_test_request().messages, "gpt-4.1", true),
        temperature: Some(0.0),
        max_tokens: Some(16),
        stream: true,
        tools: None,
        stream_options: Some(StreamOptionsBody {
            include_usage: true,
        }),
        reasoning_effort: None,
        thinking: None,
    };
    let err = provider
        .stream_post_once(&server.base_url, &body)
        .await
        .expect_err("503 网关错误应直接返回");
    assert_eq!(llm_http_status(&err), Some(503));
    assert_eq!(llm_stage(&err), Some(LlmErrorStage::Connect));
    server.shutdown().await;
}

#[tokio::test]
async fn stream_post_once_header_read_timeout_maps_to_retryable_read_timeout() {
    let mut delayed = ScriptedHttpResponse::json(
        200,
        r#"{"id":"chatcmpl_timeout","choices":[{"message":{"role":"assistant","content":"ok"},"finish_reason":"stop","index":0}]}"#,
    );
    delayed.delay_ms = 1_100;
    let server = MockHttpServer::start(vec![delayed]).await;
    let mut provider = stream_test_provider(server.base_url.clone(), None, 0);
    provider.http_read_timeout_sec = 1;
    provider.client = reqwest::Client::builder()
        .no_proxy()
        .read_timeout(Duration::from_secs(1))
        .build()
        .expect("build read-timeout reqwest client");
    let body = OpenAiRequestBody {
        model: "gpt-4.1".to_string(),
        messages: transport_messages(&stream_test_request().messages, "gpt-4.1", true),
        temperature: Some(0.0),
        max_tokens: Some(16),
        stream: true,
        tools: None,
        stream_options: Some(StreamOptionsBody {
            include_usage: true,
        }),
        reasoning_effort: None,
        thinking: None,
    };
    let err = provider
        .stream_post_once(&server.base_url, &body)
        .await
        .expect_err("响应头迟迟不来时应命中读超时");
    assert_eq!(llm_stage(&err), Some(LlmErrorStage::ReadTimeout));
    assert!(OpenAiProvider::is_retriable(&err));
    let msg = llm_summary(&err).unwrap_or_else(|| err.to_string());
    assert!(
        msg.contains("等待响应头"),
        "错误文案应说明卡在响应头阶段，实际: {}",
        msg
    );
    assert!(
        msg.contains("http_read_timeout_sec=1s"),
        "错误文案应带 read timeout 配置，实际: {}",
        msg
    );
    assert!(
        !msg.contains("1800"),
        "短超时不应再冒名为 1800s 总超时，实际: {}",
        msg
    );
    server.shutdown().await;
}

#[tokio::test]
async fn chat_stream_retries_503_before_first_delta_and_succeeds() {
    use tokio_stream::StreamExt;

    let server = MockHttpServer::start(vec![
        ScriptedHttpResponse::json(
            503,
            r#"{"error":"upstream connect error or disconnect/reset before headers. reset reason: connection timeout"}"#,
        ),
        ScriptedHttpResponse {
            status: 200,
            headers: vec![("Content-Type".to_string(), "text/event-stream".to_string())],
            body: sse_body(&[
                r#"{"choices":[{"delta":{"content":"OK"}}]}"#,
                r#"{"choices":[{"finish_reason":"stop"}]}"#,
            ]),
            delay_ms: 0,
            declared_content_length: None,
        },
    ])
    .await;
    let provider = stream_test_provider(server.base_url.clone(), None, 1);
    let mut stream = provider
        .chat_stream(stream_test_request())
        .await
        .expect("503 后应自动重试成功");
    let mut out = Vec::new();
    while let Some(item) = stream.next().await {
        out.push(item.expect("stream item should be ok"));
    }
    let text: String = out
        .iter()
        .filter_map(|evt| match evt {
            StreamEvent::ContentDelta { delta } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(server.request_count(), 2, "建连阶段应重试一次");
    assert_eq!(text, "OK", "重试后应只输出一份内容");
    server.shutdown().await;
}

#[tokio::test]
async fn chat_stream_retry_exhaustion_returns_structured_503() {
    let server = MockHttpServer::start(vec![
        ScriptedHttpResponse::json(
            503,
            r#"{"error":"upstream connect error or disconnect/reset before headers. reset reason: connection timeout"}"#,
        ),
        ScriptedHttpResponse::json(
            503,
            r#"{"error":"upstream connect error or disconnect/reset before headers. reset reason: connection timeout"}"#,
        ),
    ])
    .await;
    let provider = stream_test_provider(server.base_url.clone(), None, 1);
    let err = match provider.chat_stream(stream_test_request()).await {
        Ok(_) => panic!("503 重试耗尽应返回错误"),
        Err(err) => err,
    };
    assert_eq!(llm_http_status(&err), Some(503));
    assert_eq!(llm_stage(&err), Some(LlmErrorStage::Connect));
    assert_eq!(server.request_count(), 2);
    server.shutdown().await;
}

#[tokio::test]
async fn chat_stream_non_retryable_401_returns_immediately() {
    let server = MockHttpServer::start(vec![ScriptedHttpResponse::json(
        401,
        r#"{"error":"unauthorized"}"#,
    )])
    .await;
    let provider = stream_test_provider(server.base_url.clone(), None, 2);
    let err = match provider.chat_stream(stream_test_request()).await {
        Ok(_) => panic!("401 不应重试"),
        Err(err) => err,
    };
    assert_eq!(llm_http_status(&err), Some(401));
    assert_eq!(server.request_count(), 1);
    server.shutdown().await;
}

#[tokio::test]
async fn chat_stream_after_first_delta_body_read_error_is_not_retried() {
    use tokio_stream::StreamExt;

    let body = "data: {\"choices\":[{\"delta\":{\"content\":\"OK\"}}]}\n\n".to_string();
    let server = MockHttpServer::start(vec![ScriptedHttpResponse {
        status: 200,
        headers: vec![("Content-Type".to_string(), "text/event-stream".to_string())],
        body,
        delay_ms: 0,
        declared_content_length: None,
    }
    .with_declared_content_length(256)])
    .await;
    let provider = stream_test_provider(server.base_url.clone(), None, 2);
    let mut stream = provider
        .chat_stream(stream_test_request())
        .await
        .expect("首帧出 delta 后的断流应在消费阶段上抛");

    match stream.next().await {
        Some(Ok(StreamEvent::ContentDelta { delta })) => assert_eq!(delta, "OK"),
        other => panic!("首帧应先拿到 content delta，实际: {:?}", other),
    }

    let err = match stream.next().await {
        Some(Err(err)) => err,
        other => panic!("断流后应上抛错误且不重试，实际: {:?}", other),
    };
    assert_eq!(llm_stage(&err), Some(LlmErrorStage::BodyRead));
    assert!(
        llm_summary(&err)
            .unwrap_or_else(|| err.to_string())
            .contains("流读取"),
        "错误摘要应保留流读取失败语义，实际: {}",
        err
    );
    assert_eq!(server.request_count(), 1, "首个 delta 后不应重新建连");
    server.shutdown().await;
}

#[tokio::test]
async fn chat_stream_fallback_after_gateway_503_uses_secondary_base() {
    use tokio_stream::StreamExt;

    let primary = MockHttpServer::start(vec![ScriptedHttpResponse::json(
        503,
        r#"{"error":"upstream connect error or disconnect/reset before headers. reset reason: connection timeout"}"#,
    )])
    .await;
    let fallback = MockHttpServer::start(vec![ScriptedHttpResponse {
        status: 200,
        headers: vec![("Content-Type".to_string(), "text/event-stream".to_string())],
        body: sse_body(&[
            r#"{"choices":[{"delta":{"content":"fallback ok"}}]}"#,
            r#"{"choices":[{"finish_reason":"stop"}]}"#,
        ]),
        delay_ms: 0,
        declared_content_length: None,
    }])
    .await;
    let provider =
        stream_test_provider(primary.base_url.clone(), Some(fallback.base_url.clone()), 0);
    let mut stream = provider
        .chat_stream(stream_test_request())
        .await
        .expect("fallback 应接管成功");
    let mut text = String::new();
    while let Some(item) = stream.next().await {
        if let StreamEvent::ContentDelta { delta } = item.expect("ok") {
            text.push_str(&delta);
        }
    }
    assert_eq!(primary.request_count(), 1);
    assert_eq!(fallback.request_count(), 1);
    assert_eq!(text, "fallback ok");
    primary.shutdown().await;
    fallback.shutdown().await;
}
