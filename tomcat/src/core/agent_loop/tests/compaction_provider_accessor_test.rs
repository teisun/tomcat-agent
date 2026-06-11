use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;

use super::super::{AgentLoop, AgentLoopConfig};
use super::mocks::{MockPrimitiveExecutor, RecordingChatLlmProvider};
use crate::core::llm::LlmProvider;
use crate::infra::event_bus::DefaultEventBus;

#[test]
fn returns_configured_compaction_provider_when_present() {
    let main_provider: Arc<dyn LlmProvider> = Arc::new(RecordingChatLlmProvider::new(
        "openai",
        "main",
        Arc::new(Mutex::new(vec![])),
    ));
    let compaction_provider: Arc<dyn LlmProvider> = Arc::new(RecordingChatLlmProvider::new(
        "openai",
        "compaction",
        Arc::new(Mutex::new(vec![])),
    ));
    let agent = AgentLoop::new(
        Arc::clone(&main_provider),
        Arc::new(MockPrimitiveExecutor),
        Arc::new(DefaultEventBus::new()),
        AgentLoopConfig {
            compaction_provider: Some(Arc::clone(&compaction_provider)),
            ..Default::default()
        },
        CancellationToken::new(),
    );

    let resolved = agent.compaction_provider();
    assert!(Arc::ptr_eq(&resolved, &compaction_provider));
    assert!(!Arc::ptr_eq(&resolved, &main_provider));
}

#[test]
fn falls_back_to_main_provider_when_absent() {
    let main_provider: Arc<dyn LlmProvider> = Arc::new(RecordingChatLlmProvider::new(
        "deepseek",
        "main",
        Arc::new(Mutex::new(vec![])),
    ));
    let agent = AgentLoop::new(
        Arc::clone(&main_provider),
        Arc::new(MockPrimitiveExecutor),
        Arc::new(DefaultEventBus::new()),
        AgentLoopConfig::default(),
        CancellationToken::new(),
    );

    let resolved = agent.compaction_provider();
    assert!(Arc::ptr_eq(&resolved, &main_provider));
}
