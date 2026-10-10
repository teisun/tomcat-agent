use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;

use super::super::current_tail_guard;
use super::super::{AgentLoop, AgentLoopConfig};
use super::mocks::{
    test_binding, MockPrimitiveExecutor, RecordedChatCall, RecordingChatLlmProvider,
};
use crate::core::compaction::preheat::Preheat;
use crate::core::llm::{ChatMessage, LlmProvider, MessageKind};
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
    }));
    (agent, messages)
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
    let error = current_tail_guard::maybe_reduce_before_next_llm(&mut agent, &mut messages)
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), compaction_billing_error().to_string());
    assert_eq!(main_calls.lock().unwrap().len(), 1);
    assert!(notices.lock().unwrap().is_empty());
    assert_eq!(
        messages.len(),
        2,
        "failed summary must leave the working set intact"
    );
}

#[tokio::test]
async fn collapse_returns_session_model_error_when_fallback_also_fails() {
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
    let error = current_tail_guard::maybe_reduce_before_next_llm(&mut agent, &mut messages)
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), session_model_error().to_string());
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
