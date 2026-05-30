//! # `stream_handler` 焦点测试
//!
//! 直接打 `run_chat_stream`，验证流尾语义不会在 `FinishReason` 处提前截断。

use std::sync::Arc;
use std::sync::Mutex;

use tokio_util::sync::CancellationToken;

use crate::core::agent_loop::stream_handler::run_chat_stream;
use crate::core::agent_loop::{AgentLoop, AgentLoopConfig};
use crate::core::llm::{ChatMessage, ChatRequest, StreamEvent};
use crate::core::session::manager::ContextState;
use crate::infra::error::AppError;
use crate::infra::{wire, DefaultEventBus, EventBus};

use super::mocks::{MockLlmProvider, MockPrimitiveExecutor};

fn make_agent(streams: Vec<Vec<Result<StreamEvent, AppError>>>) -> AgentLoop {
    make_agent_with_bus(streams).0
}

fn make_agent_with_bus(streams: Vec<Vec<Result<StreamEvent, AppError>>>) -> (AgentLoop, Arc<DefaultEventBus>) {
    let llm = Arc::new(MockLlmProvider::new(streams));
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());
    let config = AgentLoopConfig {
        model: "gpt-4".to_string(),
        session_id: "s-stream-handler".to_string(),
        ..Default::default()
    };
    (
        AgentLoop::new(llm, primitive, event_bus.clone(), config, CancellationToken::new()),
        event_bus,
    )
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

#[tokio::test]
async fn run_chat_stream_surfaces_structured_llm_error() {
    let stream = vec![
        Ok(StreamEvent::LlmError {
            reason: "error:boom".to_string(),
            message: "boom".to_string(),
            code: Some("server_error".to_string()),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "error:boom".to_string(),
        }),
    ];
    let (mut agent, bus) = make_agent_with_bus(vec![stream]);
    agent.set_context_state(Some(make_context_state()));
    let observed: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&observed);
    let _listener = bus.on(
        wire::WIRE_LLM_ERROR,
        Box::new(move |ctx| {
            sink.lock().unwrap().push(ctx.payload);
            Ok(())
        }),
    );

    let outcome = run_chat_stream(&mut agent, make_request())
        .await
        .expect("stream_handler should preserve structured llm error");

    assert_eq!(outcome.finish_reason.as_deref(), Some("error:boom"));
    assert_eq!(outcome.error_message.as_deref(), Some("boom"));
    assert_eq!(outcome.error_code.as_deref(), Some("server_error"));
    let observed = observed.lock().unwrap();
    assert_eq!(observed.len(), 1);
    assert_eq!(observed[0]["errorMessage"].as_str(), Some("boom"));
    assert_eq!(observed[0]["errorCode"].as_str(), Some("server_error"));
}

#[tokio::test]
async fn run_chat_stream_emits_llm_notice_after_message_end() {
    let stream = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "hello".to_string(),
        }),
        Ok(StreamEvent::LlmNotice {
            finish_reason: "max_output_tokens".to_string(),
            message: "达到 max_output_tokens，回答可能未完成".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "max_output_tokens".to_string(),
        }),
    ];
    let (mut agent, bus) = make_agent_with_bus(vec![stream]);
    agent.set_context_state(Some(make_context_state()));
    let observed: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    for wire_name in [wire::WIRE_MESSAGE_END, wire::WIRE_LLM_NOTICE] {
        let sink = Arc::clone(&observed);
        let name = wire_name.to_string();
        let _listener = bus.on(
            wire_name,
            Box::new(move |_ctx| {
                sink.lock().unwrap().push(name.clone());
                Ok(())
            }),
        );
        let _ = _listener;
    }

    let outcome = run_chat_stream(&mut agent, make_request())
        .await
        .expect("stream_handler should emit llm notice");

    assert_eq!(outcome.finish_reason.as_deref(), Some("max_output_tokens"));
    let observed = observed.lock().unwrap().clone();
    assert_eq!(
        observed,
        vec![wire::WIRE_MESSAGE_END.to_string(), wire::WIRE_LLM_NOTICE.to_string()]
    );
}
