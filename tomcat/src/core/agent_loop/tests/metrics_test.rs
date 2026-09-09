//! # ContextMetricsUpdate 用例
//!
//! 覆盖首请求估算、provider usage 到达、timing ⑤ 边界快照等发射点：
//!
//! 1. `*_before_tool_execution`：provider 实测 metrics 必须在工具开始前发射；
//! 2. `*_payload_contains_valid_values`：payload 字段类型 / 取值范围；
//! 3. `*_compaction_count_accumulates_across_rounds`：多轮 tool 时 compaction_count 单调不减；
//! 4. `*_skipped_when_no_context_state`：无 context_state 时不发射；
//! 5. `*_emitted_on_text_only_response`：纯文本路径也至少发射一次（首请求前），且仍在 turn_end 之前。

use std::sync::Arc;
use std::sync::Mutex;

use tokio_util::sync::CancellationToken;

use crate::core::agent_loop::{AgentLoop, AgentLoopConfig};
use crate::core::llm::{ChatMessage, StreamEvent};
use crate::core::session::manager::{estimate_msg_chars, CompactionResult, ContextState};
use crate::infra::config::ContextConfig;
use crate::infra::error::AppError;
use crate::infra::event_bus::EventBus;
use crate::infra::wire;
use crate::infra::{DefaultEventBus, EventContext};

use super::mocks::{test_binding, MockLlmProvider, MockPrimitiveExecutor};

/// 工具轮场景：provider usage 到达后，水位必须在工具开始执行前可见。
#[tokio::test]
async fn provider_usage_metric_is_emitted_before_tool_execution() {
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
        Ok(StreamEvent::Usage {
            prompt_tokens: 2_000,
            completion_tokens: 20,
            cache_read_tokens: None,
            cache_write_tokens: None,
            total_tokens: Some(2_020),
            reasoning_tokens: None,
            text_tokens: None,
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
    for ev in &[
        wire::WIRE_CONTEXT_METRICS_UPDATE,
        wire::WIRE_TOOL_EXECUTION_START,
        wire::WIRE_TURN_END,
    ] {
        let list = Arc::clone(&order);
        let name = (*ev).to_string();
        event_bus.on(
            &name,
            Box::new(move |ctx: EventContext| {
                let observed = if ctx.event_name == wire::WIRE_CONTEXT_METRICS_UPDATE {
                    let measured = ctx
                        .payload
                        .get("providerUsageMeasured")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false);
                    format!(
                        "{}:{}",
                        ctx.event_name,
                        if measured { "measured" } else { "estimated" }
                    )
                } else {
                    ctx.event_name.clone()
                };
                list.lock().unwrap().push(observed);
                Ok(())
            }),
        );
    }
    let config = AgentLoopConfig {
        session_id: "s-metrics-order".to_string(),
        ..Default::default()
    };
    let abort = CancellationToken::new();
    let mut loop_ = AgentLoop::new(
        test_binding(llm, "gpt-4"),
        primitive,
        event_bus,
        config,
        abort,
    );
    loop_.set_context_state(Some(ContextState {
        messages: Vec::new(),
        estimate_context_chars: 100,
        context_budget_chars: 100_000,
        context_budget_tokens: 25_000,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: std::path::PathBuf::new(),
        latest_plan_event: None,
        resume_control: Default::default(),
        preheat: crate::core::compaction::preheat::Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    }));
    let messages = vec![ChatMessage::user("read")];
    let _ = loop_.run(messages).await.unwrap();
    let observed = order.lock().unwrap().clone();
    let measured_metrics_pos = observed
        .iter()
        .position(|e| e == &format!("{}:measured", wire::WIRE_CONTEXT_METRICS_UPDATE));
    let tool_start_pos = observed
        .iter()
        .position(|e| e == wire::WIRE_TOOL_EXECUTION_START);
    let turn_end_pos = observed.iter().position(|e| e == wire::WIRE_TURN_END);
    assert!(
        measured_metrics_pos.is_some(),
        "provider-backed context_metrics_update should be emitted, observed: {:?}",
        observed
    );
    assert!(
        measured_metrics_pos.unwrap() < tool_start_pos.unwrap(),
        "provider-backed waterline must arrive before tool execution, observed: {:?}",
        observed
    );
    assert!(
        measured_metrics_pos.unwrap() < turn_end_pos.unwrap(),
        "provider-backed context_metrics_update must precede turn_end, observed: {:?}",
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
        Ok(StreamEvent::Usage {
            prompt_tokens: 2_000,
            completion_tokens: 20,
            cache_read_tokens: Some(1_500),
            cache_write_tokens: None,
            total_tokens: Some(2_020),
            reasoning_tokens: None,
            text_tokens: None,
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
        session_id: "s-metrics-payload".to_string(),
        ..Default::default()
    };
    let abort = CancellationToken::new();
    let mut loop_ = AgentLoop::new(
        test_binding(llm, "gpt-4"),
        primitive,
        event_bus,
        config,
        abort,
    );
    loop_.set_context_state(Some(ContextState {
        messages: Vec::new(),
        estimate_context_chars: 2000,
        context_budget_chars: 100_000,
        context_budget_tokens: 25_000,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: std::path::PathBuf::new(),
        latest_plan_event: None,
        resume_control: Default::default(),
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
    assert!(p["promptTokensTotal"].is_u64());
    assert!(p["cacheReadTokensTotal"].is_u64());
    assert_eq!(p["preheatInProgress"].as_bool(), Some(false));
    assert_eq!(p["preheatResultPending"].as_bool(), Some(false));
    assert_eq!(
        p["providerUsageMeasured"].as_bool(),
        Some(false),
        "the first metric is emitted before the provider returns usage"
    );
    let sample_kinds: Vec<Option<bool>> = captured
        .iter()
        .map(|payload| payload["providerUsageMeasured"].as_bool())
        .collect();
    assert_eq!(
        sample_kinds,
        vec![Some(false), Some(true), Some(false)],
        "only the event emitted directly from StreamEvent::Usage is provider-measured; \
         the trailing timing-⑤ snapshot is an estimate"
    );
    let provider_measured = &captured[1];
    assert_eq!(provider_measured["promptTokensTotal"], 2_000);
    assert_eq!(provider_measured["cacheReadTokensTotal"], 1_500);
    assert_eq!(provider_measured["cacheHitRatio"], 0.75);
}

#[tokio::test]
async fn boundary_application_runs_layer0_before_final_context_metrics() {
    let stream: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "completed".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let llm = Arc::new(MockLlmProvider::new(vec![stream]));
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

    let trail = tempfile::tempdir().expect("trail directory");
    let config = AgentLoopConfig {
        session_id: "s-metrics-boundary-l0".to_string(),
        agent_trail_dir: trail.path().display().to_string(),
        context_config: ContextConfig {
            keep_recent_turns: 1,
            layer0_single_result_max_chars: 50_000,
            layer0_placeholder_threshold_chars: 10_000,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut loop_ = AgentLoop::new(
        test_binding(llm, "gpt-4"),
        Arc::new(MockPrimitiveExecutor),
        event_bus,
        config,
        CancellationToken::new(),
    );
    let mut covered_start = ChatMessage::user("covered start");
    covered_start.msg_id = Some("covered-start".to_string());
    let mut covered_end = ChatMessage::user("covered end");
    covered_end.msg_id = Some("covered-end".to_string());
    let mut surviving_old_turn = ChatMessage::user("surviving old turn");
    surviving_old_turn.msg_id = Some("surviving-old-turn".to_string());
    let mut last_turn = ChatMessage::user("last turn");
    last_turn.msg_id = Some("last-turn".to_string());
    let history = vec![
        covered_start,
        covered_end,
        surviving_old_turn,
        ChatMessage::tool("old-tool", &"o".repeat(12_000)),
        last_turn,
        ChatMessage::tool("large-tool", &"n".repeat(60_000)),
    ];
    let estimate_context_chars: usize = history.iter().map(estimate_msg_chars).sum();
    let mut preheat = crate::core::compaction::preheat::Preheat::new();
    preheat.restore_completed(CompactionResult {
        summary_text: "summary".to_string(),
        covered_start_id: "covered-start".to_string(),
        covered_end_id: "covered-end".to_string(),
        covered_count: 2,
        transcript_compaction_entry_id: Some("metrics-boundary".to_string()),
        estimated_covered_tokens_before: Some(15_000),
        estimated_summary_tokens: Some(10),
        estimated_tokens_saved: Some(14_990),
        preheat_elapsed_ms: 0,
    });
    loop_.set_context_state(Some(ContextState {
        messages: history.clone(),
        estimate_context_chars,
        context_budget_chars: 40_000,
        context_budget_tokens: 10_000,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: std::path::PathBuf::new(),
        latest_plan_event: None,
        resume_control: Default::default(),
        preheat,
        session_obs: Default::default(),
        live: Default::default(),
    }));
    let mut initial_messages = vec![ChatMessage::system("sys")];
    initial_messages.extend(history);
    initial_messages.push(ChatMessage::user("current input"));

    let outcome = loop_.run(initial_messages).await;
    assert!(
        outcome.is_ok(),
        "the mock turn should complete: {outcome:?}"
    );

    let state = loop_.context_state.as_ref().expect("context state");
    assert!(
        state.messages.iter().any(|message| message.text_content()
            == Some(crate::core::compaction::TOOL_RESULT_PLACEHOLDER)),
        "the surviving old result must be placeholdered before final metrics"
    );
    assert!(
        state.messages.iter().any(|message| {
            message
                .text_content()
                .is_some_and(|text| text.starts_with("[Tool result persisted:"))
        }),
        "the large previous turn result must be persisted before final metrics"
    );
    let captured = payloads.lock().unwrap();
    let final_metrics = captured.last().expect("final context metrics");
    assert!(
        final_metrics["contextUtilizationRatio"]
            .as_f64()
            .is_some_and(|ratio| ratio < 0.85),
        "L0's savings must be reflected in the final waterline: {final_metrics}"
    );
    assert!(
        final_metrics["totalToolResultBytesPersisted"]
            .as_u64()
            .is_some_and(|persisted| persisted >= 60_000),
        "final metrics must include L0 persistence"
    );
}

/// Anthropic wire 已把 input + cache read + cache write 归一为 prompt_tokens
/// 后，最终 metrics 必须使用总量，而不是将原始 input_tokens 误报为约 0.7%。
#[tokio::test]
async fn context_metrics_uses_normalized_anthropic_input_total() {
    let stream: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "done".to_string(),
        }),
        Ok(StreamEvent::Usage {
            prompt_tokens: 209_600,
            completion_tokens: 1_257,
            cache_read_tokens: Some(202_789),
            cache_write_tokens: Some(6_801),
            total_tokens: Some(210_857),
            reasoning_tokens: None,
            text_tokens: None,
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let llm = Arc::new(MockLlmProvider::new(vec![stream]));
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
    let mut loop_ = AgentLoop::new(
        test_binding(llm, "claude-opus-5"),
        Arc::new(MockPrimitiveExecutor),
        event_bus,
        AgentLoopConfig {
            session_id: "s-metrics-anthropic-usage".to_string(),
            ..Default::default()
        },
        CancellationToken::new(),
    );
    loop_.set_context_state(Some(ContextState {
        messages: Vec::new(),
        estimate_context_chars: 0,
        context_budget_chars: 3_488_000,
        context_budget_tokens: 872_000,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: std::path::PathBuf::new(),
        latest_plan_event: None,
        resume_control: Default::default(),
        preheat: crate::core::compaction::preheat::Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    }));

    loop_.run(vec![ChatMessage::user("measure")]).await.unwrap();

    let payloads = payloads.lock().unwrap();
    let final_ratio = payloads
        .last()
        .and_then(|payload| payload["contextUtilizationRatio"].as_f64())
        .expect("the final metrics update must contain a ratio");
    assert!(
        (final_ratio - (210_857.0 / 872_000.0)).abs() < 0.000_001,
        "normalized Anthropic usage must drive the displayed ratio, got {final_ratio}"
    );
}

/// 精确 usage 穿过 stream handler 后，0.50 水位必须启动异步摘要预热；
/// 不允许只在手工调用 ContextState 时才覆盖这条门。
#[tokio::test]
async fn normalized_usage_starts_preheat_at_half_watermark() {
    let stream: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "done".to_string(),
        }),
        Ok(StreamEvent::Usage {
            prompt_tokens: 436_000,
            completion_tokens: 0,
            cache_read_tokens: Some(430_000),
            cache_write_tokens: Some(6_000),
            total_tokens: Some(436_000),
            reasoning_tokens: None,
            text_tokens: None,
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let llm = Arc::new(MockLlmProvider::new(vec![stream]));
    let event_bus = Arc::new(DefaultEventBus::new());
    let starts = Arc::new(Mutex::new(0usize));
    let starts_clone = Arc::clone(&starts);
    event_bus.on(
        wire::WIRE_AUTO_COMPACTION_START,
        Box::new(move |_ctx: EventContext| {
            *starts_clone.lock().unwrap() += 1;
            Ok(())
        }),
    );
    let mut loop_ = AgentLoop::new(
        test_binding(llm, "claude-opus-5"),
        Arc::new(MockPrimitiveExecutor),
        event_bus,
        AgentLoopConfig {
            session_id: "s-preheat-normalized-usage".to_string(),
            ..Default::default()
        },
        CancellationToken::new(),
    );
    let mut initial_message = ChatMessage::user("summarize when needed");
    initial_message.msg_id = Some("preheat-user".to_string());
    loop_.set_context_state(Some(ContextState {
        messages: vec![initial_message.clone()],
        estimate_context_chars: 0,
        context_budget_chars: 3_488_000,
        context_budget_tokens: 872_000,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: std::path::PathBuf::new(),
        latest_plan_event: None,
        resume_control: Default::default(),
        preheat: crate::core::compaction::preheat::Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    }));

    loop_.run(vec![initial_message]).await.unwrap();

    assert_eq!(
        *starts.lock().unwrap(),
        1,
        "normalized 0.50 usage must enter the preheat gate"
    );
}

/// 本 mock 不返回 provider usage，因此多轮工具仅发射两次 context_metrics
/// （首请求前 + 最终 timing ⑤ 后）；compaction_count 在后一次 payload 中单调不减。
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
        session_id: "s-metrics-accum".to_string(),
        ..Default::default()
    };
    let abort = CancellationToken::new();
    let mut loop_ = AgentLoop::new(
        test_binding(llm, "gpt-4"),
        primitive,
        event_bus,
        config,
        abort,
    );
    loop_.set_context_state(Some(ContextState {
        messages: Vec::new(),
        estimate_context_chars: 100,
        context_budget_chars: 100_000,
        context_budget_tokens: 25_000,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: std::path::PathBuf::new(),
        latest_plan_event: None,
        resume_control: Default::default(),
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
        session_id: "s-no-ctx".to_string(),
        ..Default::default()
    };
    let abort = CancellationToken::new();
    let mut loop_ = AgentLoop::new(
        test_binding(llm, "gpt-4"),
        primitive,
        event_bus,
        config,
        abort,
    );
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
        session_id: "s-text-metrics".to_string(),
        ..Default::default()
    };
    let abort = CancellationToken::new();
    let mut loop_ = AgentLoop::new(
        test_binding(llm, "gpt-4"),
        primitive,
        event_bus,
        config,
        abort,
    );
    loop_.set_context_state(Some(ContextState {
        messages: Vec::new(),
        estimate_context_chars: 200,
        context_budget_chars: 100_000,
        context_budget_tokens: 25_000,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: std::path::PathBuf::new(),
        latest_plan_event: None,
        resume_control: Default::default(),
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
            cache_read_tokens: None,
            cache_write_tokens: None,
            total_tokens: Some(140),
            reasoning_tokens: None,
            text_tokens: None,
        }),
    ];
    let llm_first = Arc::new(MockLlmProvider::new(vec![first_stream]));
    let primitive_first = Arc::new(MockPrimitiveExecutor);
    let event_bus_first = Arc::new(DefaultEventBus::new());
    let mut first_loop = AgentLoop::new(
        test_binding(llm_first, "gpt-5.4"),
        primitive_first,
        event_bus_first,
        AgentLoopConfig {
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
        resume_control: Default::default(),
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
            cache_read_tokens: None,
            cache_write_tokens: None,
            total_tokens: Some(164),
            reasoning_tokens: None,
            text_tokens: None,
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
        test_binding(llm_second, "gpt-5.2"),
        primitive_second,
        event_bus_second,
        AgentLoopConfig {
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
