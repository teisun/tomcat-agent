//! # ContextMetricsUpdate 五个用例
//!
//! 覆盖 timing ⑤ + turn_index==1 两个发射点的所有等价类：
//!
//! 1. `*_emitted_before_turn_end`：metrics 必须在 turn_end 之前；
//! 2. `*_payload_contains_valid_values`：payload 字段类型 / 取值范围；
//! 3. `*_compaction_count_accumulates_across_rounds`：多轮 tool 时 compaction_count 单调不减；
//! 4. `*_skipped_when_no_context_state`：无 context_state 时不发射；
//! 5. `*_emitted_on_text_only_response`：纯文本路径也至少发射一次（首请求前），且仍在 turn_end 之前。

use std::sync::Arc;
use std::sync::Mutex;

use tokio_util::sync::CancellationToken;

use crate::core::agent_loop::{AgentLoop, AgentLoopConfig};
use crate::core::llm::{ChatMessage, StreamEvent};
use crate::core::session::manager::ContextState;
use crate::infra::error::AppError;
use crate::infra::event_bus::EventBus;
use crate::infra::wire;
use crate::infra::{DefaultEventBus, EventContext};

use super::mocks::{MockLlmProvider, MockPrimitiveExecutor};

/// 工具轮场景：metrics 必须在 turn_end 之前。
#[tokio::test]
async fn context_metrics_update_emitted_before_turn_end() {
    let stream_tool: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ToolCallDelta {
            index: 0,
            id: Some("call_m1".to_string()),
            name: Some("read".to_string()),
            arguments_delta: Some(r#"{"path":"/tmp/m"}"#.to_string()),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "tool_calls".to_string(),
        }),
    ];
    let stream_text: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "done".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let llm = Arc::new(MockLlmProvider::new(vec![stream_tool, stream_text]));
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());
    let order: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    for ev in &[wire::WIRE_CONTEXT_METRICS_UPDATE, wire::WIRE_TURN_END] {
        let list = Arc::clone(&order);
        let name = (*ev).to_string();
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
        session_id: "s-metrics-order".to_string(),
        ..Default::default()
    };
    let abort = CancellationToken::new();
    let mut loop_ = AgentLoop::new(llm, primitive, event_bus, config, abort);
    loop_.set_context_state(Some(ContextState {
        messages: Vec::new(),
        estimate_context_chars: 100,
        context_budget_chars: 100_000,
        context_budget_tokens: 25_000,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: std::path::PathBuf::new(),
        latest_plan_event: None,
        preheat: crate::core::compaction::preheat::Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    }));
    let messages = vec![ChatMessage::user("read")];
    let _ = loop_.run(messages).await.unwrap();
    let observed = order.lock().unwrap().clone();
    let metrics_pos = observed
        .iter()
        .position(|e| e == wire::WIRE_CONTEXT_METRICS_UPDATE);
    let turn_end_pos = observed.iter().position(|e| e == wire::WIRE_TURN_END);
    assert!(
        metrics_pos.is_some(),
        "context_metrics_update should be emitted, observed: {:?}",
        observed
    );
    assert!(
        metrics_pos.unwrap() < turn_end_pos.unwrap(),
        "context_metrics_update must precede turn_end, observed: {:?}",
        observed
    );
}

/// payload 字段类型与取值范围。
#[tokio::test]
async fn context_metrics_update_payload_contains_valid_values() {
    let stream_tool: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ToolCallDelta {
            index: 0,
            id: Some("call_v1".to_string()),
            name: Some("read".to_string()),
            arguments_delta: Some(r#"{"path":"/tmp/v"}"#.to_string()),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "tool_calls".to_string(),
        }),
    ];
    let stream_text: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "ok".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let llm = Arc::new(MockLlmProvider::new(vec![stream_tool, stream_text]));
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());
    let payloads: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
    let payloads_clone = Arc::clone(&payloads);
    event_bus.on(
        wire::WIRE_CONTEXT_METRICS_UPDATE,
        Box::new(move |ctx: EventContext| {
            payloads_clone.lock().unwrap().push(ctx.payload.clone());
            Ok(())
        }),
    );
    let config = AgentLoopConfig {
        model: "gpt-4".to_string(),
        session_id: "s-metrics-payload".to_string(),
        ..Default::default()
    };
    let abort = CancellationToken::new();
    let mut loop_ = AgentLoop::new(llm, primitive, event_bus, config, abort);
    loop_.set_context_state(Some(ContextState {
        messages: Vec::new(),
        estimate_context_chars: 2000,
        context_budget_chars: 100_000,
        context_budget_tokens: 25_000,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: std::path::PathBuf::new(),
        latest_plan_event: None,
        preheat: crate::core::compaction::preheat::Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    }));
    let messages = vec![ChatMessage::user("validate")];
    let _ = loop_.run(messages).await.unwrap();
    let captured = payloads.lock().unwrap();
    assert!(
        !captured.is_empty(),
        "should have captured at least one context_metrics_update payload"
    );
    let p = &captured[0];
    let tokens = p["inputTokensUsed"].as_u64().unwrap();
    let ratio = p["contextUtilizationRatio"].as_f64().unwrap();
    assert!(tokens > 0, "inputTokensUsed should be > 0, got {}", tokens);
    assert!(
        (0.0..=1.0).contains(&ratio),
        "contextUtilizationRatio should be in [0,1], got {}",
        ratio
    );
    assert!(p["compactionCount"].is_u64());
    assert!(p["compactionTokensFreed"].is_u64());
    assert!(p["totalToolResultBytesPersisted"].is_u64());
    assert_eq!(p["preheatInProgress"].as_bool(), Some(false));
    assert_eq!(p["preheatResultPending"].as_bool(), Some(false));
}

/// 多轮工具时仅发射两次 context_metrics（首请求前 + 最终 timing ⑤ 后）；
/// compaction_count 在后一次 payload 中单调不减于前一次。
#[tokio::test]
async fn context_metrics_compaction_count_accumulates_across_rounds() {
    let stream_tool1: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ToolCallDelta {
            index: 0,
            id: Some("call_a1".to_string()),
            name: Some("read".to_string()),
            arguments_delta: Some(r#"{"path":"/a"}"#.to_string()),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "tool_calls".to_string(),
        }),
    ];
    let stream_tool2: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ToolCallDelta {
            index: 0,
            id: Some("call_a2".to_string()),
            name: Some("read".to_string()),
            arguments_delta: Some(r#"{"path":"/b"}"#.to_string()),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "tool_calls".to_string(),
        }),
    ];
    let stream_end: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "fin".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let llm = Arc::new(MockLlmProvider::new(vec![
        stream_tool1,
        stream_tool2,
        stream_end,
    ]));
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());
    let counts: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
    let counts_clone = Arc::clone(&counts);
    event_bus.on(
        wire::WIRE_CONTEXT_METRICS_UPDATE,
        Box::new(move |ctx: EventContext| {
            if let Some(c) = ctx.payload.get("compactionCount").and_then(|v| v.as_u64()) {
                counts_clone.lock().unwrap().push(c);
            }
            Ok(())
        }),
    );
    let config = AgentLoopConfig {
        model: "gpt-4".to_string(),
        session_id: "s-metrics-accum".to_string(),
        ..Default::default()
    };
    let abort = CancellationToken::new();
    let mut loop_ = AgentLoop::new(llm, primitive, event_bus, config, abort);
    loop_.set_context_state(Some(ContextState {
        messages: Vec::new(),
        estimate_context_chars: 100,
        context_budget_chars: 100_000,
        context_budget_tokens: 25_000,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: std::path::PathBuf::new(),
        latest_plan_event: None,
        preheat: crate::core::compaction::preheat::Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    }));
    let messages = vec![ChatMessage::user("multi")];
    let _ = loop_.run(messages).await.unwrap();
    let captured = counts.lock().unwrap();
    assert_eq!(
        captured.len(),
        2,
        "run_reasoning_loop should emit exactly 2 context_metrics_update (before first LLM + after final reply), got {}",
        captured.len()
    );
    for window in captured.windows(2) {
        assert!(
            window[1] >= window[0],
            "compaction_count should be monotonically non-decreasing: {:?}",
            *captured
        );
    }
}

/// 无 context_state 时不发射 context_metrics_update。
#[tokio::test]
async fn context_metrics_update_skipped_when_no_context_state() {
    let stream_tool: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ToolCallDelta {
            index: 0,
            id: Some("call_n1".to_string()),
            name: Some("read".to_string()),
            arguments_delta: Some(r#"{"path":"/tmp/n"}"#.to_string()),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "tool_calls".to_string(),
        }),
    ];
    let stream_text: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "no ctx".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let llm = Arc::new(MockLlmProvider::new(vec![stream_tool, stream_text]));
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());
    let emitted: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let emitted_clone = Arc::clone(&emitted);
    event_bus.on(
        wire::WIRE_CONTEXT_METRICS_UPDATE,
        Box::new(move |ctx: EventContext| {
            emitted_clone.lock().unwrap().push(ctx.event_name.clone());
            Ok(())
        }),
    );
    let config = AgentLoopConfig {
        model: "gpt-4".to_string(),
        session_id: "s-no-ctx".to_string(),
        ..Default::default()
    };
    let abort = CancellationToken::new();
    let mut loop_ = AgentLoop::new(llm, primitive, event_bus, config, abort);
    let messages = vec![ChatMessage::user("hi")];
    let _ = loop_.run(messages).await.unwrap();
    let captured = emitted.lock().unwrap();
    assert!(
        captured.is_empty(),
        "context_metrics_update should NOT be emitted without context_state, got {:?}",
        *captured
    );
}

/// 纯文本回复：首 LLM 请求前一次 + timing ⑤ 后一次（共两次），最后一次在 turn_end 之前。
#[tokio::test]
async fn context_metrics_update_emitted_on_text_only_response() {
    let stream_text: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "hello".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let llm = Arc::new(MockLlmProvider::new(vec![stream_text]));
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());
    let order: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    for ev in &[wire::WIRE_CONTEXT_METRICS_UPDATE, wire::WIRE_TURN_END] {
        let list = Arc::clone(&order);
        let name = (*ev).to_string();
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
        session_id: "s-text-metrics".to_string(),
        ..Default::default()
    };
    let abort = CancellationToken::new();
    let mut loop_ = AgentLoop::new(llm, primitive, event_bus, config, abort);
    loop_.set_context_state(Some(ContextState {
        messages: Vec::new(),
        estimate_context_chars: 200,
        context_budget_chars: 100_000,
        context_budget_tokens: 25_000,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: std::path::PathBuf::new(),
        latest_plan_event: None,
        preheat: crate::core::compaction::preheat::Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    }));
    let messages = vec![ChatMessage::user("hi")];
    let result = loop_.run(messages).await.unwrap();
    assert_eq!(result.final_text, "hello");
    let observed = order.lock().unwrap().clone();
    let metrics_pos = observed
        .iter()
        .position(|e| e == wire::WIRE_CONTEXT_METRICS_UPDATE);
    let turn_end_pos = observed.iter().position(|e| e == wire::WIRE_TURN_END);
    assert!(
        metrics_pos.is_some(),
        "context_metrics_update should be emitted on text-only path, observed: {:?}",
        observed
    );
    assert!(
        metrics_pos.unwrap() < turn_end_pos.unwrap(),
        "context_metrics_update must precede turn_end, observed: {:?}",
        observed
    );
}

#[tokio::test]
async fn context_metrics_update_remains_nonzero_when_model_changes_with_same_context_state() {
    let first_stream: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "first".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
        Ok(StreamEvent::Usage {
            prompt_tokens: 120,
            completion_tokens: 20,
            total_tokens: Some(140),
        }),
    ];
    let llm_first = Arc::new(MockLlmProvider::new(vec![first_stream]));
    let primitive_first = Arc::new(MockPrimitiveExecutor);
    let event_bus_first = Arc::new(DefaultEventBus::new());
    let mut first_loop = AgentLoop::new(
        llm_first,
        primitive_first,
        event_bus_first,
        AgentLoopConfig {
            model: "gpt-5.4".to_string(),
            session_id: "s-model-a".to_string(),
            ..Default::default()
        },
        CancellationToken::new(),
    );
    first_loop.set_context_state(Some(ContextState {
        messages: Vec::new(),
        estimate_context_chars: 400,
        context_budget_chars: 100_000,
        context_budget_tokens: 25_000,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: std::path::PathBuf::new(),
        latest_plan_event: None,
        preheat: crate::core::compaction::preheat::Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    }));
    let _ = first_loop
        .run(vec![ChatMessage::user("first round keeps usage")])
        .await
        .unwrap();
    let carried_state = first_loop.take_context_state().expect("carried state");
    assert!(
        carried_state.live.input_tokens_used > 0,
        "first loop should materialize non-zero metrics before model switch"
    );

    let second_stream: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "second".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
        Ok(StreamEvent::Usage {
            prompt_tokens: 140,
            completion_tokens: 24,
            total_tokens: Some(164),
        }),
    ];
    let llm_second = Arc::new(MockLlmProvider::new(vec![second_stream]));
    let primitive_second = Arc::new(MockPrimitiveExecutor);
    let event_bus_second = Arc::new(DefaultEventBus::new());
    let payloads: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
    let payloads_for_listener = Arc::clone(&payloads);
    event_bus_second.on(
        wire::WIRE_CONTEXT_METRICS_UPDATE,
        Box::new(move |ctx: EventContext| {
            payloads_for_listener
                .lock()
                .unwrap()
                .push(ctx.payload.clone());
            Ok(())
        }),
    );
    let mut second_loop = AgentLoop::new(
        llm_second,
        primitive_second,
        event_bus_second,
        AgentLoopConfig {
            model: "gpt-5.2".to_string(),
            session_id: "s-model-b".to_string(),
            ..Default::default()
        },
        CancellationToken::new(),
    );
    second_loop.set_context_state(Some(carried_state));
    let _ = second_loop
        .run(vec![ChatMessage::user("second round after model switch")])
        .await
        .unwrap();

    let captured = payloads.lock().unwrap().clone();
    assert!(
        !captured.is_empty(),
        "second loop should emit ContextMetricsUpdate even after model switch"
    );
    let first_payload = &captured[0];
    assert!(
        first_payload["inputTokensUsed"].as_u64().unwrap() > 0,
        "inputTokensUsed should stay non-zero when reusing the same ContextState: {:?}",
        first_payload
    );
    assert!(
        first_payload["contextUtilizationRatio"].as_f64().unwrap() > 0.0,
        "contextUtilizationRatio should stay non-zero when reusing the same ContextState: {:?}",
        first_payload
    );
}
