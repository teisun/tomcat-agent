//! # `stream_handler` 焦点测试
//!
//! 直接打 `run_chat_stream`，验证流尾语义不会在 `FinishReason` 处提前截断。

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::core::agent_loop::stream_handler::run_chat_stream;
use crate::core::agent_loop::{AgentLoop, AgentLoopConfig};
use crate::core::llm::{ChatMessage, ChatRequest, StreamEvent};
use crate::core::session::manager::ContextState;
use crate::infra::error::AppError;
use crate::infra::DefaultEventBus;

use super::mocks::{MockLlmProvider, MockPrimitiveExecutor};

fn make_agent(streams: Vec<Vec<Result<StreamEvent, AppError>>>) -> AgentLoop {
    let llm = Arc::new(MockLlmProvider::new(streams));
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());
    let config = AgentLoopConfig {
        model: "gpt-4".to_string(),
        session_id: "s-stream-handler".to_string(),
        ..Default::default()
    };
    AgentLoop::new(llm, primitive, event_bus, config, CancellationToken::new())
}

fn make_context_state() -> ContextState {
    ContextState {
        messages: Vec::new(),
        estimate_context_chars: 0,
        context_budget_chars: 100_000,
        context_budget_tokens: 25_000,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: std::path::PathBuf::new(),
        latest_plan_event: None,
        preheat: crate::core::compaction::preheat::Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    }
}

fn make_request() -> ChatRequest {
    ChatRequest {
        messages: vec![ChatMessage::user("hi")],
        model: "gpt-4".to_string(),
        temperature: None,
        max_tokens: None,
        stream: Some(true),
        model_override: None,
        tools: None,
    }
}

#[tokio::test]
async fn run_chat_stream_preserves_finish_reason_and_trailing_usage() {
    let stream = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "hello".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
        Ok(StreamEvent::Usage {
            prompt_tokens: 123,
            completion_tokens: 45,
            total_tokens: Some(168),
        }),
    ];
    let mut agent = make_agent(vec![stream]);
    agent.set_context_state(Some(make_context_state()));

    let outcome = run_chat_stream(&mut agent, make_request())
        .await
        .expect("stream_handler should consume trailing usage");

    assert_eq!(outcome.content_buf, "hello");
    assert!(outcome.tool_calls_buf.is_empty());
    assert!(!outcome.aborted);
    assert_eq!(outcome.finish_reason.as_deref(), Some("stop"));

    let ctx = agent
        .take_context_state()
        .expect("context_state should remain attached");
    let usage = ctx
        .last_api_usage
        .expect("trailing Usage after FinishReason must still update last_api_usage");
    assert_eq!(usage.prompt_tokens, 123);
    assert_eq!(usage.completion_tokens, 45);
}
