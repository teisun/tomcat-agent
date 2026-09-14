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

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::core::agent_loop::{AgentLoop, AgentLoopConfig, AgentRunOutcome};
use crate::core::compaction::preheat::Preheat;
use crate::core::llm::retry_delay::sleep_provider_retry_delay;
use crate::core::llm::{ChatMessage, ChatRequest, ChatResponse, LlmProvider, StreamEvent};
use crate::core::session::find_dangling_tail_tool_call_ids;
use crate::core::session::manager::ContextState;
use crate::infra::error::AppError;
use crate::infra::event_bus::EventBus;
use crate::infra::wire;
use crate::infra::{DefaultEventBus, EventContext};

use super::mocks::{test_binding, MockLlmProvider, MockPrimitiveExecutor, SleepyMockPrimitive};

fn dangling_tool_call_ids(messages: &[ChatMessage]) -> Option<Vec<String>> {
    let recent = messages
        .iter()
        .map(serde_json::to_value)
        .collect::<Result<Vec<_>, _>>()
        .expect("chat messages should serialize");
    find_dangling_tail_tool_call_ids(&recent)
}

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
        session_id: "s1".to_string(),
        ..Default::default()
    };
    let abort_signal = CancellationToken::new();
    let mut loop_ = AgentLoop::new(
        test_binding(llm, "gpt-4"),
        primitive,
        event_bus,
        config,
        abort_signal.clone(),
    );
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
        session_id: "s-int-tools".to_string(),
        ..Default::default()
    };
    let cancel = CancellationToken::new();
    let mut agent = AgentLoop::new(
        test_binding(llm, "gpt-4"),
        primitive,
        event_bus,
        config,
        cancel.clone(),
    );
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
        2,
        "应保留已完成结果并为缺失尾巴补 `[interrupted]`，实际 {} 个：{:?}",
        tool_msgs.len(),
        roles
    );
    assert_eq!(tool_msgs[0].tool_call_id.as_deref(), Some("c1"));
    assert_eq!(tool_msgs[0].text_content(), Some("content:/a"));
    assert_eq!(tool_msgs[1].tool_call_id.as_deref(), Some("c2"));
    assert_eq!(tool_msgs[1].text_content(), Some("[interrupted]"));
    assert_eq!(
        dangling_tool_call_ids(&result.new_messages),
        None,
        "Interrupted 收尾后尾部 tool round 应已闭合"
    );

    let emitted = interrupted_payloads.lock().unwrap();
    assert_eq!(emitted.len(), 1, "应发布 1 次 agent_interrupted");
    let p = &emitted[0];
    assert_eq!(
        p.get("sessionId").and_then(|v| v.as_str()),
        Some("s-int-tools")
    );
    assert_eq!(p.get("toolResultsCount").and_then(|v| v.as_u64()), Some(2));
}

#[tokio::test]
async fn run_interrupt_during_active_tool_appends_synthetic_tool_result() {
    let stream_tools: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ToolCallDelta {
            index: 0,
            id: Some("c1".to_string()),
            name: Some("read".to_string()),
            arguments_delta: Some(r#"{"path":"/a"}"#.to_string()),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "tool_calls".to_string(),
        }),
    ];
    let llm = Arc::new(MockLlmProvider::new(vec![stream_tools]));
    let primitive = Arc::new(SleepyMockPrimitive);
    let event_bus = Arc::new(DefaultEventBus::new());
    let config = AgentLoopConfig {
        session_id: "s-int-active-tool".to_string(),
        ..Default::default()
    };
    let cancel = CancellationToken::new();
    let mut agent = AgentLoop::new(
        test_binding(llm, "gpt-4"),
        primitive,
        event_bus,
        config,
        cancel.clone(),
    );

    let cancel_bg = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
        cancel_bg.cancel();
    });

    let outcome = agent.run(vec![ChatMessage::user("read one file")]).await;
    let result = match outcome {
        AgentRunOutcome::Interrupted(r) => r,
        other => panic!("期望 Interrupted，实际 {:?}", other),
    };

    let tool_msgs: Vec<&ChatMessage> = result
        .new_messages
        .iter()
        .filter(|m| format!("{:?}", m.role).contains("Tool"))
        .collect();
    assert_eq!(
        tool_msgs.len(),
        1,
        "中断中的单工具调用应补一条 `[interrupted]`"
    );
    assert_eq!(tool_msgs[0].tool_call_id.as_deref(), Some("c1"));
    assert_eq!(tool_msgs[0].text_content(), Some("[interrupted]"));
    assert_eq!(
        dangling_tool_call_ids(&result.new_messages),
        None,
        "单工具调用中断后尾部应已闭合"
    );
}

#[tokio::test]
async fn run_interrupt_during_parallel_tools_heals_all_remaining() {
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
        Ok(StreamEvent::ToolCallDelta {
            index: 2,
            id: Some("c3".to_string()),
            name: Some("read".to_string()),
            arguments_delta: Some(r#"{"path":"/c"}"#.to_string()),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "tool_calls".to_string(),
        }),
    ];
    let llm = Arc::new(MockLlmProvider::new(vec![stream_tools]));
    let primitive = Arc::new(SleepyMockPrimitive);
    let event_bus = Arc::new(DefaultEventBus::new());
    let config = AgentLoopConfig {
        session_id: "s-int-three-tools".to_string(),
        ..Default::default()
    };
    let cancel = CancellationToken::new();
    let mut agent = AgentLoop::new(
        test_binding(llm, "gpt-4"),
        primitive,
        event_bus,
        config,
        cancel.clone(),
    );

    let cancel_bg = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_millis(130)).await;
        cancel_bg.cancel();
    });

    let outcome = agent.run(vec![ChatMessage::user("read three files")]).await;
    let result = match outcome {
        AgentRunOutcome::Interrupted(r) => r,
        other => panic!("期望 Interrupted，实际 {:?}", other),
    };

    let tool_msgs: Vec<&ChatMessage> = result
        .new_messages
        .iter()
        .filter(|m| format!("{:?}", m.role).contains("Tool"))
        .collect();
    assert_eq!(tool_msgs.len(), 3, "三工具调用中断后应保留 c1 并补齐 c2/c3");
    assert_eq!(tool_msgs[0].tool_call_id.as_deref(), Some("c1"));
    assert_eq!(tool_msgs[0].text_content(), Some("content:/a"));
    assert_eq!(tool_msgs[1].tool_call_id.as_deref(), Some("c2"));
    assert_eq!(tool_msgs[1].text_content(), Some("[interrupted]"));
    assert_eq!(tool_msgs[2].tool_call_id.as_deref(), Some("c3"));
    assert_eq!(tool_msgs[2].text_content(), Some("[interrupted]"));
    assert_eq!(
        dangling_tool_call_ids(&result.new_messages),
        None,
        "多工具调用中断后尾部应已闭合，不能漏掉剩余 tool_call"
    );
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
        session_id: "s-int-stream".to_string(),
        ..Default::default()
    };
    let cancel = CancellationToken::new();
    let mut agent = AgentLoop::new(
        test_binding(llm, "gpt-4"),
        primitive,
        event_bus,
        config,
        cancel.clone(),
    );
    let mut state = ContextState {
        messages: vec![],
        estimate_context_chars: 4_000,
        context_budget_chars: 100_000,
        context_budget_tokens: 25_000,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        latest_plan_event: None,
        resume_control: Default::default(),
        preheat: Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    };
    // Simulate a stale prior response usage: this stream is cancelled before
    // it emits Usage, so the partial assistant must enter the post-usage delta.
    state.update_api_usage(1_000, 0);
    agent.set_context_state(Some(state));

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
    let state = agent.context_state.as_ref().expect("context state");
    assert_eq!(
        state.post_usage_appended_chars,
        result.final_text.len(),
        "an aborted stream without Usage must retain its partial assistant in the post-usage delta"
    );
    assert_eq!(
        state.estimated_token_count(),
        1_000 + result.final_text.len() / 4,
        "the next timing-2 check must include the persisted partial response"
    );
}

#[tokio::test]
async fn run_interrupt_during_blocking_collapse_returns_promptly() {
    struct BlockingCompactionLlm {
        entered: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    }

    #[async_trait::async_trait]
    impl LlmProvider for BlockingCompactionLlm {
        fn provider_name(&self) -> &str {
            "blocking-compaction"
        }

        async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, AppError> {
            if let Some(entered) = self.entered.lock().unwrap().take() {
                let _ = entered.send(());
            }
            std::future::pending::<Result<ChatResponse, AppError>>().await
        }

        async fn chat_stream(
            &self,
            _req: ChatRequest,
        ) -> Result<
            Box<dyn tokio_stream::Stream<Item = Result<StreamEvent, AppError>> + Send + Unpin>,
            AppError,
        > {
            Err(AppError::Llm(
                "main stream must not start during collapse".to_string(),
            ))
        }

        fn count_tokens(&self, _messages: &[ChatMessage]) -> Result<u32, AppError> {
            Ok(0)
        }
    }

    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let compaction_provider: Arc<dyn LlmProvider> = Arc::new(BlockingCompactionLlm {
        entered: Mutex::new(Some(entered_tx)),
    });
    let main_provider = Arc::new(MockLlmProvider::new(vec![]));
    let cancel = CancellationToken::new();
    let mut agent = AgentLoop::new(
        test_binding(main_provider, "gpt-4"),
        Arc::new(MockPrimitiveExecutor),
        Arc::new(DefaultEventBus::new()),
        AgentLoopConfig {
            session_id: "s-int-blocking-collapse".to_string(),
            context_config: crate::infra::config::ContextConfig {
                compaction_model: "compaction".to_string(),
                ..Default::default()
            },
            compaction_provider: Some(compaction_provider),
            ..Default::default()
        },
        cancel.clone(),
    );

    let mut user = ChatMessage::user("u".repeat(4_000));
    user.msg_id = Some("u1".to_string());
    let mut assistant = ChatMessage::assistant("a".repeat(4_000));
    assistant.msg_id = Some("a1".to_string());
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

    let cancel_after_collapse_started = cancel.clone();
    tokio::spawn(async move {
        entered_rx
            .await
            .expect("collapse provider should receive exactly one summary request");
        cancel_after_collapse_started.cancel();
    });

    let outcome = tokio::time::timeout(Duration::from_secs(1), agent.run(vec![user, assistant]))
        .await
        .expect("outer cancel boundary must not wait for a non-cancellable collapse");
    assert!(
        outcome.is_interrupted(),
        "blocking collapse should finish as Interrupted, got {outcome:?}"
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
        session_id: "s-rebuild".to_string(),
        ..Default::default()
    };

    let token_a = CancellationToken::new();
    token_a.cancel();
    let mut loop_a = AgentLoop::new(
        test_binding(llm.clone(), "gpt-4"),
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
        session_id: "s-rebuild".to_string(),
        ..Default::default()
    };
    let token_b = CancellationToken::new();
    assert!(
        !token_b.is_cancelled(),
        "新 token 必须未被 cancel（否则证明 token 被跨回合复用）"
    );
    let mut loop_b = AgentLoop::new(
        test_binding(llm, "gpt-4"),
        primitive,
        event_bus,
        config_b,
        token_b,
    );
    let out_b = loop_b.run(vec![ChatMessage::user("second")]).await;
    assert!(out_b.is_ok(), "新回合应正常 Completed");
    let r = out_b.unwrap();
    assert_eq!(r.final_text, "second-ok");
}

#[tokio::test(start_paused = true)]
async fn run_interrupt_during_provider_retry_backoff_returns_interrupted() {
    struct BackoffProvider;

    #[async_trait::async_trait]
    impl LlmProvider for BackoffProvider {
        fn provider_name(&self) -> &str {
            "backoff-provider"
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
            sleep_provider_retry_delay(Duration::from_secs(30)).await?;
            Err(AppError::Llm(
                "should have been cancelled during retry backoff".into(),
            ))
        }

        fn count_tokens(&self, _messages: &[ChatMessage]) -> Result<u32, AppError> {
            Ok(0)
        }
    }

    let llm: Arc<dyn LlmProvider> = Arc::new(BackoffProvider);
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());
    let cancel = CancellationToken::new();
    let config = AgentLoopConfig {
        session_id: "provider-backoff-cancel".to_string(),
        ..Default::default()
    };
    let mut loop_ = AgentLoop::new(
        test_binding(llm, "gpt-4"),
        primitive,
        event_bus,
        config,
        cancel.clone(),
    );

    let cancel_bg = cancel.clone();
    tokio::spawn(async move {
        tokio::task::yield_now().await;
        cancel_bg.cancel();
    });

    let outcome = loop_
        .run(vec![ChatMessage::user("interrupt provider retry backoff")])
        .await;
    assert!(
        outcome.is_interrupted(),
        "expected Interrupted outcome, got {:?}",
        outcome
    );
}
