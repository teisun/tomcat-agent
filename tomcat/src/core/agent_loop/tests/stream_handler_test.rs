//! # `stream_handler` 焦点测试
//!
//! 直接打 `run_chat_stream`，验证流尾语义不会在 `FinishReason` 处提前截断。

use std::io::Write;
use std::sync::Arc;
use std::sync::Mutex;

use tokio_util::sync::CancellationToken;
use tracing_subscriber::fmt;

use crate::core::agent_loop::stream_handler::{extract_path_from_partial_args, run_chat_stream};
use crate::core::agent_loop::{AgentLoop, AgentLoopConfig, LoopError};
use crate::core::llm::{ChatMessage, ChatMessageContentPart, ChatRequest, StreamEvent};
use crate::core::session::manager::ContextState;
use crate::infra::error::AppError;
use crate::infra::{wire, DefaultEventBus, EventBus};

use super::mocks::{test_binding, MockLlmProvider, MockPrimitiveExecutor};

fn make_agent(streams: Vec<Vec<Result<StreamEvent, AppError>>>) -> AgentLoop {
    make_agent_with_bus(streams).0
}

fn make_agent_with_bus(
    streams: Vec<Vec<Result<StreamEvent, AppError>>>,
) -> (AgentLoop, Arc<DefaultEventBus>) {
    let llm = Arc::new(MockLlmProvider::new(streams));
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());
    let config = AgentLoopConfig {
        session_id: "s-stream-handler".to_string(),
        ..Default::default()
    };
    (
        AgentLoop::new(
            test_binding(llm, "gpt-4"),
            primitive,
            event_bus.clone(),
            config,
            CancellationToken::new(),
        ),
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
        resume_control: Default::default(),
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
        resolved_output_limit: None,
        diagnostic_request_id: None,
        stream: Some(true),
        model_override: None,
        thinking_level: None,
        cache_key: None,
        tools: None,
    }
}

#[derive(Clone)]
struct SharedLogWriter(Arc<Mutex<Vec<u8>>>);

impl Write for SharedLogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
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
            cache_read_tokens: Some(100),
            cache_write_tokens: Some(23),
            total_tokens: Some(168),
            reasoning_tokens: Some(30),
            text_tokens: Some(15),
        }),
    ];
    let mut agent = make_agent(vec![stream]);
    agent.set_context_state(Some(make_context_state()));

    let outcome = run_chat_stream(&mut agent, make_request(), 1, 4)
        .await
        .expect("stream_handler should consume trailing usage");

    assert_eq!(outcome.content_buf, "hello");
    assert_eq!(
        outcome
            .usage
            .as_ref()
            .and_then(|usage| usage.cache_read_tokens),
        Some(100)
    );
    assert_eq!(
        outcome
            .usage
            .as_ref()
            .and_then(|usage| usage.reasoning_tokens),
        Some(30)
    );
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
async fn run_chat_stream_empty_llm_error_returns_err_and_emits_structured_error() {
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

    let err = match run_chat_stream(&mut agent, make_request(), 1, 4).await {
        Ok(_) => panic!("empty llm error should be promoted to Err"),
        Err(err) => err,
    };

    assert!(matches!(err, LoopError::Retryable(_)));
    assert!(
        observed.lock().unwrap().iter().any(|event| {
            event["errorMessage"].as_str() == Some("boom")
                && event["errorCode"].as_str() == Some("server_error")
        }),
        "terminal failures must remain visible even when no deltas preceded them"
    );
}

#[tokio::test]
async fn run_chat_stream_with_text_and_llm_error_discards_partial_output_and_retries() {
    let stream = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "partial".to_string(),
        }),
        Ok(StreamEvent::LlmError {
            reason: "error:server_error".to_string(),
            message: "boom".to_string(),
            code: Some("server_error".to_string()),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "error:server_error".to_string(),
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

    let err = match run_chat_stream(&mut agent, make_request(), 1, 4).await {
        Ok(_) => panic!("a terminal LLM error must discard partial output and retry"),
        Err(err) => err,
    };
    assert!(matches!(err, LoopError::Retryable(_)));
    let observed = observed.lock().unwrap();
    assert_eq!(observed.len(), 1);
    assert!(
        observed[0]["errorMessage"]
            .as_str()
            .is_some_and(|message| message.contains("boom") && message.contains("已丢弃 7 字")),
        "the visible error must disclose discarded partial output: {:?}",
        observed[0]
    );
    assert_eq!(observed[0]["errorCode"].as_str(), Some("server_error"));
}

#[tokio::test]
async fn run_chat_stream_with_tool_calls_and_llm_error_discards_partial_calls_and_retries() {
    let stream = vec![
        Ok(StreamEvent::ToolCallDelta {
            index: 0,
            id: Some("call_1".to_string()),
            name: Some("write".to_string()),
            arguments_delta: Some(r#"{"path":"/tmp/a.txt","content":"x"}"#.to_string()),
        }),
        Ok(StreamEvent::LlmError {
            reason: "error:server_error".to_string(),
            message: "boom".to_string(),
            code: Some("server_error".to_string()),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "tool_calls".to_string(),
        }),
    ];
    let (mut agent, bus) = make_agent_with_bus(vec![stream]);
    let observed: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&observed);
    let _listener = bus.on(
        wire::WIRE_LLM_ERROR,
        Box::new(move |ctx| {
            sink.lock().unwrap().push(ctx.payload);
            Ok(())
        }),
    );

    let err = match run_chat_stream(&mut agent, make_request(), 1, 4).await {
        Ok(_) => panic!("a terminal LLM error must discard partial tool calls"),
        Err(err) => err,
    };
    assert!(matches!(err, LoopError::Retryable(_)));
    assert_eq!(observed.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn run_chat_stream_warn_log_never_contains_base64_payload() {
    let stream = vec![
        Ok(StreamEvent::LlmError {
            reason: "error:server_error".to_string(),
            message: "boom".to_string(),
            code: Some("server_error".to_string()),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "error:server_error".to_string(),
        }),
    ];
    let mut agent = make_agent(vec![stream]);
    let pdf_sentinel = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        b"UNIQUE_LOG_SENTINEL_PDF",
    );
    let req = ChatRequest {
        messages: vec![ChatMessage::user_with_parts(vec![
            ChatMessageContentPart::text("hi"),
            ChatMessageContentPart::file_base64_data("brief.pdf", "application/pdf", &pdf_sentinel)
                .expect("build inline pdf part"),
        ])],
        ..make_request()
    };

    let logs = Arc::new(Mutex::new(Vec::new()));
    let subscriber = fmt()
        .with_ansi(false)
        .without_time()
        .with_writer({
            let logs = logs.clone();
            move || SharedLogWriter(logs.clone())
        })
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let _ = run_chat_stream(&mut agent, req, 1, 4).await;

    let rendered = String::from_utf8(logs.lock().unwrap().clone()).expect("utf8 logs");
    assert!(
        rendered.contains("input_file"),
        "形状日志应只暴露 part 类型：{}",
        rendered
    );
    assert!(
        !rendered.contains(&pdf_sentinel),
        "warn 日志不应包含 base64 原文：{}",
        rendered
    );
    assert!(
        !rendered.contains("data:application/pdf;base64"),
        "warn 日志不应包含 data url 前缀：{}",
        rendered
    );
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

    let outcome = run_chat_stream(&mut agent, make_request(), 1, 4)
        .await
        .expect("stream_handler should emit llm notice");

    assert_eq!(outcome.finish_reason.as_deref(), Some("max_output_tokens"));
    let observed = observed.lock().unwrap().clone();
    assert_eq!(
        observed,
        vec![
            wire::WIRE_MESSAGE_END.to_string(),
            wire::WIRE_LLM_NOTICE.to_string()
        ]
    );
}

#[test]
fn extract_path_from_partial_args_handles_complete_and_partial_json() {
    assert_eq!(
        extract_path_from_partial_args(r#"{"path":"/tmp/demo.txt","content":"hello"}"#),
        Some("/tmp/demo.txt".to_string())
    );
    assert_eq!(
        extract_path_from_partial_args(r#"{"path":"/tmp/demo.txt","content":"AAAA""#),
        Some("/tmp/demo.txt".to_string())
    );
    assert_eq!(
        extract_path_from_partial_args(r#"{"content":"hello"}"#),
        None
    );
}

#[tokio::test]
async fn run_chat_stream_emits_tool_call_streaming_for_write_once() {
    let stream = vec![
        Ok(StreamEvent::ToolCallDelta {
            index: 0,
            id: Some("call_write".to_string()),
            name: Some("write".to_string()),
            arguments_delta: Some(r#"{"path":"/tmp/demo.txt","#.to_string()),
        }),
        Ok(StreamEvent::ToolCallDelta {
            index: 0,
            id: None,
            name: None,
            arguments_delta: Some(r#""content":"hello world"}"#.to_string()),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "tool_calls".to_string(),
        }),
    ];
    let (mut agent, bus) = make_agent_with_bus(vec![stream]);
    let observed: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&observed);
    let _listener = bus.on(
        wire::WIRE_TOOL_CALL_STREAMING,
        Box::new(move |ctx| {
            sink.lock().unwrap().push(ctx.payload);
            Ok(())
        }),
    );

    let outcome = run_chat_stream(&mut agent, make_request(), 1, 4)
        .await
        .expect("stream_handler should emit tool_call_streaming");

    assert_eq!(outcome.finish_reason.as_deref(), Some("tool_calls"));
    let observed = observed.lock().unwrap();
    assert_eq!(observed.len(), 1, "write should announce exactly once");
    assert_eq!(observed[0]["toolCallId"].as_str(), Some("call_write"));
    assert_eq!(observed[0]["toolName"].as_str(), Some("write"));
}

#[tokio::test]
async fn run_chat_stream_tool_call_streaming_carries_path_preview_when_available() {
    let stream = vec![
        Ok(StreamEvent::ToolCallDelta {
            index: 0,
            id: Some("call_edit".to_string()),
            name: Some("edit".to_string()),
            arguments_delta: Some(r#"{"path":"~/workspace/demo.txt","#.to_string()),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "tool_calls".to_string(),
        }),
    ];
    let (mut agent, bus) = make_agent_with_bus(vec![stream]);
    let observed: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&observed);
    let _listener = bus.on(
        wire::WIRE_TOOL_CALL_STREAMING,
        Box::new(move |ctx| {
            sink.lock().unwrap().push(ctx.payload);
            Ok(())
        }),
    );

    run_chat_stream(&mut agent, make_request(), 1, 4)
        .await
        .expect("stream_handler should emit tool_call_streaming with preview");

    let observed = observed.lock().unwrap();
    assert_eq!(observed.len(), 1);
    assert_eq!(
        observed[0]["argsPreview"]["path"].as_str(),
        Some("~/workspace/demo.txt")
    );
}

#[tokio::test]
async fn run_chat_stream_tool_call_streaming_skips_small_tools() {
    let stream = vec![
        Ok(StreamEvent::ToolCallDelta {
            index: 0,
            id: Some("call_read".to_string()),
            name: Some("read".to_string()),
            arguments_delta: Some(r#"{"path":"/tmp/a.txt"}"#.to_string()),
        }),
        Ok(StreamEvent::ToolCallDelta {
            index: 1,
            id: Some("call_bash".to_string()),
            name: Some("bash".to_string()),
            arguments_delta: Some(r#"{"command":"echo hi"}"#.to_string()),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "tool_calls".to_string(),
        }),
    ];
    let (mut agent, bus) = make_agent_with_bus(vec![stream]);
    let observed: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&observed);
    let _listener = bus.on(
        wire::WIRE_TOOL_CALL_STREAMING,
        Box::new(move |ctx| {
            sink.lock().unwrap().push(ctx.payload);
            Ok(())
        }),
    );

    run_chat_stream(&mut agent, make_request(), 1, 4)
        .await
        .expect("small tools should not emit streaming preview");

    assert!(
        observed.lock().unwrap().is_empty(),
        "read/bash should not emit tool_call_streaming"
    );
}
