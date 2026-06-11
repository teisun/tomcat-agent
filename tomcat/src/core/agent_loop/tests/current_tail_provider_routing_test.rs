use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;

use super::super::current_tail_guard;
use super::super::{AgentLoop, AgentLoopConfig};
use super::mocks::{MockPrimitiveExecutor, RecordedChatCall, RecordingChatLlmProvider};
use crate::core::compaction::preheat::Preheat;
use crate::core::llm::{ChatMessage, LlmProvider, MessageKind};
use crate::core::session::manager::ContextState;
use crate::infra::config::ContextConfig;
use crate::infra::event_bus::DefaultEventBus;

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
        Arc::clone(&main_provider),
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
    agent.start_idx = 1;
    agent.context_tail_start = 1;

    let system = ChatMessage::system("sys");
    let mut user = ChatMessage::user("u".repeat(4_000));
    user.msg_id = Some("u1".to_string());
    let mut assistant = ChatMessage::assistant("a".repeat(4_000));
    assistant.msg_id = Some("a1".to_string());
    let tail_chars = user.text_content().unwrap().len() + assistant.text_content().unwrap().len();
    let mut messages = vec![system, user, assistant];

    agent.set_context_state(Some(ContextState {
        messages: vec![],
        estimate_context_chars: tail_chars,
        context_budget_chars: 200,
        context_budget_tokens: 50,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        latest_plan_event: None,
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
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[1].kind, MessageKind::CompactionSummary);
}
