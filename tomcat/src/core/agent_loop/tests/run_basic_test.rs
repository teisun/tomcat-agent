//! # 基础 Run 路径测试（正向 + 重试 + 工具循环 + 边界）
//!
//! 覆盖最朴素的四条路径：
//!
//! - text-only：LLM 一次返回纯文本，run 退出携带 final_text；
//! - 重试：第 1 次 chat_stream 返回 429，第 2 次成功；
//! - 工具循环：第 1 次 LLM 返回 tool_call，工具执行后第 2 次返回纯文本；
//! - 空消息：messages=[] 不崩溃，run 仍能 Ok 返回。

use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use base64::Engine as _;
use tokio_util::sync::CancellationToken;

use crate::core::agent_loop::{AgentLoop, AgentLoopConfig, AgentRunOutcome};
use crate::core::llm::multimodal::UNSUPPORTED_FILE_INPUT_PLACEHOLDER;
use crate::core::llm::{
    ChatMessage, ChatMessageContent, ChatMessageContentPart, MessageKind, ProviderRefs,
    ReasoningContinuation, ReasoningFormat, StreamEvent,
};
use crate::core::plan_runtime::file_store::PlanFileState;
use crate::core::plan_runtime::PlanRuntime;
use crate::core::session::manager::{
    estimate_msg_chars, ApiUsage, ContextState, MessageAppendSink,
};
use crate::infra::error::{llm_error, llm_http_status_error, AppError, LlmError, LlmErrorStage};
use crate::infra::event_bus::EventBus;
use crate::infra::{wire, DefaultEventBus, EventContext};

use super::mocks::{
    test_binding, MockLlmProvider, MockPrimitiveExecutor, RecordingStreamLlmProvider,
};

fn unsupported_file_stream() -> Vec<Result<StreamEvent, AppError>> {
    vec![
        Ok(StreamEvent::LlmError {
            reason: "error:invalid_request_error".to_string(),
            message:
                "[OneOfParam] [input[0].content[1]] [invalid_enum_value] Invalid value: 'input_file'. Supported values are: 'input_text'."
                    .to_string(),
            code: Some("invalid_request_error".to_string()),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "error:invalid_request_error".to_string(),
        }),
    ]
}

fn ok_text_stream(text: &str) -> Vec<Result<StreamEvent, AppError>> {
    vec![
        Ok(StreamEvent::ContentDelta {
            delta: text.to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ]
}

fn hidden_empty_stream(response_id: &str) -> Vec<Result<StreamEvent, AppError>> {
    vec![
        Ok(StreamEvent::ReasoningSnapshot {
            thinking_text: None,
            reasoning_continuation: Some(ReasoningContinuation {
                source_provider: "openai".to_string(),
                source_api: "openai-responses".to_string(),
                source_model: "gpt-5.4".to_string(),
                format: ReasoningFormat::OpenaiResponsesReasoningItems,
                opaque_payload: serde_json::json!([{
                    "type": "reasoning",
                    "id": "rsn_123",
                    "encrypted_content": "opaque"
                }]),
                fallback_text: None,
                provider_refs: Some(ProviderRefs {
                    openai_response_id: Some(response_id.to_string()),
                    replay_profile_id: None,
                }),
            }),
            continuity: None,
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ]
}

fn thinking_only_empty_stream() -> Vec<Result<StreamEvent, AppError>> {
    vec![
        Ok(StreamEvent::ReasoningSnapshot {
            thinking_text: Some("reasoning without a visible answer".to_string()),
            reasoning_continuation: None,
            continuity: None,
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ]
}

fn pdf_user_message() -> ChatMessage {
    let pdf_b64 = base64::engine::general_purpose::STANDARD.encode(b"%PDF-1.4\n%%EOF\n");
    ChatMessage::user_with_parts(vec![
        ChatMessageContentPart::text("summarize file"),
        ChatMessageContentPart::file_base64_data("notes.pdf", "application/pdf", pdf_b64)
            .expect("pdf part"),
    ])
}

fn overbudget_context_state(messages: Vec<ChatMessage>) -> ContextState {
    let estimate_context_chars = messages.iter().map(estimate_msg_chars).sum();
    ContextState {
        messages,
        estimate_context_chars,
        context_budget_chars: 10_000,
        context_budget_tokens: 2_500,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: std::path::PathBuf::new(),
        latest_plan_event: None,
        resume_control: Default::default(),
        preheat: crate::core::compaction::preheat::Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    }
}

#[tokio::test]
async fn run_moves_history_in_and_parks_it_back() {
    let mut historical = ChatMessage::user("earlier completed turn");
    historical.msg_id = Some("history-id".to_string());
    let mut context_state = overbudget_context_state(vec![historical.clone()]);
    let mut messages = std::mem::take(&mut context_state.messages);
    let mut current_user = ChatMessage::user("current turn");
    current_user.msg_id = Some("current-id".to_string());
    messages.push(current_user.clone());

    let llm = Arc::new(MockLlmProvider::new(vec![ok_text_stream("done")]));
    let mut loop_ = AgentLoop::new(
        test_binding(llm, "gpt-4"),
        Arc::new(MockPrimitiveExecutor),
        Arc::new(DefaultEventBus::new()),
        AgentLoopConfig {
            session_id: "single-list-park".to_string(),
            ..Default::default()
        },
        CancellationToken::new(),
    );
    loop_.set_context_state(Some(context_state));

    let result = loop_.run(messages).await.unwrap();
    assert_eq!(
        result
            .new_messages
            .iter()
            .filter_map(|message| message.msg_id.as_deref())
            .collect::<Vec<_>>(),
        vec!["current-id"],
        "new_messages contains the current turn but excludes parked history"
    );

    let parked = loop_
        .take_context_state()
        .expect("context state parked on exit");
    assert_eq!(parked.messages[0].msg_id.as_deref(), Some("history-id"));
    assert_eq!(parked.messages[1].msg_id.as_deref(), Some("current-id"));
    assert_eq!(
        parked
            .messages
            .iter()
            .filter_map(|message| message.msg_id.as_deref())
            .collect::<std::collections::HashSet<_>>()
            .len(),
        2,
        "the parked list has no duplicate durable ids"
    );
}

fn large_user_turn(label: &str) -> ChatMessage {
    ChatMessage::user(format!("{label}: {}", "x".repeat(3_200)))
}

#[derive(Default)]
struct RecordingAppendSink {
    next_id: Mutex<u32>,
    messages: Mutex<Vec<serde_json::Value>>,
    custom_entries: Mutex<Vec<serde_json::Value>>,
}

impl MessageAppendSink for RecordingAppendSink {
    fn append_message(&self, value: serde_json::Value) -> Result<String, AppError> {
        self.messages.lock().unwrap().push(value);
        let mut next = self.next_id.lock().unwrap();
        *next += 1;
        Ok(format!("msg-{}", *next))
    }

    fn append_custom_entry(&self, extra: serde_json::Value) -> Result<(), AppError> {
        self.custom_entries.lock().unwrap().push(extra);
        Ok(())
    }

    fn append_message_with_id(
        &self,
        value: serde_json::Value,
        forced_id: &str,
    ) -> Result<String, AppError> {
        self.messages.lock().unwrap().push(value);
        Ok(forced_id.to_string())
    }
}

#[tokio::test]
async fn run_returns_text_when_llm_returns_text_only() {
    let stream1: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "Hello".to_string(),
        }),
        Ok(StreamEvent::ContentDelta {
            delta: " world".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let llm = Arc::new(MockLlmProvider::new(vec![stream1]));
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());
    let config = AgentLoopConfig {
        session_id: "s1".to_string(),
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
    let result = loop_.run(messages).await.unwrap();
    assert_eq!(result.final_text, "Hello world");
}

#[tokio::test]
async fn run_refuses_to_send_an_assistant_tailed_request() {
    let llm = Arc::new(MockLlmProvider::new(vec![]));
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());
    let mut loop_ = AgentLoop::new(
        test_binding(llm, "gpt-4"),
        primitive,
        event_bus,
        AgentLoopConfig {
            session_id: "assistant-tail".to_string(),
            ..Default::default()
        },
        CancellationToken::new(),
    );

    let outcome = loop_
        .run(vec![
            ChatMessage::user("hello"),
            ChatMessage::assistant("partial"),
        ])
        .await;
    assert!(matches!(
        outcome,
        AgentRunOutcome::Failed(error)
            if error.to_string().contains("tail is not a user input or completed tool result")
    ));
}

#[tokio::test]
async fn outbound_invariant_allows_every_legal_path() {
    let mut completion_nudge = ChatMessage::user("continue until the plan is complete");
    completion_nudge.kind = MessageKind::Nudge;
    let mut background_signal = ChatMessage::user("background task finished");
    background_signal.kind = MessageKind::Signal;
    let steering = ChatMessage::steering("answer in Chinese");
    let mut tool_call = ChatMessage::assistant("");
    tool_call.tool_calls = Some(vec![serde_json::json!({
        "id": "read-1",
        "type": "function",
        "function": { "name": "read", "arguments": "{}" },
    })]);
    let legal_paths = vec![
        (
            "completion guard continuation",
            vec![ChatMessage::user("start"), completion_nudge],
        ),
        (
            "background follow-up injection",
            vec![ChatMessage::user("start"), background_signal],
        ),
        (
            "mid-turn steering injection",
            vec![ChatMessage::user("start"), steering],
        ),
        (
            "completed tool round",
            vec![
                ChatMessage::user("read the file"),
                tool_call,
                ChatMessage::tool("read-1", "file contents"),
            ],
        ),
    ];

    for (path, messages) in legal_paths {
        let llm = Arc::new(MockLlmProvider::new(vec![ok_text_stream("accepted")]));
        let mut loop_ = AgentLoop::new(
            test_binding(llm, "gpt-4"),
            Arc::new(MockPrimitiveExecutor),
            Arc::new(DefaultEventBus::new()),
            AgentLoopConfig {
                session_id: format!("legal-tail-{path}"),
                ..Default::default()
            },
            CancellationToken::new(),
        );
        let outcome = loop_.run(messages).await;
        assert!(
            outcome.is_ok(),
            "{path} must remain legal after the outbound tail invariant"
        );
    }
}

/// 重试：Mock LLM 先返回 429 再返回成功 -> 自动重试后得到文本。
#[tokio::test]
async fn run_retries_on_429_then_succeeds() {
    let stream_err = vec![Err(llm_http_status_error("mock", 429, "rate limit"))];
    let stream_ok: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "OK".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let llm = Arc::new(MockLlmProvider::new(vec![stream_err, stream_ok]));
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());
    let config = AgentLoopConfig {
        max_attempts: 3,
        system_prompt: None,
        session_id: "s1".to_string(),
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
    let result = loop_.run(messages).await.unwrap();
    assert_eq!(result.final_text, "OK");
}

#[tokio::test]
async fn run_unattended_transport_retries_past_interactive_budget_without_manual_resume() {
    let mut streams = (0..4)
        .map(|_| {
            vec![Err(llm_error(
                "mock",
                LlmErrorStage::Connect,
                "connection reset before response",
            ))]
        })
        .collect::<Vec<_>>();
    streams.push(ok_text_stream("RECOVERED_AUTONOMOUSLY"));
    let (provider, requests) = RecordingStreamLlmProvider::new(streams);

    let event_bus = Arc::new(DefaultEventBus::new());
    let retry_events = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    {
        let retry_events = Arc::clone(&retry_events);
        event_bus.on(
            wire::WIRE_AUTO_RETRY_START,
            Box::new(move |ctx: EventContext| {
                retry_events.lock().unwrap().push(ctx.payload);
                Ok(())
            }),
        );
    }
    let mut loop_ = AgentLoop::new(
        test_binding(Arc::new(provider), "gpt-4"),
        Arc::new(MockPrimitiveExecutor),
        event_bus,
        AgentLoopConfig {
            // The fifth request is past the interactive budget of four.
            max_attempts: 4,
            system_prompt: None,
            retry_base_delay_ms: 0,
            session_id: "unattended-transport-retry".to_string(),
            unattended_retry: true,
            plan_runtime: None,
            ..Default::default()
        },
        CancellationToken::new(),
    );

    let outcome = loop_
        .run(vec![ChatMessage::user("continue execution")])
        .await;

    let AgentRunOutcome::Completed(result) = outcome else {
        panic!("transient transport failure should recover in the same unattended run");
    };
    assert_eq!(result.final_text, "RECOVERED_AUTONOMOUSLY");
    assert_eq!(
        requests.0.lock().unwrap().len(),
        5,
        "the agent must issue the fifth request itself, without a user resume"
    );
    let retry_events = retry_events.lock().unwrap().clone();
    assert_eq!(
        retry_events
            .iter()
            .map(|event| event["attempt"].as_u64())
            .collect::<Vec<_>>(),
        vec![Some(2), Some(3), Some(4), Some(5)]
    );
    assert!(
        retry_events
            .iter()
            .all(|event| event["maxAttempts"].as_u64() == Some(10)),
        "unattended transport retries must advertise their elevated budget: {retry_events:?}"
    );
}

#[tokio::test]
async fn run_unattended_billing_429_stops_without_elevated_retries() {
    let (provider, requests) =
        RecordingStreamLlmProvider::new(vec![vec![Err(llm_http_status_error(
            "mock",
            429,
            r#"{"error":{"code":"insufficient_quota","message":"insufficient credits"}}"#,
        ))]]);
    let plan_runtime = PlanRuntime::new("unattended-billing-retry");
    plan_runtime.seed_active_plan_for_test(
        "unattended-billing-retry-plan".to_string(),
        PlanFileState::Executing,
    );
    let event_bus = Arc::new(DefaultEventBus::new());
    let retry_events = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    {
        let retry_events = Arc::clone(&retry_events);
        event_bus.on(
            wire::WIRE_AUTO_RETRY_START,
            Box::new(move |ctx: EventContext| {
                retry_events.lock().unwrap().push(ctx.payload);
                Ok(())
            }),
        );
    }
    let mut loop_ = AgentLoop::new(
        test_binding(Arc::new(provider), "gpt-4"),
        Arc::new(MockPrimitiveExecutor),
        event_bus,
        AgentLoopConfig {
            max_attempts: 4,
            system_prompt: None,
            retry_base_delay_ms: 0,
            session_id: "unattended-billing-retry".to_string(),
            plan_runtime: Some(plan_runtime),
            ..Default::default()
        },
        CancellationToken::new(),
    );

    let outcome = loop_
        .run(vec![ChatMessage::user("continue execution")])
        .await;

    assert!(
        matches!(outcome, AgentRunOutcome::Failed(_)),
        "insufficient quota is an account failure, not a transient retry"
    );
    assert_eq!(
        requests.0.lock().unwrap().len(),
        1,
        "billing must not consume the elevated unattended transport budget"
    );
    assert!(
        retry_events.lock().unwrap().is_empty(),
        "billing must not emit a waiting/retry notification"
    );
}

#[tokio::test]
async fn run_unattended_transport_exhaustion_stops_at_ten_attempts() {
    let streams = (0..10)
        .map(|_| {
            vec![Err(llm_error(
                "mock",
                LlmErrorStage::Connect,
                "connection reset before response",
            ))]
        })
        .collect::<Vec<_>>();
    let (provider, requests) = RecordingStreamLlmProvider::new(streams);
    let plan_runtime = PlanRuntime::new("unattended-transport-exhaustion");
    plan_runtime.seed_active_plan_for_test(
        "unattended-transport-exhaustion-plan".to_string(),
        PlanFileState::Executing,
    );
    let event_bus = Arc::new(DefaultEventBus::new());
    let retry_starts = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let retry_ends = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    {
        let retry_starts = Arc::clone(&retry_starts);
        event_bus.on(
            wire::WIRE_AUTO_RETRY_START,
            Box::new(move |ctx: EventContext| {
                retry_starts.lock().unwrap().push(ctx.payload);
                Ok(())
            }),
        );
    }
    {
        let retry_ends = Arc::clone(&retry_ends);
        event_bus.on(
            wire::WIRE_AUTO_RETRY_END,
            Box::new(move |ctx: EventContext| {
                retry_ends.lock().unwrap().push(ctx.payload);
                Ok(())
            }),
        );
    }
    let mut loop_ = AgentLoop::new(
        test_binding(Arc::new(provider), "gpt-4"),
        Arc::new(MockPrimitiveExecutor),
        event_bus,
        AgentLoopConfig {
            max_attempts: 4,
            system_prompt: None,
            retry_base_delay_ms: 0,
            session_id: "unattended-transport-exhaustion".to_string(),
            plan_runtime: Some(plan_runtime),
            ..Default::default()
        },
        CancellationToken::new(),
    );

    let outcome = tokio::time::timeout(
        Duration::from_secs(1),
        loop_.run(vec![ChatMessage::user("continue execution")]),
    )
    .await
    .expect("elevated retries must terminate instead of hanging");

    assert!(
        matches!(
            &outcome,
            AgentRunOutcome::Failed(error)
                if crate::infra::error::llm_stage(error) == Some(LlmErrorStage::Connect)
        ),
        "exhausting the elevated transport budget must surface the terminal LoopError::Fatal: {outcome:?}"
    );
    assert_eq!(
        requests.0.lock().unwrap().len(),
        10,
        "the unattended budget includes the initial request and must not issue an eleventh request"
    );
    let retry_starts = retry_starts.lock().unwrap().clone();
    assert_eq!(
        retry_starts
            .iter()
            .map(|event| event["attempt"].as_u64())
            .collect::<Vec<_>>(),
        (2..=10).map(Some).collect::<Vec<_>>()
    );
    assert!(
        retry_starts
            .iter()
            .all(|event| event["maxAttempts"].as_u64() == Some(10)),
        "every retry notification must retain the elevated ceiling: {retry_starts:?}"
    );
    let retry_ends = retry_ends.lock().unwrap().clone();
    assert_eq!(
        retry_ends.len(),
        1,
        "only the terminal retry result is emitted"
    );
    assert_eq!(retry_ends[0]["success"].as_bool(), Some(false));
    assert_eq!(retry_ends[0]["attempt"].as_u64(), Some(10));
}

#[tokio::test]
async fn run_unattended_non_billing_429_retries_past_interactive_budget() {
    let mut streams = (0..4)
        .map(|_| {
            vec![Err(llm_http_status_error(
                "mock",
                429,
                r#"{"error":{"code":"rate_limit_exceeded","message":"too many requests"}}"#,
            ))]
        })
        .collect::<Vec<_>>();
    streams.push(ok_text_stream("RECOVERED_AFTER_RATE_LIMIT"));
    let (provider, requests) = RecordingStreamLlmProvider::new(streams);
    let plan_runtime = PlanRuntime::new("unattended-rate-limit-retry");
    plan_runtime.seed_active_plan_for_test(
        "unattended-rate-limit-retry-plan".to_string(),
        PlanFileState::Executing,
    );
    let event_bus = Arc::new(DefaultEventBus::new());
    let retry_events = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    {
        let retry_events = Arc::clone(&retry_events);
        event_bus.on(
            wire::WIRE_AUTO_RETRY_START,
            Box::new(move |ctx: EventContext| {
                retry_events.lock().unwrap().push(ctx.payload);
                Ok(())
            }),
        );
    }
    let mut loop_ = AgentLoop::new(
        test_binding(Arc::new(provider), "gpt-4"),
        Arc::new(MockPrimitiveExecutor),
        event_bus,
        AgentLoopConfig {
            max_attempts: 4,
            system_prompt: None,
            retry_base_delay_ms: 0,
            session_id: "unattended-rate-limit-retry".to_string(),
            plan_runtime: Some(plan_runtime),
            ..Default::default()
        },
        CancellationToken::new(),
    );

    let outcome = loop_
        .run(vec![ChatMessage::user("continue execution")])
        .await;

    assert!(
        matches!(
            outcome,
            AgentRunOutcome::Completed(result) if result.final_text == "RECOVERED_AFTER_RATE_LIMIT"
        ),
        "a non-billing 429 must receive the elevated unattended retry budget"
    );
    assert_eq!(
        requests.0.lock().unwrap().len(),
        5,
        "the fifth request proves rate limiting retries beyond the interactive budget of four"
    );
    let retry_events = retry_events.lock().unwrap().clone();
    assert_eq!(
        retry_events
            .iter()
            .map(|event| event["attempt"].as_u64())
            .collect::<Vec<_>>(),
        vec![Some(2), Some(3), Some(4), Some(5)]
    );
    assert!(
        retry_events
            .iter()
            .all(|event| event["maxAttempts"].as_u64() == Some(10)),
        "unlike insufficient_quota, a rate-limit 429 must advertise the elevated ceiling: {retry_events:?}"
    );
}

#[tokio::test]
async fn run_retry_after_is_announced_and_used_instead_of_exponential_backoff() {
    let retry_after_error = AppError::LlmDetailed(Box::new(
        LlmError::http_status("mock", 429, "rate limit").with_retry_after_ms(1),
    ));
    let llm = Arc::new(MockLlmProvider::new(vec![
        vec![Err(retry_after_error)],
        ok_text_stream("RETRY_AFTER_RECOVERED"),
    ]));
    let event_bus = Arc::new(DefaultEventBus::new());
    let retry_events = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    {
        let retry_events = Arc::clone(&retry_events);
        event_bus.on(
            wire::WIRE_AUTO_RETRY_START,
            Box::new(move |ctx: EventContext| {
                retry_events.lock().unwrap().push(ctx.payload);
                Ok(())
            }),
        );
    }
    let mut loop_ = AgentLoop::new(
        test_binding(llm, "gpt-4"),
        Arc::new(MockPrimitiveExecutor),
        event_bus,
        AgentLoopConfig {
            max_attempts: 2,
            system_prompt: None,
            // A timeout below would fail if the loop ignored Retry-After and used this fallback.
            retry_base_delay_ms: 60_000,
            session_id: "retry-after-wait".to_string(),
            ..Default::default()
        },
        CancellationToken::new(),
    );

    let outcome = tokio::time::timeout(
        Duration::from_secs(1),
        loop_.run(vec![ChatMessage::user("retry with server delay")]),
    )
    .await
    .expect("Retry-After should avoid the 60-second exponential fallback");

    assert!(matches!(
        outcome,
        AgentRunOutcome::Completed(result) if result.final_text == "RETRY_AFTER_RECOVERED"
    ));
    let retry_events = retry_events.lock().unwrap().clone();
    assert_eq!(retry_events.len(), 1);
    assert_eq!(retry_events[0]["delayMs"].as_u64(), Some(1));
    assert_eq!(retry_events[0]["maxAttempts"].as_u64(), Some(2));
}

#[tokio::test]
async fn run_stops_after_one_request_when_overflow_trim_cannot_shrink_payload() {
    let (provider, requests) = RecordingStreamLlmProvider::new(vec![
        vec![Err(llm_http_status_error(
            "mock",
            400,
            r#"{"error":{"code":"context_length_exceeded"}}"#,
        ))],
        ok_text_stream("must not send"),
    ]);
    let mut loop_ = AgentLoop::new(
        test_binding(Arc::new(provider), "gpt-4"),
        Arc::new(MockPrimitiveExecutor),
        Arc::new(DefaultEventBus::new()),
        AgentLoopConfig {
            max_attempts: 4,
            system_prompt: None,
            retry_base_delay_ms: 0,
            session_id: "overflow-no-progress".to_string(),
            ..Default::default()
        },
        CancellationToken::new(),
    );

    let outcome = loop_.run(vec![ChatMessage::user("no context state")]).await;

    assert!(
        matches!(outcome, AgentRunOutcome::Failed(_)),
        "without ContextState the overflow retry cannot reduce its payload"
    );
    assert_eq!(
        requests.0.lock().unwrap().len(),
        1,
        "a retry with applied=false must fail honestly instead of resending the same payload"
    );
}

#[tokio::test]
async fn run_second_overflow_collapses_and_strictly_shrinks_main_requests() {
    let initial_messages = vec![
        large_user_turn("oldest"),
        large_user_turn("middle"),
        large_user_turn("latest"),
    ];
    let (provider, requests) = RecordingStreamLlmProvider::new(vec![
        vec![Err(llm_http_status_error(
            "mock",
            400,
            r#"{"error":{"code":"context_length_exceeded"}}"#,
        ))],
        vec![Err(llm_http_status_error(
            "mock",
            400,
            r#"{"error":{"code":"context_length_exceeded"}}"#,
        ))],
        ok_text_stream("recovered after collapse"),
    ]);
    let mut loop_ = AgentLoop::new(
        test_binding(Arc::new(provider), "gpt-4"),
        Arc::new(MockPrimitiveExecutor),
        Arc::new(DefaultEventBus::new()),
        AgentLoopConfig {
            max_attempts: 4,
            system_prompt: None,
            retry_base_delay_ms: 0,
            session_id: "overflow-collapse-progress".to_string(),
            ..Default::default()
        },
        CancellationToken::new(),
    );
    let mut stale_estimate = overbudget_context_state(initial_messages.clone());
    stale_estimate.last_api_usage = Some(ApiUsage {
        prompt_tokens: 0,
        completion_tokens: 0,
    });
    stale_estimate.post_usage_appended_chars = 0;
    stale_estimate.messages.clear();
    loop_.set_context_state(Some(stale_estimate));

    let outcome = loop_.run(initial_messages).await;

    assert!(
        matches!(outcome, AgentRunOutcome::Completed(_)),
        "the third request should receive the collapsed context and succeed: {outcome:?}"
    );
    let recorded = requests.0.lock().unwrap().clone();
    assert_eq!(recorded.len(), 3, "original + L3 retry + collapse retry");
    let request_chars = |index: usize| {
        recorded[index]
            .messages
            .iter()
            .map(estimate_msg_chars)
            .sum::<usize>()
    };
    assert!(
        recorded[1].messages.len() < recorded[0].messages.len()
            && request_chars(1) < request_chars(0),
        "first overflow must make strict L3 progress"
    );
    assert!(
        request_chars(2) < request_chars(1)
            && recorded[2]
                .messages
                .iter()
                .any(|message| message.kind == MessageKind::CompactionSummary),
        "second overflow must take Collapse and send a strictly smaller summary payload"
    );

    let context = loop_
        .take_context_state()
        .expect("context state remains available after recovery");
    assert_eq!(
        context.session_obs.compaction_count, 2,
        "one L3 reduction plus one Collapse must be recorded"
    );
    assert!(
        context
            .messages
            .first()
            .is_some_and(|message| message.kind == MessageKind::CompactionSummary),
        "second overflow must park the compacted history as a leading summary"
    );
}

#[tokio::test]
async fn tail_only_overflow_falls_back_to_collapse_on_first_attempt() {
    let initial_messages = vec![ChatMessage::user("oversized current input ".repeat(1_000))];
    let initial_chars = initial_messages.iter().map(estimate_msg_chars).sum();
    let (provider, requests) = RecordingStreamLlmProvider::new(vec![
        vec![Err(llm_http_status_error(
            "mock",
            400,
            r#"{"error":{"code":"context_length_exceeded"}}"#,
        ))],
        ok_text_stream("recovered after first-overflow collapse"),
    ]);
    let mut loop_ = AgentLoop::new(
        test_binding(Arc::new(provider), "gpt-4"),
        Arc::new(MockPrimitiveExecutor),
        Arc::new(DefaultEventBus::new()),
        AgentLoopConfig {
            max_attempts: 2,
            retry_base_delay_ms: 0,
            session_id: "tail-only-overflow".to_string(),
            ..Default::default()
        },
        CancellationToken::new(),
    );
    loop_.set_context_state(Some(ContextState {
        messages: Vec::new(),
        estimate_context_chars: initial_chars,
        // The local pre-request guard must accept the request. The provider error below is the
        // authoritative overflow signal that exercises the L3 tail-only fallback.
        context_budget_chars: initial_chars.saturating_mul(2),
        context_budget_tokens: initial_chars,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: std::path::PathBuf::new(),
        latest_plan_event: None,
        resume_control: Default::default(),
        preheat: crate::core::compaction::preheat::Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    }));

    let outcome = loop_.run(initial_messages).await;

    assert!(
        matches!(outcome, AgentRunOutcome::Completed(_)),
        "a tail-only overflow must collapse and retry in the same attempt: {outcome:?}"
    );
    let requests = requests.0.lock().unwrap().clone();
    assert_eq!(requests.len(), 2, "initial overflow plus collapsed retry");
    assert!(
        requests[1]
            .messages
            .iter()
            .any(|message| message.kind == MessageKind::CompactionSummary),
        "the retry must use the branch summary rather than the oversized tail"
    );
    let parked = loop_
        .take_context_state()
        .expect("context parked after retry");
    assert_eq!(parked.session_obs.compaction_count, 1);
    assert_eq!(parked.messages[0].kind, MessageKind::CompactionSummary);
}

#[tokio::test]
async fn guard_runs_before_first_request_of_a_turn() {
    let initial_messages = vec![
        large_user_turn("oldest"),
        large_user_turn("middle"),
        large_user_turn("latest"),
    ];
    let (provider, requests) =
        RecordingStreamLlmProvider::new(vec![ok_text_stream("guarded before first request")]);
    let mut loop_ = AgentLoop::new(
        test_binding(Arc::new(provider), "gpt-4"),
        Arc::new(MockPrimitiveExecutor),
        Arc::new(DefaultEventBus::new()),
        AgentLoopConfig {
            session_id: "first-request-guard".to_string(),
            ..Default::default()
        },
        CancellationToken::new(),
    );
    let mut context_state = overbudget_context_state(initial_messages.clone());
    context_state.context_budget_chars = 4_000;
    context_state.context_budget_tokens = 1_000;
    context_state.messages.clear();
    loop_.set_context_state(Some(context_state));

    let outcome = loop_.run(initial_messages).await;

    assert!(
        matches!(outcome, AgentRunOutcome::Completed(_)),
        "the first request should be sent after proactive compaction: {outcome:?}"
    );
    let recorded = requests.0.lock().unwrap().clone();
    assert_eq!(
        recorded.len(),
        1,
        "the guard must prevent an overflow round-trip"
    );
    assert!(
        recorded[0]
            .messages
            .iter()
            .any(|message| message.kind == MessageKind::CompactionSummary),
        "the first provider request should receive proactively collapsed context"
    );
    assert!(
        recorded[0].messages.iter().all(|message| {
            !message
                .text_content()
                .is_some_and(|text| text.starts_with("oldest:"))
        }),
        "the first provider request must not resend the oversized raw prefix"
    );
}

#[test]
fn retry_delay_uses_jitter_window_and_cap() {
    let min_delay = super::super::run::compute_retry_delay_ms(500, 2, 0);
    let max_delay = super::super::run::compute_retry_delay_ms(500, 2, 40);
    let capped = super::super::run::compute_retry_delay_ms(500, 20, 40);
    assert_eq!(min_delay, 400, "attempt=2 最小 jitter 应为 base 的 80%");
    assert_eq!(max_delay, 600, "attempt=2 最大 jitter 应为 base 的 120%");
    assert_eq!(capped, 8_000, "指数退避应被上限 cap 到 8s");
}

#[tokio::test]
async fn run_respects_configured_max_attempts() {
    let llm = Arc::new(MockLlmProvider::new(vec![
        vec![Err(llm_http_status_error(
            "mock",
            503,
            "service unavailable",
        ))],
        vec![Err(llm_http_status_error(
            "mock",
            503,
            "service unavailable",
        ))],
        vec![
            Ok(StreamEvent::ContentDelta {
                delta: "UNREACHABLE".to_string(),
            }),
            Ok(StreamEvent::FinishReason {
                reason: "stop".to_string(),
            }),
        ],
    ]));
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());
    let config = AgentLoopConfig {
        max_attempts: 2,
        system_prompt: None,
        retry_base_delay_ms: 0,
        session_id: "s1".to_string(),
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
    let outcome = loop_.run(vec![ChatMessage::user("hi")]).await;
    assert!(
        matches!(outcome, AgentRunOutcome::Failed(_)),
        "max_attempts=2 时第 3 次成功不应被消费"
    );
}

#[tokio::test]
async fn run_honors_larger_configured_attempt_budget() {
    let stream_ok: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "AFTER_RETRIES".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let llm = Arc::new(MockLlmProvider::new(vec![
        vec![Err(llm_http_status_error(
            "mock",
            503,
            "service unavailable",
        ))],
        vec![Err(llm_http_status_error(
            "mock",
            503,
            "service unavailable",
        ))],
        vec![Err(llm_http_status_error(
            "mock",
            503,
            "service unavailable",
        ))],
        vec![Err(llm_http_status_error(
            "mock",
            503,
            "service unavailable",
        ))],
        stream_ok,
    ]));
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());
    let config = AgentLoopConfig {
        max_attempts: 5,
        system_prompt: None,
        retry_base_delay_ms: 0,
        session_id: "s1".to_string(),
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
    let outcome = loop_.run(vec![ChatMessage::user("hi")]).await;
    let result = match outcome {
        AgentRunOutcome::Completed(result) => result,
        other => panic!("max_attempts=5 应允许第 5 次成功，实际: {other:?}"),
    };
    assert_eq!(result.final_text, "AFTER_RETRIES");
}

#[tokio::test]
async fn run_retries_unsupported_file_once_then_degrades_before_next_attempt() {
    let (provider, requests) = RecordingStreamLlmProvider::new(vec![
        unsupported_file_stream(),
        unsupported_file_stream(),
        ok_text_stream("DEGRADED_OK"),
    ]);
    let llm = Arc::new(provider);
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());
    let retry_events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let degrade_notices: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let retry_events = Arc::clone(&retry_events);
        event_bus.on(
            wire::WIRE_AUTO_RETRY_START,
            Box::new(move |_ctx: EventContext| {
                retry_events.lock().unwrap().push("retry".to_string());
                Ok(())
            }),
        );
    }
    {
        let degrade_notices = Arc::clone(&degrade_notices);
        event_bus.on(
            wire::WIRE_LLM_NOTICE,
            Box::new(move |ctx: EventContext| {
                if let Some(message) = ctx
                    .payload
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                {
                    degrade_notices.lock().unwrap().push(message.to_string());
                }
                Ok(())
            }),
        );
    }
    let config = AgentLoopConfig {
        max_attempts: 4,
        system_prompt: None,
        retry_base_delay_ms: 0,
        session_id: "s-unsupported-file".to_string(),
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
    let result = loop_.run(vec![pdf_user_message()]).await.unwrap();

    assert_eq!(result.final_text, "DEGRADED_OK");
    assert_eq!(
        retry_events.lock().unwrap().len(),
        2,
        "first failure should raw-retry once, second failure should start the degraded retry",
    );
    assert_eq!(
        degrade_notices.lock().unwrap().as_slice(),
        ["本轮附件未被当前端点接受，已按纯文本发送"],
    );

    let recorded = requests.0.lock().unwrap().clone();
    assert_eq!(
        recorded.len(),
        3,
        "expected original + raw retry + degraded retry"
    );
    for raw_request in &recorded[..2] {
        let user_message = raw_request
            .messages
            .iter()
            .rev()
            .find(|message| matches!(message.role, crate::core::llm::ChatMessageRole::User))
            .expect("user message");
        let Some(ChatMessageContent::Parts(parts)) = &user_message.content else {
            panic!(
                "expected multimodal user parts, got {:?}",
                user_message.content
            );
        };
        assert!(
            parts
                .iter()
                .any(|part| matches!(part, ChatMessageContentPart::InputFile { .. })),
            "the first two attempts must keep the original input_file: {parts:?}"
        );
    }
    let degraded_user_message = recorded[2]
        .messages
        .iter()
        .rev()
        .find(|message| matches!(message.role, crate::core::llm::ChatMessageRole::User))
        .expect("degraded user message");
    let Some(ChatMessageContent::Parts(parts)) = &degraded_user_message.content else {
        panic!(
            "expected degraded user parts, got {:?}",
            degraded_user_message.content
        );
    };
    assert!(
        parts
            .iter()
            .all(|part| !matches!(part, ChatMessageContentPart::InputFile { .. })),
        "degraded retry must strip input_file parts: {parts:?}"
    );
    assert!(
        parts.iter().any(|part| {
            matches!(
                part,
                ChatMessageContentPart::InputText { text }
                    if text.contains(UNSUPPORTED_FILE_INPUT_PLACEHOLDER)
            )
        }),
        "degraded retry must include the placeholder text: {parts:?}"
    );
}

#[tokio::test]
async fn run_unsupported_file_exhausts_full_retry_budget_before_failing() {
    let (provider, requests) = RecordingStreamLlmProvider::new(vec![
        unsupported_file_stream(),
        unsupported_file_stream(),
        unsupported_file_stream(),
        unsupported_file_stream(),
    ]);
    let llm = Arc::new(provider);
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());
    let retry_events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let retry_events = Arc::clone(&retry_events);
        event_bus.on(
            wire::WIRE_AUTO_RETRY_START,
            Box::new(move |_ctx: EventContext| {
                retry_events.lock().unwrap().push("retry".to_string());
                Ok(())
            }),
        );
    }
    let config = AgentLoopConfig {
        max_attempts: 4,
        system_prompt: None,
        retry_base_delay_ms: 0,
        session_id: "s-unsupported-file-fatal".to_string(),
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
    let outcome = loop_.run(vec![pdf_user_message()]).await;

    assert!(
        matches!(outcome, AgentRunOutcome::Failed(_)),
        "four refusals should still end as one final failure",
    );
    assert_eq!(retry_events.lock().unwrap().len(), 3);
    assert_eq!(
        requests.0.lock().unwrap().len(),
        4,
        "unsupported multimodal fallback must still honor the configured retry budget",
    );
}

#[tokio::test(start_paused = true)]
async fn run_retry_sleep_is_interruptible() {
    let llm = Arc::new(MockLlmProvider::new(vec![vec![Err(
        llm_http_status_error("mock", 503, "service unavailable"),
    )]]));
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());
    let config = AgentLoopConfig {
        max_attempts: 3,
        system_prompt: None,
        retry_base_delay_ms: 5_000,
        session_id: "s1".to_string(),
        ..Default::default()
    };
    let abort = CancellationToken::new();
    let cancel = abort.clone();
    let mut loop_ = AgentLoop::new(
        test_binding(llm, "gpt-4"),
        primitive,
        event_bus,
        config,
        abort,
    );
    let task = tokio::spawn(async move { loop_.run(vec![ChatMessage::user("hi")]).await });
    tokio::task::yield_now().await;
    cancel.cancel();
    let outcome = task.await.expect("join ok");
    assert!(
        matches!(outcome, AgentRunOutcome::Interrupted(_)),
        "退避 sleep 期间 cancel 应立即打断"
    );
}

#[tokio::test]
async fn run_persists_auto_retry_events_to_transcript_sink() {
    let llm = Arc::new(MockLlmProvider::new(vec![
        vec![Err(llm_http_status_error(
            "mock",
            503,
            "service unavailable",
        ))],
        ok_text_stream("RECOVERED"),
    ]));
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());
    let sink = Arc::new(RecordingAppendSink::default());
    let config = AgentLoopConfig {
        max_attempts: 2,
        system_prompt: None,
        retry_base_delay_ms: 0,
        session_id: "s-retry-transcript".to_string(),
        message_append_sink: Some(sink.clone()),
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

    let outcome = loop_.run(vec![ChatMessage::user("hi")]).await;
    assert!(matches!(outcome, AgentRunOutcome::Completed(_)));

    let entries = sink.custom_entries.lock().unwrap().clone();
    assert_eq!(entries.len(), 2, "expected retry start + end entries");
    assert_eq!(
        entries[0]["event"].as_str(),
        Some(wire::WIRE_AUTO_RETRY_START)
    );
    assert_eq!(entries[0]["attempt"].as_u64(), Some(2));
    assert_eq!(
        entries[1]["event"].as_str(),
        Some(wire::WIRE_AUTO_RETRY_END)
    );
    assert_eq!(entries[1]["success"].as_bool(), Some(true));
}

/// 空正文、无 thinking 的纯工具轮是合法中间态，不能被空回合守卫误判。
/// 所有空回合护栏都携带 usage：真实 provider 会回传它，省略会掩盖
/// 「正文为空但 completion_tokens 非零」这一线上形状。
#[tokio::test]
async fn run_pure_tool_turn_without_thinking_completes_in_two_requests() {
    let stream_tool: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ToolCallDelta {
            index: 0,
            id: Some("call_1".to_string()),
            name: Some("read".to_string()),
            arguments_delta: Some(r#"{"path":"/tmp/x"}"#.to_string()),
        }),
        Ok(StreamEvent::Usage {
            prompt_tokens: 100,
            completion_tokens: 3,
            total_tokens: Some(103),
            cache_read_tokens: None,
            cache_write_tokens: None,
            reasoning_tokens: None,
            text_tokens: None,
        }),
        Ok(StreamEvent::FinishReason {
            reason: "tool_calls".to_string(),
        }),
    ];
    let stream_text: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "done".to_string(),
        }),
        Ok(StreamEvent::Usage {
            prompt_tokens: 110,
            completion_tokens: 5,
            total_tokens: Some(115),
            cache_read_tokens: None,
            cache_write_tokens: None,
            reasoning_tokens: None,
            text_tokens: None,
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let (provider, requests) = RecordingStreamLlmProvider::new(vec![stream_tool, stream_text]);
    let llm = Arc::new(provider);
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());
    let config = AgentLoopConfig {
        session_id: "s1".to_string(),
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
    let messages = vec![ChatMessage::user("read /tmp/x")];
    let result = loop_.run(messages).await.unwrap();
    assert!(result.final_text.contains("done"));
    assert_eq!(
        requests.0.lock().unwrap().len(),
        2,
        "pure tool turn must execute then ask the model for its follow-up exactly once"
    );
}

/// 某些兼容端点会在合法的结构化空收尾使用 `end_turn`，但没有正文和 thinking。
/// 它与“只思考、不回答”不同，不能被终止守卫当成失败。
/// 使用真实 provider 形状：即使没有可见正文，usage 也可能包含 completion token。
#[tokio::test]
async fn run_structured_end_turn_without_content_is_not_empty_turn_failure() {
    let (provider, requests) = RecordingStreamLlmProvider::new(vec![vec![
        Ok(StreamEvent::Usage {
            prompt_tokens: 100,
            completion_tokens: 3,
            total_tokens: Some(103),
            cache_read_tokens: None,
            cache_write_tokens: None,
            reasoning_tokens: None,
            text_tokens: None,
        }),
        Ok(StreamEvent::FinishReason {
            reason: "end_turn".to_string(),
        }),
    ]]);
    let mut loop_ = AgentLoop::new(
        test_binding(Arc::new(provider), "gpt-4"),
        Arc::new(MockPrimitiveExecutor),
        Arc::new(DefaultEventBus::new()),
        AgentLoopConfig {
            session_id: "structured-empty-end-turn".to_string(),
            ..Default::default()
        },
        CancellationToken::new(),
    );

    let outcome = loop_
        .run(vec![ChatMessage::user("return structured empty")])
        .await;

    assert!(
        matches!(outcome, AgentRunOutcome::Completed(_)),
        "end_turn without content or thinking is a provider-valid structured completion: {outcome:?}"
    );
    assert_eq!(requests.0.lock().unwrap().len(), 1);
}

/// 工具已完整执行时，下一次收尾请求即使是空 `end_turn` 也不能把整个工具回合判失败。
#[tokio::test]
async fn run_empty_end_turn_after_tool_result_is_not_empty_turn_failure() {
    let stream_tool = vec![
        Ok(StreamEvent::ToolCallDelta {
            index: 0,
            id: Some("call_empty_tail".to_string()),
            name: Some("read".to_string()),
            arguments_delta: Some(r#"{"path":"/tmp/x"}"#.to_string()),
        }),
        Ok(StreamEvent::Usage {
            prompt_tokens: 100,
            completion_tokens: 3,
            total_tokens: Some(103),
            cache_read_tokens: None,
            cache_write_tokens: None,
            reasoning_tokens: None,
            text_tokens: None,
        }),
        Ok(StreamEvent::FinishReason {
            reason: "tool_calls".to_string(),
        }),
    ];
    let stream_empty_tail = vec![
        Ok(StreamEvent::Usage {
            prompt_tokens: 110,
            completion_tokens: 3,
            total_tokens: Some(113),
            cache_read_tokens: None,
            cache_write_tokens: None,
            reasoning_tokens: None,
            text_tokens: None,
        }),
        Ok(StreamEvent::FinishReason {
            reason: "end_turn".to_string(),
        }),
    ];
    let (provider, requests) =
        RecordingStreamLlmProvider::new(vec![stream_tool, stream_empty_tail]);
    let mut loop_ = AgentLoop::new(
        test_binding(Arc::new(provider), "gpt-4"),
        Arc::new(MockPrimitiveExecutor),
        Arc::new(DefaultEventBus::new()),
        AgentLoopConfig {
            session_id: "tool-result-empty-tail".to_string(),
            ..Default::default()
        },
        CancellationToken::new(),
    );

    let outcome = loop_.run(vec![ChatMessage::user("read then end")]).await;

    let AgentRunOutcome::Completed(result) = outcome else {
        panic!("a completed tool round followed by end_turn must remain successful");
    };
    assert!(result.final_text.is_empty());
    assert!(
        result
            .new_messages
            .iter()
            .any(|message| message.role == crate::core::llm::ChatMessageRole::Tool),
        "the successfully produced tool result must survive the empty tail"
    );
    assert_eq!(
        requests.0.lock().unwrap().len(),
        2,
        "one request for the tool turn and one for its empty structured tail"
    );
}

#[tokio::test]
async fn run_tool_loop_emits_display_on_tool_execution_end() {
    let stream_tool: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ToolCallDelta {
            index: 0,
            id: Some("call_1".to_string()),
            name: Some("write".to_string()),
            arguments_delta: Some(
                r#"{"path":"~/workspace/demo.txt","content":"","overwrite":false}"#.to_string(),
            ),
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
    let captured: Arc<Mutex<Option<serde_json::Value>>> = Arc::new(Mutex::new(None));
    let captured_cb = Arc::clone(&captured);
    event_bus.on(
        wire::WIRE_TOOL_EXECUTION_END,
        Box::new(move |ctx: EventContext| {
            *captured_cb.lock().unwrap() = Some(ctx.payload.clone());
            Ok(())
        }),
    );
    let config = AgentLoopConfig {
        session_id: "s1".to_string(),
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
    let messages = vec![ChatMessage::user("write demo file")];
    let _ = loop_.run(messages).await.unwrap();

    let payload = captured
        .lock()
        .unwrap()
        .clone()
        .expect("应捕获到 tool_execution_end payload");
    assert_eq!(payload["toolName"].as_str(), Some("write"));
    assert_eq!(payload["display"]["kind"].as_str(), Some("file"));
    assert_eq!(
        payload["display"]["file"].as_str(),
        Some("~/workspace/demo.txt")
    );
}

/// 边界：空消息列表必须被出站不变量明确拒绝，而不是发送畸形请求。
#[tokio::test]
async fn run_empty_messages_fails_before_calling_the_llm() {
    let stream1: Vec<Result<StreamEvent, AppError>> = vec![Ok(StreamEvent::FinishReason {
        reason: "stop".to_string(),
    })];
    let llm = Arc::new(MockLlmProvider::new(vec![stream1]));
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());
    let config = AgentLoopConfig {
        session_id: "s1".to_string(),
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
    let messages: Vec<ChatMessage> = vec![];
    let result = loop_.run(messages).await;
    assert!(matches!(
        result,
        AgentRunOutcome::Failed(error)
            if error.to_string().contains("tail is not a user input or completed tool result")
    ));
}

#[tokio::test]
async fn empty_turn_retries_then_succeeds_with_attempt_evidence() {
    let (provider, requests) = RecordingStreamLlmProvider::new(vec![
        thinking_only_empty_stream(),
        hidden_empty_stream("resp_empty_2"),
        ok_text_stream("recovered"),
    ]);
    let llm = Arc::new(provider);
    let event_bus = Arc::new(DefaultEventBus::new());
    let retry_events = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    {
        let retry_events = Arc::clone(&retry_events);
        event_bus.on(
            wire::WIRE_AUTO_RETRY_START,
            Box::new(move |ctx: EventContext| {
                retry_events.lock().unwrap().push(ctx.payload);
                Ok(())
            }),
        );
    }
    let sink = Arc::new(RecordingAppendSink::default());
    let mut loop_ = AgentLoop::new(
        test_binding(llm, "gpt-4"),
        Arc::new(MockPrimitiveExecutor),
        event_bus,
        AgentLoopConfig {
            max_attempts: 4,
            system_prompt: None,
            retry_base_delay_ms: 0,
            session_id: "s-reasoning-only".to_string(),
            message_append_sink: Some(sink.clone()),
            ..Default::default()
        },
        CancellationToken::new(),
    );

    let outcome = loop_.run(vec![ChatMessage::user("hi")]).await;
    let AgentRunOutcome::Completed(result) = outcome else {
        panic!("hidden empty output should retry and recover");
    };
    assert_eq!(result.final_text, "recovered");
    assert_eq!(
        requests.0.lock().unwrap().len(),
        3,
        "two empty responses must be retried before the successful third request"
    );
    assert_eq!(
        retry_events
            .lock()
            .unwrap()
            .iter()
            .map(|event| event["attempt"].as_u64())
            .collect::<Vec<_>>(),
        vec![Some(2), Some(3)]
    );
    assert!(retry_events
        .lock()
        .unwrap()
        .iter()
        .all(|event| event["maxAttempts"].as_u64() == Some(4)));
    let empty_turns = sink
        .custom_entries
        .lock()
        .unwrap()
        .iter()
        .filter(|entry| entry["event"].as_str() == Some("empty_turn"))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(empty_turns.len(), 2);
    assert_eq!(empty_turns[0]["event"].as_str(), Some("empty_turn"));
    assert_eq!(empty_turns[0]["attempt"].as_u64(), Some(1));
    assert_eq!(
        empty_turns[0]["failure_kind"].as_str(),
        Some("thinking_only")
    );
    assert!(empty_turns[0]["response_id"].is_null());
    assert_eq!(empty_turns[1]["attempt"].as_u64(), Some(2));
    assert_eq!(
        empty_turns[1]["failure_kind"].as_str(),
        Some("hidden_output")
    );
    assert_eq!(empty_turns[1]["response_id"].as_str(), Some("resp_empty_2"));
    let retry_ends = sink
        .custom_entries
        .lock()
        .unwrap()
        .iter()
        .filter(|entry| entry["event"].as_str() == Some(wire::WIRE_AUTO_RETRY_END))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(retry_ends.len(), 1);
    assert_eq!(retry_ends[0]["success"].as_bool(), Some(true));
    assert_eq!(retry_ends[0]["attempt"].as_u64(), Some(3));
}

#[tokio::test]
async fn truncated_encrypted_thinking_without_visible_text_is_fatal() {
    let stream = vec![
        Ok(StreamEvent::ReasoningSnapshot {
            thinking_text: None,
            reasoning_continuation: Some(ReasoningContinuation {
                source_provider: "anthropic".to_string(),
                source_api: "anthropic-messages".to_string(),
                source_model: "claude-opus-4-6".to_string(),
                format: ReasoningFormat::AnthropicThinkingBlocks,
                opaque_payload: serde_json::json!([{
                    "type": "thinking",
                    "thinking": "encrypted-payload",
                    "signature": "sig_123"
                }]),
                fallback_text: None,
                provider_refs: None,
            }),
            continuity: None,
        }),
        Ok(StreamEvent::Usage {
            prompt_tokens: 1_000,
            completion_tokens: 8_192,
            total_tokens: Some(9_192),
            cache_read_tokens: None,
            cache_write_tokens: None,
            reasoning_tokens: None,
            text_tokens: None,
        }),
        Ok(StreamEvent::FinishReason {
            reason: "max_tokens".to_string(),
        }),
    ];
    let (provider, requests) = RecordingStreamLlmProvider::new(vec![stream]);
    let mut loop_ = AgentLoop::new(
        test_binding(Arc::new(provider), "claude-opus-4-6"),
        Arc::new(MockPrimitiveExecutor),
        Arc::new(DefaultEventBus::new()),
        AgentLoopConfig {
            max_attempts: 4,
            system_prompt: None,
            retry_base_delay_ms: 0,
            session_id: "encrypted-thinking-truncated".to_string(),
            ..Default::default()
        },
        CancellationToken::new(),
    );
    let outcome = loop_.run(vec![ChatMessage::user("hi")]).await;
    assert!(
        matches!(&outcome, AgentRunOutcome::Failed(error) if error.to_string().contains("达到上限")),
        "truncated output without plaintext thinking must fail: {outcome:?}"
    );
    assert_eq!(requests.0.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn hidden_reasoning_exhausts_interactive_budget_with_retry_count() {
    let (provider, requests) = RecordingStreamLlmProvider::new(vec![
        hidden_empty_stream("resp_empty_1"),
        hidden_empty_stream("resp_empty_2"),
        hidden_empty_stream("resp_empty_3"),
        hidden_empty_stream("resp_empty_4"),
    ]);
    let mut loop_ = AgentLoop::new(
        test_binding(Arc::new(provider), "gpt-5.4"),
        Arc::new(MockPrimitiveExecutor),
        Arc::new(DefaultEventBus::new()),
        AgentLoopConfig {
            max_attempts: 4,
            system_prompt: None,
            retry_base_delay_ms: 0,
            session_id: "hidden-reasoning-not-truncated".to_string(),
            ..Default::default()
        },
        CancellationToken::new(),
    );

    let outcome = loop_.run(vec![ChatMessage::user("hi")]).await;

    assert!(
        matches!(&outcome, AgentRunOutcome::Failed(error) if error.to_string().contains("已自动重试 3 次")),
        "the exhausted error must state the actual automatic retry count: {outcome:?}"
    );
    assert_eq!(
        requests.0.lock().unwrap().len(),
        4,
        "interactive empty responses use the four-request budget"
    );
}

#[tokio::test]
async fn hidden_empty_turn_uses_unattended_ten_request_budget() {
    let mut streams = (0..5)
        .map(|index| hidden_empty_stream(&format!("resp_empty_{index}")))
        .collect::<Vec<_>>();
    streams.push(ok_text_stream("recovered unattended"));
    let (provider, requests) = RecordingStreamLlmProvider::new(streams);
    let event_bus = Arc::new(DefaultEventBus::new());
    let retry_events = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    {
        let retry_events = Arc::clone(&retry_events);
        event_bus.on(
            wire::WIRE_AUTO_RETRY_START,
            Box::new(move |ctx: EventContext| {
                retry_events.lock().unwrap().push(ctx.payload);
                Ok(())
            }),
        );
    }
    let mut loop_ = AgentLoop::new(
        test_binding(Arc::new(provider), "gpt-5.4"),
        Arc::new(MockPrimitiveExecutor),
        event_bus,
        AgentLoopConfig {
            max_attempts: 4,
            system_prompt: None,
            retry_base_delay_ms: 0,
            session_id: "hidden-empty-unattended".to_string(),
            unattended_retry: true,
            ..Default::default()
        },
        CancellationToken::new(),
    );

    let outcome = loop_.run(vec![ChatMessage::user("continue")]).await;

    assert!(
        matches!(outcome, AgentRunOutcome::Completed(ref result) if result.final_text == "recovered unattended"),
        "unattended empty responses should recover within the elevated budget: {outcome:?}"
    );
    assert_eq!(requests.0.lock().unwrap().len(), 6);
    assert!(
        retry_events
            .lock()
            .unwrap()
            .iter()
            .all(|event| event["maxAttempts"].as_u64() == Some(10)),
        "unattended retries must advertise the ten-request budget"
    );
}

#[tokio::test]
async fn responses_max_output_tokens_without_visible_text_is_fatal() {
    let stream = vec![
        Ok(StreamEvent::Usage {
            prompt_tokens: 1_000,
            completion_tokens: 8_192,
            total_tokens: Some(9_192),
            cache_read_tokens: None,
            cache_write_tokens: None,
            reasoning_tokens: Some(8_192),
            text_tokens: Some(0),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "max_output_tokens".to_string(),
        }),
    ];
    let (provider, requests) = RecordingStreamLlmProvider::new(vec![stream]);
    let mut loop_ = AgentLoop::new(
        test_binding(Arc::new(provider), "gpt-5.4"),
        Arc::new(MockPrimitiveExecutor),
        Arc::new(DefaultEventBus::new()),
        AgentLoopConfig {
            max_attempts: 4,
            system_prompt: None,
            retry_base_delay_ms: 0,
            session_id: "responses-thinking-truncated".to_string(),
            ..Default::default()
        },
        CancellationToken::new(),
    );

    let outcome = loop_.run(vec![ChatMessage::user("hi")]).await;

    assert!(
        matches!(&outcome, AgentRunOutcome::Failed(error) if error.to_string().contains("达到上限")),
        "Responses max-output exhaustion without visible text must fail: {outcome:?}"
    );
    assert_eq!(
        requests.0.lock().unwrap().len(),
        1,
        "the guard must not retry the same truncated Responses request"
    );
}

#[tokio::test]
async fn truncated_empty_tail_keeps_the_completed_tool_result_as_the_persisted_tail() {
    let tool_round = vec![
        Ok(StreamEvent::ToolCallDelta {
            index: 0,
            id: Some("read-before-truncation".to_string()),
            name: Some("read".to_string()),
            arguments_delta: Some(r#"{"path":"/tmp/x"}"#.to_string()),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "tool_calls".to_string(),
        }),
    ];
    let truncated_tail = vec![
        Ok(StreamEvent::ReasoningSnapshot {
            thinking_text: None,
            reasoning_continuation: Some(ReasoningContinuation {
                source_provider: "anthropic".to_string(),
                source_api: "anthropic-messages".to_string(),
                source_model: "claude-opus-4-6".to_string(),
                format: ReasoningFormat::AnthropicThinkingBlocks,
                opaque_payload: serde_json::json!([{"type": "thinking", "signature": "sig"}]),
                fallback_text: None,
                provider_refs: None,
            }),
            continuity: None,
        }),
        Ok(StreamEvent::FinishReason {
            reason: "max_tokens".to_string(),
        }),
    ];
    let (provider, _) = RecordingStreamLlmProvider::new(vec![tool_round, truncated_tail]);
    let sink = Arc::new(RecordingAppendSink::default());
    let mut loop_ = AgentLoop::new(
        test_binding(Arc::new(provider), "claude-opus-4-6"),
        Arc::new(MockPrimitiveExecutor),
        Arc::new(DefaultEventBus::new()),
        AgentLoopConfig {
            session_id: "tool-before-truncation".to_string(),
            message_append_sink: Some(sink.clone()),
            ..Default::default()
        },
        CancellationToken::new(),
    );

    let outcome = loop_.run(vec![ChatMessage::user("read then answer")]).await;

    assert!(matches!(outcome, AgentRunOutcome::Failed(_)));
    let persisted = sink.messages.lock().unwrap();
    assert_eq!(
        persisted
            .last()
            .and_then(|message| message["role"].as_str()),
        Some("tool"),
        "the failed empty assistant turn must not replace a completed tool result"
    );
    assert_eq!(
        persisted
            .last()
            .and_then(|message| message["tool_call_id"].as_str()),
        Some("read-before-truncation")
    );
}

#[tokio::test]
async fn truncated_partial_visible_text_remains_a_completed_turn() {
    let stream = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "partial answer".to_string(),
        }),
        Ok(StreamEvent::Usage {
            prompt_tokens: 1_000,
            completion_tokens: 8_192,
            total_tokens: Some(9_192),
            cache_read_tokens: None,
            cache_write_tokens: None,
            reasoning_tokens: None,
            text_tokens: None,
        }),
        Ok(StreamEvent::FinishReason {
            reason: "max_tokens".to_string(),
        }),
    ];
    let (provider, requests) = RecordingStreamLlmProvider::new(vec![stream]);
    let mut loop_ = AgentLoop::new(
        test_binding(Arc::new(provider), "claude-opus-4-6"),
        Arc::new(MockPrimitiveExecutor),
        Arc::new(DefaultEventBus::new()),
        AgentLoopConfig {
            session_id: "partial-text-truncated".to_string(),
            ..Default::default()
        },
        CancellationToken::new(),
    );
    let outcome = loop_.run(vec![ChatMessage::user("hi")]).await;
    assert!(
        matches!(&outcome, AgentRunOutcome::Completed(result) if result.final_text == "partial answer"),
        "a partial visible answer must remain available: {outcome:?}"
    );
    assert_eq!(requests.0.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn thinking_prefix_leak_is_retried_then_succeeds() {
    let stream = vec![
        Ok(StreamEvent::ReasoningSnapshot {
            thinking_text: Some(
                "I will reason through every implementation detail first.".to_string(),
            ),
            reasoning_continuation: None,
            continuity: None,
        }),
        Ok(StreamEvent::ContentDelta {
            delta: "I will reason".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let (provider, requests) =
        RecordingStreamLlmProvider::new(vec![stream, ok_text_stream("recovered")]);
    let sink = Arc::new(RecordingAppendSink::default());
    let mut loop_ = AgentLoop::new(
        test_binding(Arc::new(provider), "gpt-4"),
        Arc::new(MockPrimitiveExecutor),
        Arc::new(DefaultEventBus::new()),
        AgentLoopConfig {
            max_attempts: 4,
            system_prompt: None,
            retry_base_delay_ms: 0,
            session_id: "s-thinking-prefix-leak".to_string(),
            message_append_sink: Some(sink.clone()),
            ..Default::default()
        },
        CancellationToken::new(),
    );

    let outcome = loop_.run(vec![ChatMessage::user("hi")]).await;
    assert!(
        matches!(outcome, AgentRunOutcome::Completed(ref result) if result.final_text == "recovered")
    );
    assert_eq!(
        requests.0.lock().unwrap().len(),
        2,
        "a leaked thinking prefix must retry before the successful response"
    );
    let entries = sink.custom_entries.lock().unwrap().clone();
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry["event"].as_str() == Some("empty_turn"))
            .count(),
        1
    );
    assert!(entries.iter().any(|entry| {
        entry["event"].as_str() == Some(wire::WIRE_AUTO_RETRY_END)
            && entry["success"].as_bool() == Some(true)
    }));
}

#[tokio::test]
async fn duplicated_thinking_as_body_is_retried_then_succeeds() {
    let stream = vec![
        Ok(StreamEvent::ReasoningSnapshot {
            thinking_text: Some("I will inspect the implementation before answering.".to_string()),
            reasoning_continuation: None,
            continuity: None,
        }),
        Ok(StreamEvent::ContentDelta {
            delta: "I will inspect the implementation before answering.".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let (provider, requests) =
        RecordingStreamLlmProvider::new(vec![stream, ok_text_stream("recovered")]);
    let sink = Arc::new(RecordingAppendSink::default());
    let mut loop_ = AgentLoop::new(
        test_binding(Arc::new(provider), "gpt-4"),
        Arc::new(MockPrimitiveExecutor),
        Arc::new(DefaultEventBus::new()),
        AgentLoopConfig {
            max_attempts: 4,
            system_prompt: None,
            retry_base_delay_ms: 0,
            session_id: "s-duplicated-thinking".to_string(),
            message_append_sink: Some(sink.clone()),
            ..Default::default()
        },
        CancellationToken::new(),
    );

    assert!(matches!(
        loop_.run(vec![ChatMessage::user("hi")]).await,
        AgentRunOutcome::Completed(ref result) if result.final_text == "recovered"
    ));
    assert_eq!(requests.0.lock().unwrap().len(), 2);
    let entries = sink.custom_entries.lock().unwrap().clone();
    assert!(entries.iter().any(|entry| {
        entry["event"].as_str() == Some(wire::WIRE_AUTO_RETRY_END)
            && entry["success"].as_bool() == Some(true)
    }));
}

#[tokio::test]
async fn short_non_prefix_body_after_thinking_remains_a_valid_reply() {
    let stream = vec![
        Ok(StreamEvent::ReasoningSnapshot {
            thinking_text: Some("I will inspect the implementation before answering.".to_string()),
            reasoning_continuation: None,
            continuity: None,
        }),
        Ok(StreamEvent::ContentDelta {
            delta: "Done.".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let (provider, requests) = RecordingStreamLlmProvider::new(vec![stream]);
    let mut loop_ = AgentLoop::new(
        test_binding(Arc::new(provider), "gpt-4"),
        Arc::new(MockPrimitiveExecutor),
        Arc::new(DefaultEventBus::new()),
        AgentLoopConfig {
            max_attempts: 4,
            system_prompt: None,
            retry_base_delay_ms: 0,
            session_id: "s-short-final-body".to_string(),
            ..Default::default()
        },
        CancellationToken::new(),
    );

    assert!(matches!(
        loop_.run(vec![ChatMessage::user("hi")]).await,
        AgentRunOutcome::Completed(_)
    ));
    assert_eq!(requests.0.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn final_assistant_message_persists_provider_usage() {
    let stream = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "done".to_string(),
        }),
        Ok(StreamEvent::Usage {
            prompt_tokens: 12,
            completion_tokens: 34,
            cache_read_tokens: Some(9),
            cache_write_tokens: Some(3),
            total_tokens: Some(46),
            reasoning_tokens: Some(20),
            text_tokens: Some(14),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let mut loop_ = AgentLoop::new(
        test_binding(Arc::new(MockLlmProvider::new(vec![stream])), "gpt-4"),
        Arc::new(MockPrimitiveExecutor),
        Arc::new(DefaultEventBus::new()),
        AgentLoopConfig {
            session_id: "s-usage-persist".to_string(),
            ..Default::default()
        },
        CancellationToken::new(),
    );

    let outcome = loop_.run(vec![ChatMessage::user("hi")]).await;
    let AgentRunOutcome::Completed(result) = outcome else {
        panic!("text reply should complete");
    };
    let usage = result
        .new_messages
        .iter()
        .find(|message| matches!(message.role, crate::core::llm::ChatMessageRole::Assistant))
        .expect("result must include an assistant message")
        .usage
        .as_ref()
        .expect("assistant transcript message must keep provider usage");
    assert_eq!(usage.prompt_tokens, 12);
    assert_eq!(usage.completion_tokens, 34);
    assert_eq!(usage.cache_read_tokens, Some(9));
    assert_eq!(usage.cache_write_tokens, Some(3));
    assert_eq!(usage.total_tokens, Some(46));
    assert_eq!(usage.reasoning_tokens, Some(20));
    assert_eq!(usage.text_tokens, Some(14));
}

#[tokio::test]
async fn run_emits_tool_call_streaming_before_tool_execution_start_for_write() {
    let stream_tool: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ToolCallDelta {
            index: 0,
            id: Some("call_1".to_string()),
            name: Some("write".to_string()),
            arguments_delta: Some(
                r#"{"path":"~/workspace/demo.txt","content":"hello","overwrite":false}"#
                    .to_string(),
            ),
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
    let observed: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    for wire_name in [
        wire::WIRE_TOOL_CALL_STREAMING,
        wire::WIRE_TOOL_EXECUTION_START,
        wire::WIRE_TOOL_EXECUTION_END,
    ] {
        let sink = Arc::clone(&observed);
        let name = wire_name.to_string();
        event_bus.on(
            wire_name,
            Box::new(move |_ctx: EventContext| {
                sink.lock().unwrap().push(name.clone());
                Ok(())
            }),
        );
    }
    let config = AgentLoopConfig {
        session_id: "s-streaming-order".to_string(),
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
    let messages = vec![ChatMessage::user("write demo file")];
    let _ = loop_.run(messages).await.unwrap();

    assert_eq!(
        observed.lock().unwrap().clone(),
        vec![
            wire::WIRE_TOOL_CALL_STREAMING.to_string(),
            wire::WIRE_TOOL_EXECUTION_START.to_string(),
            wire::WIRE_TOOL_EXECUTION_END.to_string(),
        ]
    );
}
