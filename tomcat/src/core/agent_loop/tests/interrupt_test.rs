//! # Abort / Interrupt 时序硬验收（T-003 / T-004 / T-017）
//!
//! 中断路径的"持久化契约"测试，分四个用例：
//!
//! - `run_aborts_returns_interrupted`：工具执行中 cancel，run 返回 Interrupted +
//!   agent_end(error=interrupted)。
//! - `run_interrupt_between_tools_retains_completed_tool_result`：tool 轮之间
//!   cancel，partial_messages 必须**包含**已完成的 tool_result（T-017 核心）。
//! - `run_interrupt_during_stream_preserves_partial_text`：LLM 流式 delta 期间
//!   cancel，partial_text 非空、assistant partial 入 messages（T-004 核心）。
//! - `token_rebuild_per_turn_allows_next_run`：预 cancel 的 token 在 run() 入口
//!   立即返回 Interrupted；新 token 的 AgentLoop 应能正常收束（架构 §6.2）。

use std::sync::Arc;
use std::sync::Mutex;

use tokio_util::sync::CancellationToken;

use crate::core::agent_loop::{AgentLoop, AgentLoopConfig, AgentRunOutcome};
use crate::core::llm::{ChatMessage, ChatRequest, ChatResponse, LlmProvider, StreamEvent};
use crate::infra::error::AppError;
use crate::infra::event_bus::EventBus;
use crate::infra::wire;
use crate::infra::{DefaultEventBus, EventContext};

use super::mocks::{MockLlmProvider, MockPrimitiveExecutor, SleepyMockPrimitive};

/// Abort：工具执行前/中设置 abort_signal，run 返回 Err，agent_end 含 interrupted。
#[tokio::test]
async fn run_aborts_returns_interrupted() {
    let stream_tools: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ToolCallDelta {
            index: 0,
            id: Some("c1".to_string()),
            name: Some("read".to_string()),
            arguments_delta: Some(r#"{"path":"/a"}"#.to_string()),
        }),
        Ok(StreamEvent::ToolCallDelta {
            index: 1,
            id: Some("c2".to_string()),
            name: Some("read".to_string()),
            arguments_delta: Some(r#"{"path":"/b"}"#.to_string()),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "tool_calls".to_string(),
        }),
    ];
    let llm = Arc::new(MockLlmProvider::new(vec![stream_tools]));
    let primitive = Arc::new(SleepyMockPrimitive);
    let event_bus = Arc::new(DefaultEventBus::new());
    let agent_end_error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let err_clone = Arc::clone(&agent_end_error);
    event_bus.on(
        wire::WIRE_AGENT_END,
        Box::new(move |ctx: EventContext| {
            let err = ctx
                .payload
                .get("error")
                .and_then(|v| v.as_str())
                .map(String::from);
            *err_clone.lock().unwrap() = err;
            Ok(())
        }),
    );
    let config = AgentLoopConfig {
        model: "gpt-4".to_string(),
        session_id: "s1".to_string(),
        ..Default::default()
    };
    let abort_signal = CancellationToken::new();
    let mut loop_ = AgentLoop::new(llm, primitive, event_bus, config, abort_signal.clone());
    let messages = vec![ChatMessage::user("read files")];
    let abort_for_thread = abort_signal.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(20));
        abort_for_thread.cancel();
    });
    let result = loop_.run(messages).await;
    assert!(
        result.is_interrupted(),
        "expected Interrupted outcome, got {:?}",
        result
    );
    let captured = agent_end_error.lock().unwrap().take();
    assert_eq!(captured.as_deref(), Some("interrupted"));
}

/// 在 tool 轮之间取消：partial_messages 必须**包含**已完成的 tool_result，
/// 使外层 chat_loop 对中断路径做与正常收束一致的落盘（T-017 的核心主张）。
#[tokio::test]
async fn run_interrupt_between_tools_retains_completed_tool_result() {
    let stream_tools: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ToolCallDelta {
            index: 0,
            id: Some("c1".to_string()),
            name: Some("read".to_string()),
            arguments_delta: Some(r#"{"path":"/a"}"#.to_string()),
        }),
        Ok(StreamEvent::ToolCallDelta {
            index: 1,
            id: Some("c2".to_string()),
            name: Some("read".to_string()),
            arguments_delta: Some(r#"{"path":"/b"}"#.to_string()),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "tool_calls".to_string(),
        }),
    ];
    let llm = Arc::new(MockLlmProvider::new(vec![stream_tools]));
    let primitive = Arc::new(SleepyMockPrimitive);
    let event_bus = Arc::new(DefaultEventBus::new());

    let interrupted_payloads: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
    let ip_clone = Arc::clone(&interrupted_payloads);
    event_bus.on(
        wire::WIRE_AGENT_INTERRUPTED,
        Box::new(move |ctx: EventContext| {
            ip_clone.lock().unwrap().push(ctx.payload.clone());
            Ok(())
        }),
    );

    let config = AgentLoopConfig {
        model: "gpt-4".to_string(),
        session_id: "s-int-tools".to_string(),
        ..Default::default()
    };
    let cancel = CancellationToken::new();
    let mut agent = AgentLoop::new(llm, primitive, event_bus, config, cancel.clone());

    let cancel_bg = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_millis(130)).await;
        cancel_bg.cancel();
    });

    let outcome = agent.run(vec![ChatMessage::user("read two files")]).await;
    assert!(
        outcome.is_interrupted(),
        "期望 Interrupted outcome，实际 {:?}",
        outcome
    );
    let result = match outcome {
        AgentRunOutcome::Interrupted(r) => r,
        other => panic!("unexpected: {:?}", other),
    };

    let roles: Vec<String> = result
        .new_messages
        .iter()
        .map(|m| format!("{:?}", m.role))
        .collect();
    assert!(
        roles.iter().any(|r| r.contains("Assistant")),
        "partial_messages 应含 assistant（发起工具调用的一条），实际 roles={:?}",
        roles
    );
    let tool_msgs: Vec<&ChatMessage> = result
        .new_messages
        .iter()
        .filter(|m| format!("{:?}", m.role).contains("Tool"))
        .collect();
    assert_eq!(
        tool_msgs.len(),
        1,
        "应恰好有 1 个已完成的 tool_result（c1），实际 {} 个：{:?}",
        tool_msgs.len(),
        roles
    );

    let emitted = interrupted_payloads.lock().unwrap();
    assert_eq!(emitted.len(), 1, "应发布 1 次 agent_interrupted");
    let p = &emitted[0];
    assert_eq!(
        p.get("sessionId").and_then(|v| v.as_str()),
        Some("s-int-tools")
    );
    assert_eq!(p.get("toolResultsCount").and_then(|v| v.as_u64()), Some(1));
}

/// 在 LLM 流式输出 delta 期间取消：partial_text 非空、assistant partial 入 messages、
/// final_text 与 partial_text 一致。覆盖 T-004（不丢 LLM 回复）。
#[tokio::test]
async fn run_interrupt_during_stream_preserves_partial_text() {
    use tokio_stream::wrappers::ReceiverStream;

    struct StreamingLlm {
        rx: Mutex<Option<tokio::sync::mpsc::Receiver<Result<StreamEvent, AppError>>>>,
    }

    #[async_trait::async_trait]
    impl LlmProvider for StreamingLlm {
        fn provider_name(&self) -> &str {
            "streaming_mock"
        }
        async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, AppError> {
            Err(AppError::Llm("unused".into()))
        }
        async fn chat_stream(
            &self,
            _req: ChatRequest,
        ) -> Result<
            Box<dyn tokio_stream::Stream<Item = Result<StreamEvent, AppError>> + Send + Unpin>,
            AppError,
        > {
            let rx = self
                .rx
                .lock()
                .unwrap()
                .take()
                .expect("chat_stream called twice");
            Ok(Box::new(ReceiverStream::new(rx)))
        }
        fn count_tokens(&self, _messages: &[ChatMessage]) -> Result<u32, AppError> {
            Ok(0)
        }
    }

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<StreamEvent, AppError>>(16);
    let llm = Arc::new(StreamingLlm {
        rx: Mutex::new(Some(rx)),
    });
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());

    tokio::spawn(async move {
        for i in 0..200 {
            if tx
                .send(Ok(StreamEvent::ContentDelta {
                    delta: format!("chunk-{i} "),
                }))
                .await
                .is_err()
            {
                break;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }
    });

    let config = AgentLoopConfig {
        model: "gpt-4".to_string(),
        session_id: "s-int-stream".to_string(),
        ..Default::default()
    };
    let cancel = CancellationToken::new();
    let mut agent = AgentLoop::new(llm, primitive, event_bus, config, cancel.clone());

    let cancel_bg = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_millis(80)).await;
        cancel_bg.cancel();
    });

    let outcome = agent.run(vec![ChatMessage::user("stream")]).await;
    let result = match outcome {
        AgentRunOutcome::Interrupted(r) => r,
        other => panic!("期望 Interrupted，实际 {:?}", other),
    };

    assert!(
        !result.final_text.is_empty(),
        "partial_text 不应为空（stream 期间 delta 已累积）"
    );
    assert!(
        result.final_text.contains("chunk-"),
        "partial_text 应含 delta 片段，实际: {:?}",
        result.final_text
    );
    assert!(
        result
            .new_messages
            .iter()
            .any(|m| format!("{:?}", m.role).contains("Assistant")),
        "partial_messages 应含 assistant 消息（承载 partial_text）"
    );
}

/// Token 每回合重建：预取消的 token 应在 run() 入口立即返回 Interrupted；
/// 新 token 的 AgentLoop 应能正常收束。验证架构文档 §6.2 的契约——
/// CancellationToken 一旦 cancel 不可逆，必须每回合重建。
#[tokio::test]
async fn token_rebuild_per_turn_allows_next_run() {
    let stream2: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "second-ok".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let llm = Arc::new(MockLlmProvider::new(vec![stream2]));
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());
    let config_a = AgentLoopConfig {
        model: "gpt-4".to_string(),
        session_id: "s-rebuild".to_string(),
        ..Default::default()
    };

    let token_a = CancellationToken::new();
    token_a.cancel();
    let mut loop_a = AgentLoop::new(
        llm.clone(),
        primitive.clone(),
        event_bus.clone(),
        config_a,
        token_a.clone(),
    );
    let out_a = loop_a.run(vec![ChatMessage::user("first")]).await;
    assert!(
        out_a.is_interrupted(),
        "已 cancel 的 token 应在 run() 入口立即返回 Interrupted"
    );

    let config_b = AgentLoopConfig {
        model: "gpt-4".to_string(),
        session_id: "s-rebuild".to_string(),
        ..Default::default()
    };
    let token_b = CancellationToken::new();
    assert!(
        !token_b.is_cancelled(),
        "新 token 必须未被 cancel（否则证明 token 被跨回合复用）"
    );
    let mut loop_b = AgentLoop::new(llm, primitive, event_bus, config_b, token_b);
    let out_b = loop_b.run(vec![ChatMessage::user("second")]).await;
    assert!(out_b.is_ok(), "新回合应正常 Completed");
    let r = out_b.unwrap();
    assert_eq!(r.final_text, "second-ok");
}
