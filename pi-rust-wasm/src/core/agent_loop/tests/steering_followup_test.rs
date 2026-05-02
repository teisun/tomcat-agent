//! # Steering / FollowUp 时序测试
//!
//! - **Steering**：第 1 个工具执行后注入 steering，第 2 个工具不应执行；
//!   下一轮 LLM 收到 steering 后返回文本，且实际 read_count == 1。
//!   覆盖 `tool_dispatcher::run_tool_calls` 内 steering 注入抢占点。
//! - **FollowUp**：run 前先 `follow_up("next")`，同一上下文继续 attempt 第二轮，
//!   final_text 含两轮回复中的第二轮内容。覆盖 `run.rs::run` 第一层
//!   Conversation Loop 的 follow_up_queue drain 分支。

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::core::agent_loop::{AgentLoop, AgentLoopConfig};
use crate::core::llm::{ChatMessage, StreamEvent};
use crate::infra::error::AppError;
use crate::infra::DefaultEventBus;

use super::mocks::{MockLlmProvider, MockPrimitiveExecutor, SteerableMockPrimitive};

/// Steering：第 1 个工具执行后注入 steering，第 2 个工具不执行，下一轮 LLM 收到 steering 后返回文本。
#[tokio::test]
async fn run_steering_skips_remaining_tools() {
    let stream_tools: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ToolCallDelta {
            index: 0,
            id: Some("c1".to_string()),
            name: Some("read_file".to_string()),
            arguments_delta: Some(r#"{"path":"/a"}"#.to_string()),
        }),
        Ok(StreamEvent::ToolCallDelta {
            index: 1,
            id: Some("c2".to_string()),
            name: Some("read_file".to_string()),
            arguments_delta: Some(r#"{"path":"/b"}"#.to_string()),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "tool_calls".to_string(),
        }),
    ];
    let stream_text: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "steered".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let llm = Arc::new(MockLlmProvider::new(vec![stream_tools, stream_text]));
    let steering_queue = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let read_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let primitive = Arc::new(SteerableMockPrimitive {
        steering_queue: Arc::clone(&steering_queue),
        read_count: Arc::clone(&read_count),
    });
    let event_bus = Arc::new(DefaultEventBus::new());
    let config = AgentLoopConfig {
        model: "gpt-4".to_string(),
        session_id: "s1".to_string(),
        ..Default::default()
    };
    let abort = CancellationToken::new();
    let mut loop_ = AgentLoop::new_with_steering_queue(
        llm,
        primitive,
        event_bus,
        config,
        abort,
        steering_queue,
    );
    let messages = vec![ChatMessage::user("read two files")];
    let result = loop_.run(messages).await.unwrap();
    assert!(result.final_text.contains("steered"));
    assert_eq!(read_count.load(std::sync::atomic::Ordering::SeqCst), 1);
}

/// FollowUp：run 前先 follow_up("next")，同一上下文继续，final_text 含两轮回复。
#[tokio::test]
async fn run_follow_up_continues_in_same_context() {
    let stream_a: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "A".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let stream_b: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "B".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let llm = Arc::new(MockLlmProvider::new(vec![stream_a, stream_b]));
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());
    let config = AgentLoopConfig {
        model: "gpt-4".to_string(),
        session_id: "s1".to_string(),
        ..Default::default()
    };
    let abort = CancellationToken::new();
    let mut loop_ = AgentLoop::new(llm, primitive, event_bus, config, abort);
    loop_.follow_up("next".to_string());
    let messages = vec![ChatMessage::user("first")];
    let result = loop_.run(messages).await.unwrap();
    assert!(result.final_text.contains("B"));
}
