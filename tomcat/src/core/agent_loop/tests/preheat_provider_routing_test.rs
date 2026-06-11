use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use super::super::turn_finalize::finalize_turn_after_text;
use super::super::{AgentLoop, AgentLoopConfig};
use super::mocks::{MockPrimitiveExecutor, RecordedChatCall, RecordingChatLlmProvider};
use crate::core::compaction::preheat::{Preheat, PreheatOutcome};
use crate::core::llm::{ChatMessage, ChatRequest, ChatResponse, LlmProvider, StreamEvent};
use crate::core::session::manager::ContextState;
use crate::infra::config::ContextConfig;
use crate::infra::error::AppError;
use crate::infra::event_bus::{DefaultEventBus, EventBus};

struct FailingChatProvider;

#[async_trait]
impl LlmProvider for FailingChatProvider {
    fn provider_name(&self) -> &str {
        "failing"
    }

    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, AppError> {
        Err(AppError::Llm("expected failure".to_string()))
    }

    async fn chat_stream(
        &self,
        _request: ChatRequest,
    ) -> Result<
        Box<dyn tokio_stream::Stream<Item = Result<StreamEvent, AppError>> + Send + Unpin>,
        AppError,
    > {
        Ok(Box::new(tokio_stream::iter(Vec::<
            Result<StreamEvent, AppError>,
        >::new())))
    }

    fn count_tokens(&self, _messages: &[ChatMessage]) -> Result<u32, AppError> {
        Ok(0)
    }
}

fn user_message(id: &str, text: &str) -> ChatMessage {
    let mut msg = ChatMessage::user(text);
    msg.msg_id = Some(id.to_string());
    msg.timestamp = Some("2026-06-11T00:00:00Z".to_string());
    msg
}

fn assistant_message(id: &str, text: &str) -> ChatMessage {
    let mut msg = ChatMessage::assistant(text);
    msg.msg_id = Some(id.to_string());
    msg.timestamp = Some("2026-06-11T00:00:01Z".to_string());
    msg
}

fn build_context_state(messages: Vec<ChatMessage>) -> ContextState {
    ContextState {
        messages,
        estimate_context_chars: 70,
        context_budget_chars: 100,
        context_budget_tokens: 25,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        latest_plan_event: None,
        preheat: Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    }
}

fn build_agent(
    main_provider: Arc<dyn LlmProvider>,
    compaction_provider: Option<Arc<dyn LlmProvider>>,
    event_bus: Arc<dyn EventBus>,
) -> (AgentLoop, Vec<ChatMessage>) {
    let context_messages = vec![
        user_message("u1", "summarize this conversation"),
        assistant_message("a1", "working on it"),
    ];
    let messages = context_messages.clone();
    let mut agent = AgentLoop::new(
        main_provider,
        Arc::new(MockPrimitiveExecutor),
        event_bus,
        AgentLoopConfig {
            session_id: "sess-preheat-routing".to_string(),
            context_config: ContextConfig {
                compaction_model: "compaction-x".to_string(),
                ..Default::default()
            },
            compaction_provider,
            ..Default::default()
        },
        CancellationToken::new(),
    );
    agent.set_context_state(Some(build_context_state(context_messages)));
    (agent, messages)
}

async fn assert_preheat_completed(agent: &mut AgentLoop) {
    let outcome = agent
        .context_state
        .as_mut()
        .unwrap()
        .preheat
        .await_result(Duration::from_secs(1))
        .await;
    assert!(
        matches!(outcome, PreheatOutcome::Completed(_)),
        "timing ⑤ 应启动并完成 preheat"
    );
}

async fn run_timing5_try_start_case(
    main_provider_name: &'static str,
    compaction_provider_name: &'static str,
    inject_compaction_provider: bool,
) -> (Vec<RecordedChatCall>, Vec<RecordedChatCall>) {
    let main_calls = Arc::new(Mutex::new(Vec::<RecordedChatCall>::new()));
    let compaction_calls = Arc::new(Mutex::new(Vec::<RecordedChatCall>::new()));
    let main_provider: Arc<dyn LlmProvider> = Arc::new(RecordingChatLlmProvider::new(
        main_provider_name,
        "main-summary",
        Arc::clone(&main_calls),
    ));
    let dedicated_compaction_provider: Arc<dyn LlmProvider> =
        Arc::new(RecordingChatLlmProvider::new(
            compaction_provider_name,
            "compaction-summary",
            Arc::clone(&compaction_calls),
        ));
    let event_bus: Arc<dyn EventBus> = Arc::new(DefaultEventBus::new());
    let (mut agent, mut messages) = build_agent(
        Arc::clone(&main_provider),
        inject_compaction_provider.then_some(Arc::clone(&dedicated_compaction_provider)),
        event_bus,
    );

    finalize_turn_after_text(
        &mut agent,
        &mut messages,
        "assistant tail reply",
        1,
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .unwrap();
    assert_preheat_completed(&mut agent).await;

    let main_calls = main_calls.lock().unwrap().clone();
    let compaction_calls = compaction_calls.lock().unwrap().clone();
    (main_calls, compaction_calls)
}

#[tokio::test]
async fn timing5_try_start_uses_compaction_provider_same_provider() {
    let (main_calls, compaction_calls) = run_timing5_try_start_case("openai", "openai", true).await;

    assert!(
        main_calls.is_empty(),
        "主 provider 不应收到 compaction 请求"
    );
    assert_eq!(compaction_calls.len(), 1);
    assert_eq!(compaction_calls[0].provider, "openai");
    assert_eq!(compaction_calls[0].model, "compaction-x");
}

#[tokio::test]
async fn timing5_try_start_uses_compaction_provider_main_deepseek_compaction_openai() {
    let (main_calls, compaction_calls) =
        run_timing5_try_start_case("deepseek", "openai", true).await;

    assert!(
        main_calls.is_empty(),
        "DeepSeek 主 provider 不应收到压缩模型调用"
    );
    assert_eq!(compaction_calls.len(), 1);
    assert_eq!(compaction_calls[0].provider, "openai");
    assert_eq!(compaction_calls[0].model, "compaction-x");
}

#[tokio::test]
async fn timing5_try_start_uses_compaction_provider_main_openai_compaction_deepseek() {
    let (main_calls, compaction_calls) =
        run_timing5_try_start_case("openai", "deepseek", true).await;

    assert!(
        main_calls.is_empty(),
        "OpenAI 主 provider 不应收到压缩模型调用"
    );
    assert_eq!(compaction_calls.len(), 1);
    assert_eq!(compaction_calls[0].provider, "deepseek");
    assert_eq!(compaction_calls[0].model, "compaction-x");
}

#[tokio::test]
async fn timing5_try_start_falls_back_to_main_provider_when_compaction_provider_absent() {
    let (main_calls, compaction_calls) =
        run_timing5_try_start_case("deepseek", "openai", false).await;

    assert!(
        compaction_calls.is_empty(),
        "未注入 compaction provider 时不应命中独立 provider"
    );
    assert_eq!(main_calls.len(), 1);
    assert_eq!(main_calls[0].provider, "deepseek");
    assert_eq!(main_calls[0].model, "compaction-x");
}

#[tokio::test]
async fn timing5_try_restart_uses_compaction_provider_after_exhausted_pending() {
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
    let event_bus: Arc<dyn EventBus> = Arc::new(DefaultEventBus::new());
    let (mut agent, mut messages) = build_agent(
        Arc::clone(&main_provider),
        Some(Arc::clone(&compaction_provider)),
        Arc::clone(&event_bus),
    );

    {
        let ctx_state = agent.context_state.as_mut().unwrap();
        let failing_provider: Arc<dyn LlmProvider> = Arc::new(FailingChatProvider);
        assert!(ctx_state.preheat.try_start(
            0.95,
            &ctx_state.messages,
            &ctx_state.transcript_path,
            failing_provider,
            &agent.config.context_config,
            Arc::clone(&event_bus),
        ));
    }
    tokio::time::sleep(Duration::from_millis(1_700)).await;
    let exhausted = agent.context_state.as_mut().unwrap().preheat.poll_result();
    assert!(matches!(exhausted, PreheatOutcome::Exhausted));

    finalize_turn_after_text(
        &mut agent,
        &mut messages,
        "assistant tail reply",
        2,
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .unwrap();
    assert_preheat_completed(&mut agent).await;

    let main_calls = main_calls.lock().unwrap().clone();
    let compaction_calls = compaction_calls.lock().unwrap().clone();
    assert!(
        main_calls.is_empty(),
        "restart 也必须命中 compaction provider"
    );
    assert_eq!(compaction_calls.len(), 1);
    assert_eq!(compaction_calls[0].provider, "deepseek");
    assert_eq!(compaction_calls[0].model, "compaction-x");
}
