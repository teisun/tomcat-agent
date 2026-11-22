use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use super::super::current_tail_guard;
use super::super::{AgentLoop, AgentLoopConfig};
use super::mocks::{
    test_binding, MockPrimitiveExecutor, RecordedChatCall, RecordingChatLlmProvider,
    RecordingStreamLlmProvider,
};
use crate::core::compaction::preheat::Preheat;
use crate::core::llm::{
    ChatMessage, ChatRequest, ChatResponse, LlmProvider, MessageKind, StreamEvent,
};
use crate::core::session::manager::ContextState;
use crate::infra::config::ContextConfig;
use crate::infra::error::{llm_http_status_error, AppError};
use crate::infra::{wire, DefaultEventBus, EventBus, EventContext};

fn compaction_billing_error() -> AppError {
    llm_http_status_error(
        "deepseek",
        402,
        r#"{"error":{"message":"Insufficient Balance","type":"unknown_error","param":null,"code":"invalid_request_error"}}"#,
    )
}

fn session_model_error() -> AppError {
    llm_http_status_error("openai", 503, "session provider unavailable")
}

fn capture_notices() -> (Arc<DefaultEventBus>, Arc<Mutex<Vec<serde_json::Value>>>) {
    let bus = Arc::new(DefaultEventBus::new());
    let notices = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&notices);
    bus.on(
        wire::WIRE_LLM_NOTICE,
        Box::new(move |ctx: EventContext| {
            recorded.lock().unwrap().push(ctx.payload);
            Ok(())
        }),
    );
    (bus, notices)
}

fn over_budget_agent(
    main: Arc<dyn LlmProvider>,
    compaction: Option<Arc<dyn LlmProvider>>,
    compaction_model: &str,
    event_bus: Arc<DefaultEventBus>,
) -> (AgentLoop, Vec<ChatMessage>) {
    let mut agent = AgentLoop::new(
        test_binding(main, "main-x"),
        Arc::new(MockPrimitiveExecutor),
        event_bus,
        AgentLoopConfig {
            session_id: "sess-compaction-fallback".into(),
            context_config: ContextConfig {
                compaction_model: compaction_model.into(),
                ..Default::default()
            },
            compaction_provider: compaction,
            ..Default::default()
        },
        CancellationToken::new(),
    );
    let mut user = ChatMessage::user("u".repeat(4_000));
    user.msg_id = Some("u1".into());
    let mut assistant = ChatMessage::assistant("a".repeat(4_000));
    assistant.msg_id = Some("a1".into());
    let messages = vec![user, assistant];
    agent.set_context_state(Some(ContextState {
        messages: vec![],
        estimate_context_chars: 8_000,
        context_budget_chars: 200,
        context_budget_tokens: 50,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        latest_plan_event: None,
        resume_control: Default::default(),
        preheat: Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
        hydrated: true,
    }));
    (agent, messages)
}

struct SlowPreheat {
    response: Option<(Duration, Result<ChatResponse, AppError>)>,
    trace: Arc<Mutex<Vec<&'static str>>>,
}

#[async_trait::async_trait]
impl LlmProvider for SlowPreheat {
    fn provider_name(&self) -> &str {
        "slow-preheat"
    }

    async fn chat(&self, _: ChatRequest) -> Result<ChatResponse, AppError> {
        struct Exit<'a>(&'a Mutex<Vec<&'static str>>);
        impl Drop for Exit<'_> {
            fn drop(&mut self) {
                self.0.lock().unwrap().push("A exited");
            }
        }
        let _exit = Exit(&self.trace);
        self.trace.lock().unwrap().push("A started");
        match &self.response {
            Some((delay, result)) => {
                tokio::time::sleep(*delay).await;
                match result {
                    Ok(response) => Ok(response.clone()),
                    Err(_) => Err(session_model_error()),
                }
            }
            None => std::future::pending().await,
        }
    }

    async fn chat_stream(
        &self,
        _: ChatRequest,
    ) -> Result<
        Box<dyn tokio_stream::Stream<Item = Result<StreamEvent, AppError>> + Send + Unpin>,
        AppError,
    > {
        unreachable!("only used as the background summary provider")
    }

    fn count_tokens(&self, _: &[ChatMessage]) -> Result<u32, AppError> {
        Ok(0)
    }
}

fn start_preheat(agent: &mut AgentLoop, prefix: &[ChatMessage], provider: SlowPreheat) {
    let state = agent.context_state.as_mut().unwrap();
    assert!(state.preheat.try_start(
        0.6,
        prefix,
        &state.transcript_path,
        None,
        Arc::new(provider),
        None,
        None,
        &agent.config.context_config,
        Arc::new(agent.emitter.clone()),
        None,
    ));
}

fn track_compaction_phase(bus: &DefaultEventBus, trace: &Arc<Mutex<Vec<&'static str>>>) {
    for (wire_name, label) in [
        (wire::WIRE_FOREGROUND_COMPACTION_START, "blocked"),
        (wire::WIRE_FOREGROUND_COMPACTION_END, "unblocked"),
    ] {
        let trace = Arc::clone(trace);
        bus.on(
            wire_name,
            Box::new(move |_| {
                trace.lock().unwrap().push(label);
                Ok(())
            }),
        );
    }
}

#[tokio::test(start_paused = true)]
async fn preheat_handoff_reaps_a_before_b_at_300_seconds() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let trace = Arc::new(Mutex::new(Vec::new()));
    let (bus, _) = capture_notices();
    track_compaction_phase(&bus, &trace);
    let (mut agent, mut messages) = over_budget_agent(
        Arc::new(RecordingChatLlmProvider::new(
            "stub",
            "B summary",
            calls.clone(),
        )),
        None,
        "main-x",
        bus,
    );
    start_preheat(
        &mut agent,
        &messages[..1],
        SlowPreheat {
            response: None,
            trace: trace.clone(),
        },
    );
    tokio::task::yield_now().await;
    assert_eq!(
        *trace.lock().unwrap(),
        ["A started"],
        "background A must not show the blocking phase"
    );
    let start = tokio::time::Instant::now();
    let guard = tokio::spawn(async move {
        current_tail_guard::maybe_reduce_before_next_llm(&mut agent, &mut messages)
            .await
            .unwrap();
        messages
    });
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(299)).await;
    assert!(calls.lock().unwrap().is_empty());
    assert_eq!(*trace.lock().unwrap(), ["A started", "blocked"]);
    let messages = guard.await.unwrap();
    assert_eq!(start.elapsed().as_secs(), 300);
    assert_eq!(
        *trace.lock().unwrap(),
        [
            "A started",
            "blocked",
            "A exited",
            "unblocked",
            "blocked",
            "unblocked"
        ]
    );
    assert_eq!(calls.lock().unwrap().len(), 1);
    assert_eq!(messages[0].kind, MessageKind::CompactionSummary);
}

#[tokio::test(start_paused = true)]
async fn preheat_handoff_stop_preserves_a_and_sends_neither_b_nor_main() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let trace = Arc::new(Mutex::new(Vec::new()));
    let (bus, _) = capture_notices();
    track_compaction_phase(&bus, &trace);
    // An empty script panics if a main request is made.
    let (main, requests) = RecordingStreamLlmProvider::new(vec![]);
    let (mut agent, mut messages) = over_budget_agent(
        Arc::new(main),
        Some(Arc::new(RecordingChatLlmProvider::new(
            "stub",
            "B summary",
            calls.clone(),
        ))),
        "main-x",
        bus,
    );
    start_preheat(
        &mut agent,
        &messages,
        SlowPreheat {
            response: None,
            trace: trace.clone(),
        },
    );
    let cancel = agent.cancel_token.clone();
    let start = tokio::time::Instant::now();
    let turn = tokio::spawn(async move {
        let result = super::super::request_retry::run_request(&mut agent, &mut messages, 1).await;
        assert!(matches!(
            result,
            Err(super::super::types::LoopError::Aborted { .. })
        ));
        agent
    });
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(10)).await;
    assert!(trace.lock().unwrap().contains(&"blocked"));
    cancel.cancel();
    let mut agent = turn.await.unwrap();
    assert_eq!(start.elapsed(), Duration::from_secs(10));
    assert!(calls.lock().unwrap().is_empty());
    assert!(requests.0.lock().unwrap().is_empty());
    let preheat = &mut agent.context_state.as_mut().unwrap().preheat;
    assert!(preheat.is_warmup_task_active());
    assert!(!trace.lock().unwrap().contains(&"A exited"));
    assert_eq!(trace.lock().unwrap().last(), Some(&"unblocked"));
    preheat.abort();
}

#[tokio::test(start_paused = true)]
async fn preheat_handoff_does_not_wait_at_99_percent_and_rechecks_after_a() {
    for (working_tokens, covered_all, a_fails, expect_b) in [
        (990, false, false, false),
        (2_000, true, false, false),
        (2_000, false, false, true),
        (2_000, true, true, true),
    ] {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let trace = Arc::new(Mutex::new(Vec::new()));
        let (bus, _) = capture_notices();
        track_compaction_phase(&bus, &trace);
        let (mut agent, mut messages) = over_budget_agent(
            Arc::new(RecordingChatLlmProvider::new(
                "stub",
                "B summary",
                calls.clone(),
            )),
            None,
            "main-x",
            bus,
        );
        let state = agent.context_state.as_mut().unwrap();
        state.context_budget_tokens = 1_000;
        state.context_budget_chars = 4_000;
        state.update_api_usage(working_tokens, 0);
        let response = if a_fails {
            Err(session_model_error())
        } else {
            Ok(ChatResponse {
                id: None,
                usage: None,
                choices: vec![crate::core::llm::ChatResponseChoice {
                    message: ChatMessage::assistant("A summary"),
                    index: 0,
                    finish_reason: Some("stop".into()),
                }],
            })
        };
        let prefix = if covered_all {
            &messages[..]
        } else {
            &messages[..1]
        };
        start_preheat(
            &mut agent,
            prefix,
            SlowPreheat {
                response: Some((Duration::from_secs(45), response)),
                trace: trace.clone(),
            },
        );
        let start = tokio::time::Instant::now();
        current_tail_guard::maybe_reduce_before_next_llm(&mut agent, &mut messages)
            .await
            .unwrap();
        assert_eq!(
            start.elapsed().as_secs(),
            if working_tokens < 1_000 { 0 } else { 45 }
        );
        assert_eq!(calls.lock().unwrap().len(), usize::from(expect_b));
        if working_tokens < 1_000 {
            assert!(!trace.lock().unwrap().contains(&"blocked"));
            agent.context_state.as_mut().unwrap().preheat.abort();
        } else if !a_fails && expect_b {
            let calls = calls.lock().unwrap();
            assert!(calls[0].message_texts[1].contains("A summary"));
            assert!(calls[0].message_texts[1].contains("[Assistant] aaaa"));
        } else if !a_fails {
            assert!(messages[0].text_content().unwrap().contains("A summary"));
        }
    }
}

#[tokio::test(start_paused = true)]
async fn preheat_handoff_also_applies_a_for_confirmed_overflow() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let trace = Arc::new(Mutex::new(Vec::new()));
    let (bus, _) = capture_notices();
    let (mut agent, mut messages) = over_budget_agent(
        Arc::new(RecordingChatLlmProvider::new(
            "stub",
            "B summary",
            calls.clone(),
        )),
        None,
        "main-x",
        bus,
    );
    start_preheat(
        &mut agent,
        &messages,
        SlowPreheat {
            response: Some((
                Duration::from_secs(3),
                Ok(ChatResponse {
                    id: None,
                    usage: None,
                    choices: vec![crate::core::llm::ChatResponseChoice {
                        index: 0,
                        message: ChatMessage::assistant("A summary"),
                        finish_reason: Some("stop".into()),
                    }],
                }),
            )),
            trace,
        },
    );
    let overflow = llm_http_status_error(
        "stub",
        400,
        r#"{"error":{"code":"context_length_exceeded"}}"#,
    );
    let start = tokio::time::Instant::now();
    let stats = super::super::error_classifier::handle_overflow_retry(
        &mut agent,
        &mut messages,
        2,
        &overflow,
    )
    .await;
    assert!(stats.applied);
    assert_eq!(start.elapsed().as_secs(), 3);
    assert!(calls.lock().unwrap().is_empty());
    assert_eq!(messages.len(), 1);
    assert!(messages[0].text_content().unwrap().contains("A summary"));
}

#[tokio::test]
async fn failed_proactive_collapse_still_sends_and_does_not_repeat_on_main_retry() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let (bus, _) = capture_notices();
    let (main, requests) = RecordingStreamLlmProvider::new(vec![
        vec![Err(session_model_error())],
        vec![
            Ok(StreamEvent::ContentDelta {
                delta: "original context answered".into(),
            }),
            Ok(StreamEvent::FinishReason {
                reason: "stop".into(),
            }),
        ],
    ]);
    let (mut agent, mut messages) = over_budget_agent(
        Arc::new(main),
        Some(Arc::new(RecordingChatLlmProvider::failing(
            "fixture",
            calls.clone(),
            compaction_billing_error,
        ))),
        "main-x",
        bus,
    );
    agent.config.retry_base_delay_ms = 0;
    let mut prompt = ChatMessage::user("current prompt");
    prompt.msg_id = Some("u2".into());
    agent.start_idx = messages.len();
    messages.push(prompt);
    let result = super::super::request_retry::run_request(&mut agent, &mut messages, 1).await;
    assert_eq!(result.unwrap().content_buf, "original context answered");
    assert_eq!(calls.lock().unwrap().len(), 1);
    let requests = requests.0.lock().unwrap();
    assert_eq!(requests.len(), 2);
    for request in requests.iter() {
        assert!(request
            .messages
            .iter()
            .any(|m| m.msg_id.as_deref() == Some("u1")));
        assert!(request
            .messages
            .iter()
            .any(|m| m.msg_id.as_deref() == Some("a1")));
        assert!(request
            .messages
            .iter()
            .any(|m| m.msg_id.as_deref() == Some("u2")));
    }
    assert!(!agent.main_request_failed);
}

#[tokio::test]
async fn collapse_falls_back_to_session_model_when_compaction_model_fails() {
    let main_calls = Arc::new(Mutex::new(Vec::new()));
    let compaction_calls = Arc::new(Mutex::new(Vec::new()));
    let (bus, notices) = capture_notices();
    let (mut agent, mut messages) = over_budget_agent(
        Arc::new(RecordingChatLlmProvider::new(
            "openai",
            "main-summary",
            Arc::clone(&main_calls),
        )),
        Some(Arc::new(RecordingChatLlmProvider::failing(
            "deepseek",
            Arc::clone(&compaction_calls),
            compaction_billing_error,
        ))),
        "compaction-x",
        bus,
    );
    current_tail_guard::maybe_reduce_before_next_llm(&mut agent, &mut messages)
        .await
        .unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].kind, MessageKind::CompactionSummary);
    assert!(messages[0].text_content().unwrap().contains("main-summary"));
    let main_calls = main_calls.lock().unwrap();
    let compaction_calls = compaction_calls.lock().unwrap();
    assert_eq!(main_calls.len(), 1);
    assert_eq!(main_calls[0].model, "main-x");
    assert_eq!(compaction_calls.len(), 1);
    assert_eq!(compaction_calls[0].model, "compaction-x");
    assert_eq!(
        main_calls[0].message_texts, compaction_calls[0].message_texts,
        "fallback summarizes the same working set"
    );
    let notices = notices.lock().unwrap();
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0]["finishReason"], "compaction_model_fallback");
    let text = notices[0]["message"].as_str().unwrap();
    for expected in ["compaction-x", "main-x", "402"] {
        assert!(text.contains(expected), "{text}");
    }
}

#[tokio::test]
async fn collapse_does_not_retry_when_compaction_model_is_session_model() {
    let main_calls = Arc::new(Mutex::new(Vec::new()));
    let (bus, notices) = capture_notices();
    let (mut agent, mut messages) = over_budget_agent(
        Arc::new(RecordingChatLlmProvider::failing(
            "deepseek",
            Arc::clone(&main_calls),
            compaction_billing_error,
        )),
        None,
        "main-x",
        bus,
    );
    let mut generation_failed = false;
    current_tail_guard::prepare_request_context(&mut agent, &mut messages, &mut generation_failed)
        .await
        .unwrap();
    assert!(generation_failed);
    assert_eq!(main_calls.lock().unwrap().len(), 1);
    assert!(notices.lock().unwrap().is_empty());
    assert_eq!(
        messages.len(),
        2,
        "failed summary must leave the working set intact"
    );
}

#[tokio::test]
async fn collapse_fails_open_when_compaction_and_fallback_generation_fail() {
    let main_calls = Arc::new(Mutex::new(Vec::new()));
    let compaction_calls = Arc::new(Mutex::new(Vec::new()));
    let (bus, notices) = capture_notices();
    let (mut agent, mut messages) = over_budget_agent(
        Arc::new(RecordingChatLlmProvider::failing(
            "openai",
            Arc::clone(&main_calls),
            session_model_error,
        )),
        Some(Arc::new(RecordingChatLlmProvider::failing(
            "deepseek",
            Arc::clone(&compaction_calls),
            compaction_billing_error,
        ))),
        "compaction-x",
        bus,
    );
    let mut generation_failed = false;
    current_tail_guard::prepare_request_context(&mut agent, &mut messages, &mut generation_failed)
        .await
        .unwrap();
    assert!(generation_failed);
    assert_eq!(main_calls.lock().unwrap().len(), 1);
    assert_eq!(compaction_calls.lock().unwrap().len(), 1);
    assert_eq!(notices.lock().unwrap().len(), 1);
    assert_eq!(messages.len(), 2);
}

#[tokio::test]
async fn collapse_summary_uses_compaction_provider_cross_provider() {
    let main_calls = Arc::new(Mutex::new(Vec::<RecordedChatCall>::new()));
    let compaction_calls = Arc::new(Mutex::new(Vec::<RecordedChatCall>::new()));
    let main_provider: Arc<dyn LlmProvider> = Arc::new(RecordingChatLlmProvider::new(
        "openai",
        "main-summary",
        Arc::clone(&main_calls),
    ));
    let compaction_provider: Arc<dyn LlmProvider> = Arc::new(RecordingChatLlmProvider::new(
        "deepseek",
        "compaction-summary",
        Arc::clone(&compaction_calls),
    ));

    let mut agent = AgentLoop::new(
        test_binding(Arc::clone(&main_provider), "main-x"),
        Arc::new(MockPrimitiveExecutor),
        Arc::new(DefaultEventBus::new()),
        AgentLoopConfig {
            session_id: "sess-current-tail-routing".to_string(),
            context_config: ContextConfig {
                compaction_model: "compaction-x".to_string(),
                ..Default::default()
            },
            compaction_provider: Some(Arc::clone(&compaction_provider)),
            ..Default::default()
        },
        CancellationToken::new(),
    );
    agent.start_idx = 0;

    let mut user = ChatMessage::user("u".repeat(4_000));
    user.msg_id = Some("u1".to_string());
    let mut assistant = ChatMessage::assistant("a".repeat(4_000));
    assistant.msg_id = Some("a1".to_string());
    let tail_chars = user.text_content().unwrap().len() + assistant.text_content().unwrap().len();
    let mut messages = vec![user, assistant];

    agent.set_context_state(Some(ContextState {
        messages: vec![],
        estimate_context_chars: tail_chars,
        context_budget_chars: 200,
        context_budget_tokens: 50,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        latest_plan_event: None,
        resume_control: Default::default(),
        preheat: Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
        hydrated: true,
    }));

    current_tail_guard::maybe_reduce_before_next_llm(&mut agent, &mut messages)
        .await
        .unwrap();

    let main_calls = main_calls.lock().unwrap().clone();
    let compaction_calls = compaction_calls.lock().unwrap().clone();
    assert!(main_calls.is_empty(), "collapse summary 不应走主 provider");
    assert_eq!(compaction_calls.len(), 1);
    assert_eq!(compaction_calls[0].provider, "deepseek");
    assert_eq!(compaction_calls[0].model, "compaction-x");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].kind, MessageKind::CompactionSummary);
}
