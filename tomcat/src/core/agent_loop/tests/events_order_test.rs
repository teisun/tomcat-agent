//! # 事件顺序与终结类测试（events_order）
//!
//! 本文件聚合"事件序列契约"相关的三个用例：
//!
//! - `run_emits_events_in_correct_order`：纯文本一轮的 wire 事件全序列；
//! - `run_fatal_401_terminates_immediately`：Fatal 立即终止 + agent_end(error);
//! - `run_chat_stream_returns_err_is_classified`：chat_stream 直接返回 Err
//!   仍被 classify_error 正确分流（503 → Retryable，但耗尽 → Fatal 终止）。

use std::sync::Arc;
use std::sync::Mutex;

use tokio_util::sync::CancellationToken;

use crate::core::agent_loop::{AgentLoop, AgentLoopConfig};
use crate::core::llm::{ChatMessage, StreamEvent, ThinkingSource};
use crate::infra::error::{llm_http_status_error, AppError};
use crate::infra::event_bus::EventBus;
use crate::infra::wire;
use crate::infra::{DefaultEventBus, EventContext};

use super::mocks::{MockLlmProvider, MockLlmProviderFatal, MockPrimitiveExecutor};

type MessageUpdateKinds = Vec<(String, Option<String>)>;

/// 事件顺序：纯文本一轮，断言 agent_start -> turn_start -> message_start ->
/// message_update* -> message_end -> turn_end -> agent_end。
#[tokio::test]
async fn run_emits_events_in_correct_order() {
    let stream1: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "x".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let llm = Arc::new(MockLlmProvider::new(vec![stream1]));
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());
    let order: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let expected: Vec<String> = vec![
        wire::WIRE_AGENT_START.into(),
        wire::WIRE_TURN_START.into(),
        wire::WIRE_MESSAGE_START.into(),
        wire::WIRE_MESSAGE_UPDATE.into(),
        wire::WIRE_MESSAGE_END.into(),
        wire::WIRE_TURN_END.into(),
        wire::WIRE_AGENT_END.into(),
    ];
    for ev in &expected {
        let list = Arc::clone(&order);
        let name = ev.clone();
        event_bus.on(
            &name,
            Box::new(move |ctx: EventContext| {
                list.lock().unwrap().push(ctx.event_name.clone());
                Ok(())
            }),
        );
    }
    let config = AgentLoopConfig {
        model: "gpt-4".to_string(),
        session_id: "s1".to_string(),
        ..Default::default()
    };
    let abort = CancellationToken::new();
    let mut loop_ = AgentLoop::new(llm, primitive, event_bus, config, abort);
    let messages = vec![ChatMessage::user("hi")];
    let _ = loop_.run(messages).await.unwrap();
    let observed = order.lock().unwrap().clone();
    assert_eq!(observed, expected);
}

/// Fatal 401：chat_stream 直接返回 Err，run 立即终止并返回含 401 的错误。
#[tokio::test]
async fn run_fatal_401_terminates_immediately() {
    let llm = Arc::new(MockLlmProviderFatal {
        err: Box::new(|| llm_http_status_error("mock", 401, "unauthorized")),
    });
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());
    let config = AgentLoopConfig {
        model: "gpt-4".to_string(),
        session_id: "s1".to_string(),
        ..Default::default()
    };
    let abort = CancellationToken::new();
    let mut loop_ = AgentLoop::new(llm, primitive, event_bus, config, abort);
    let messages = vec![ChatMessage::user("hi")];
    let result = loop_.run(messages).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("401"));
}

/// P3：Thinking 与 ContentDelta 必须分别带 `kind=thinking_delta` / `content_delta`，
/// 保证 CLI/TUI 单订阅者可以分流渲染（折叠/正文/工具）。
#[tokio::test]
async fn run_message_update_carries_kind_for_thinking_and_content() {
    let stream1: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::Thinking {
            delta: "let me think".to_string(),
            source: ThinkingSource::Summary,
            signature: None,
        }),
        Ok(StreamEvent::ContentDelta {
            delta: "hello".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let llm = Arc::new(MockLlmProvider::new(vec![stream1]));
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());
    let events_seen: Arc<Mutex<MessageUpdateKinds>> = Arc::new(Mutex::new(Vec::new()));
    let events_seen_cb = Arc::clone(&events_seen);
    event_bus.on(
        wire::WIRE_MESSAGE_UPDATE,
        Box::new(move |ctx: EventContext| {
            // payload schema: AssistantMessageEvent(serde_json::Value) wrapped in `assistantMessageEvent`
            let p = &ctx.payload;
            let kind = p
                .pointer("/assistantMessageEvent/kind")
                .and_then(|v| v.as_str())
                .unwrap_or("<missing>")
                .to_string();
            let source = p
                .pointer("/assistantMessageEvent/source")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            events_seen_cb.lock().unwrap().push((kind, source));
            Ok(())
        }),
    );
    let config = AgentLoopConfig {
        model: "gpt-4".to_string(),
        session_id: "s1".to_string(),
        ..Default::default()
    };
    let abort = CancellationToken::new();
    let mut loop_ = AgentLoop::new(llm, primitive, event_bus, config, abort);
    let messages = vec![ChatMessage::user("hi")];
    let _ = loop_.run(messages).await.unwrap();
    let observed = events_seen.lock().unwrap().clone();
    assert_eq!(
        observed,
        vec![
            ("thinking_delta".to_string(), Some("summary".to_string())),
            ("content_delta".to_string(), None),
        ],
        "应分别走 thinking_delta/source=summary 与 content_delta，且顺序保持流式到达顺序"
    );
}

/// P3：Thinking 携带 signature 时（Anthropic 协议）必须透传到 payload，
/// 用于多轮重发时按 provider 决定 strip 还是保留。
#[tokio::test]
async fn run_message_update_thinking_signature_propagates() {
    let stream1: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::Thinking {
            delta: "anthropic".to_string(),
            source: ThinkingSource::Raw,
            signature: Some("sig-xyz".to_string()),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let llm = Arc::new(MockLlmProvider::new(vec![stream1]));
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());
    let captured: Arc<Mutex<Option<serde_json::Value>>> = Arc::new(Mutex::new(None));
    let captured_cb = Arc::clone(&captured);
    event_bus.on(
        wire::WIRE_MESSAGE_UPDATE,
        Box::new(move |ctx: EventContext| {
            let p = ctx
                .payload
                .pointer("/assistantMessageEvent")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            *captured_cb.lock().unwrap() = Some(p);
            Ok(())
        }),
    );
    let config = AgentLoopConfig {
        model: "gpt-4".to_string(),
        session_id: "s1".to_string(),
        ..Default::default()
    };
    let abort = CancellationToken::new();
    let mut loop_ = AgentLoop::new(llm, primitive, event_bus, config, abort);
    let messages = vec![ChatMessage::user("hi")];
    let _ = loop_.run(messages).await.unwrap();
    let payload = captured.lock().unwrap().clone().expect("应捕获到 payload");
    assert_eq!(
        payload.get("kind").and_then(|v| v.as_str()),
        Some("thinking_delta")
    );
    assert_eq!(payload.get("source").and_then(|v| v.as_str()), Some("raw"));
    assert_eq!(
        payload.get("signature").and_then(|v| v.as_str()),
        Some("sig-xyz")
    );
}

/// Thinking summary/raw 都必须继续复用 `kind=thinking_delta`，只靠 `source` 区分，
/// 不引入新的 wire event type。
#[tokio::test]
async fn run_message_update_keeps_thinking_wire_shape_for_summary_and_raw() {
    let stream1: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::Thinking {
            delta: "summary".to_string(),
            source: ThinkingSource::Summary,
            signature: None,
        }),
        Ok(StreamEvent::Thinking {
            delta: "raw".to_string(),
            source: ThinkingSource::Raw,
            signature: None,
        }),
        Ok(StreamEvent::ContentDelta {
            delta: "done".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let llm = Arc::new(MockLlmProvider::new(vec![stream1]));
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());
    let events_seen: Arc<Mutex<MessageUpdateKinds>> = Arc::new(Mutex::new(Vec::new()));
    let events_seen_cb = Arc::clone(&events_seen);
    event_bus.on(
        wire::WIRE_MESSAGE_UPDATE,
        Box::new(move |ctx: EventContext| {
            let p = &ctx.payload;
            let kind = p
                .pointer("/assistantMessageEvent/kind")
                .and_then(|v| v.as_str())
                .unwrap_or("<missing>")
                .to_string();
            let source = p
                .pointer("/assistantMessageEvent/source")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            events_seen_cb.lock().unwrap().push((kind, source));
            Ok(())
        }),
    );
    let config = AgentLoopConfig {
        model: "gpt-4".to_string(),
        session_id: "s1".to_string(),
        ..Default::default()
    };
    let abort = CancellationToken::new();
    let mut loop_ = AgentLoop::new(llm, primitive, event_bus, config, abort);
    let messages = vec![ChatMessage::user("hi")];
    let _ = loop_.run(messages).await.unwrap();
    let observed = events_seen.lock().unwrap().clone();
    assert_eq!(
        observed,
        vec![
            ("thinking_delta".to_string(), Some("summary".to_string())),
            ("thinking_delta".to_string(), Some("raw".to_string())),
            ("content_delta".to_string(), None),
        ]
    );
}

/// chat_stream 直接返回 Err（非 stream 内 Err）时也被正确分类并终止。
#[tokio::test]
async fn run_chat_stream_returns_err_is_classified() {
    let llm = Arc::new(MockLlmProviderFatal {
        err: Box::new(|| llm_http_status_error("mock", 503, "service unavailable")),
    });
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());
    let config = AgentLoopConfig {
        model: "gpt-4".to_string(),
        session_id: "s1".to_string(),
        ..Default::default()
    };
    let abort = CancellationToken::new();
    let mut loop_ = AgentLoop::new(llm, primitive, event_bus, config, abort);
    let messages = vec![ChatMessage::user("hi")];
    let result = loop_.run(messages).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("503"));
}
