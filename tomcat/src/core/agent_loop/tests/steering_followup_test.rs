//! # Steering / FollowUp 时序测试
//!
//! - **Steering**：第 1 个工具执行后注入 steering，第 2 个工具不应执行；
//!   下一轮 LLM 收到 steering 后返回文本，且实际 read_count == 1。
//!   覆盖 `tool_dispatcher::run_tool_calls` 内 steering 注入抢占点。
//! - **FollowUp**：run 前先 `follow_up("next")`，同一上下文继续 attempt 第二轮，
//!   final_text 含两轮回复中的第二轮内容。覆盖 `run.rs::run` 第一层
//!   Conversation Loop 的 follow_up_queue drain 分支。

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use super::super::steering_injection::{inject_follow_up_messages, inject_steering_messages};
use crate::core::agent_loop::{AgentLoop, AgentLoopConfig};
use crate::core::compaction::preheat::Preheat;
use crate::core::llm::{
    ChatMessage, ChatMessageRole, ChatRequest, ChatResponse, LlmProvider, MessageKind, StreamEvent,
};
use crate::core::session::manager::{estimate_msg_chars, ContextState};
use crate::core::session::transcript::{read_entries_tail, TranscriptEntry};
use crate::infra::error::AppError;
use crate::infra::DefaultEventBus;
use crate::SessionManager;

use super::mocks::{MockLlmProvider, MockPrimitiveExecutor, SteerableMockPrimitive};

struct RecordingMockLlmProvider {
    streams: StdMutex<VecDeque<Vec<Result<StreamEvent, AppError>>>>,
    requests: StdMutex<Vec<ChatRequest>>,
}

impl RecordingMockLlmProvider {
    fn new(streams: Vec<Vec<Result<StreamEvent, AppError>>>) -> Self {
        Self {
            streams: StdMutex::new(streams.into()),
            requests: StdMutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl LlmProvider for RecordingMockLlmProvider {
    fn provider_name(&self) -> &str {
        "recording_mock"
    }

    async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, AppError> {
        Err(AppError::Llm("mock chat not used".to_string()))
    }

    async fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> Result<
        Box<dyn tokio_stream::Stream<Item = Result<StreamEvent, AppError>> + Send + Unpin>,
        AppError,
    > {
        self.requests.lock().unwrap().push(req);
        let events = self.streams.lock().unwrap().pop_front().ok_or_else(|| {
            AppError::Llm("RecordingMockLlmProvider: no more streams".to_string())
        })?;
        Ok(Box::new(tokio_stream::iter(events)))
    }

    fn count_tokens(&self, _messages: &[ChatMessage]) -> Result<u32, AppError> {
        Ok(0)
    }
}

/// Steering：第 1 个工具执行后注入 steering，第 2 个工具不执行，下一轮 LLM 收到 steering 后返回文本。
#[tokio::test]
async fn run_steering_skips_remaining_tools() {
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

#[tokio::test]
async fn run_follow_up_drains_at_tool_batch_boundary_before_next_llm_request() {
    let stream_tools: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ToolCallDelta {
            index: 0,
            id: Some("c1".to_string()),
            name: Some("read".to_string()),
            arguments_delta: Some(r#"{"path":"/tmp/demo"}"#.to_string()),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "tool_calls".to_string(),
        }),
    ];
    let stream_text: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "MIDTURN_OK".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let llm = Arc::new(RecordingMockLlmProvider::new(vec![
        stream_tools,
        stream_text,
    ]));
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());
    let synthetic =
        "<background-task-finished task_id=\"t1\" exit_code=\"0\">done</background-task-finished>";
    let follow_up_queue = Arc::new(parking_lot::Mutex::new(vec![ChatMessage::user(synthetic)]));
    let config = AgentLoopConfig {
        model: "gpt-4".to_string(),
        session_id: "s-followup-midturn".to_string(),
        ..Default::default()
    };
    let abort = CancellationToken::new();
    let mut loop_ = AgentLoop::new(llm.clone(), primitive, event_bus, config, abort)
        .with_shared_follow_up_queue(follow_up_queue.clone());

    let result = loop_.run(vec![ChatMessage::user("start")]).await.unwrap();
    assert!(
        result.final_text.contains("MIDTURN_OK"),
        "第二轮应成功收束，实际 final_text={:?}",
        result.final_text
    );
    assert!(
        follow_up_queue.lock().is_empty(),
        "midturn drain 后 queue 应为空"
    );

    let requests = llm.requests.lock().unwrap();
    assert_eq!(requests.len(), 2, "应正好发起两次 LLM 请求");
    let second_messages = &requests[1].messages;
    let tool_idx = second_messages
        .iter()
        .rposition(|msg| msg.role == ChatMessageRole::Tool)
        .expect("第二次请求前应已有第一轮 tool_result");
    let follow_up_idx = second_messages
        .iter()
        .position(|msg| msg.text_content() == Some(synthetic))
        .expect("第二次请求应已携带 synthetic follow-up");
    assert!(
        follow_up_idx > tool_idx,
        "synthetic follow-up 必须位于 tool_result 之后；tool_idx={tool_idx}, follow_up_idx={follow_up_idx}"
    );
    assert_eq!(
        second_messages.last().and_then(|msg| msg.text_content()),
        Some(synthetic),
        "第二次请求前最后一条消息应是刚注入的 follow-up"
    );
}

#[tokio::test]
async fn run_follow_up_does_not_bypass_max_tool_rounds() {
    let stream_tools: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ToolCallDelta {
            index: 0,
            id: Some("c1".to_string()),
            name: Some("read".to_string()),
            arguments_delta: Some(r#"{"path":"/tmp/demo"}"#.to_string()),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "tool_calls".to_string(),
        }),
    ];
    let llm = Arc::new(RecordingMockLlmProvider::new(vec![stream_tools]));
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());
    let synthetic =
        "<background-task-finished task_id=\"t-budget\" exit_code=\"0\">done</background-task-finished>";
    let follow_up_queue = Arc::new(parking_lot::Mutex::new(vec![ChatMessage::user(synthetic)]));
    let config = AgentLoopConfig {
        model: "gpt-4".to_string(),
        session_id: "s-followup-budget".to_string(),
        max_tool_rounds: 1,
        ..Default::default()
    };
    let abort = CancellationToken::new();
    let mut loop_ = AgentLoop::new(llm.clone(), primitive, event_bus, config, abort)
        .with_shared_follow_up_queue(follow_up_queue.clone());

    let result = loop_.run(vec![ChatMessage::user("start")]).await.unwrap();
    assert!(
        result.final_text.is_empty(),
        "触顶时不应偷偷继续第二轮，实际 final_text={:?}",
        result.final_text
    );
    assert_eq!(
        llm.requests.lock().unwrap().len(),
        1,
        "follow-up 不应绕过 max_tool_rounds 开启额外 LLM 请求"
    );
    assert_eq!(
        follow_up_queue.lock().len(),
        1,
        "预算触顶时 follow-up 应保留给下一次独立 run 处理"
    );
}

#[tokio::test]
async fn inject_steering_messages_records_context_and_persists_msg_id() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = SessionManager::new(dir.path().to_path_buf());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();
    let transcript = mgr.current_transcript_path().unwrap().unwrap();

    let llm = Arc::new(MockLlmProvider::new(vec![]));
    let event_bus = Arc::new(DefaultEventBus::new());
    let steering_queue = Arc::new(parking_lot::Mutex::new(vec![ChatMessage::steering(
        "stop now",
    )]));
    let mut loop_ = AgentLoop::new_with_steering_queue(
        llm,
        Arc::new(MockPrimitiveExecutor),
        event_bus,
        AgentLoopConfig {
            model: "gpt-4".to_string(),
            session_id: "s-steering".to_string(),
            message_append_sink: Some(Arc::new(mgr.clone())),
            ..Default::default()
        },
        CancellationToken::new(),
        steering_queue,
    );
    loop_.set_context_state(Some(ContextState {
        messages: vec![],
        estimate_context_chars: 10,
        context_budget_chars: 1_000,
        context_budget_tokens: 250,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: transcript.clone(),
        latest_plan_event: None,
        preheat: Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    }));

    let mut messages = vec![ChatMessage::user("hello")];
    let appended = inject_steering_messages(&mut loop_, &mut messages).unwrap();
    assert!(appended);
    let steering = messages.last().expect("steering should be appended");
    assert_eq!(steering.kind, MessageKind::Steering);
    assert_eq!(steering.text_content(), Some("stop now"));
    assert!(
        steering.msg_id.is_some(),
        "steering should be persisted and get msg_id"
    );

    let state = loop_.context_state.as_ref().unwrap();
    assert_eq!(
        state.estimate_context_chars,
        10 + estimate_msg_chars(steering)
    );
    assert_eq!(
        state.post_usage_appended_chars,
        estimate_msg_chars(steering)
    );

    let entries = read_entries_tail(&transcript, 10).unwrap();
    assert!(entries.into_iter().any(|entry| match entry {
        TranscriptEntry::Message(me) => {
            me.id == steering.msg_id
                && me.message.get("content").and_then(|v| v.as_str()) == Some("stop now")
        }
        _ => false,
    }));
}

#[tokio::test]
async fn inject_follow_up_messages_returns_false_for_empty_queue() {
    let llm = Arc::new(MockLlmProvider::new(vec![]));
    let event_bus = Arc::new(DefaultEventBus::new());
    let follow_up_queue = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let mut loop_ = AgentLoop::new(
        llm,
        Arc::new(MockPrimitiveExecutor),
        event_bus,
        AgentLoopConfig {
            model: "gpt-4".to_string(),
            session_id: "s-followup-empty".to_string(),
            ..Default::default()
        },
        CancellationToken::new(),
    )
    .with_shared_follow_up_queue(follow_up_queue);

    let mut messages = vec![ChatMessage::user("hello")];
    let appended = inject_follow_up_messages(&mut loop_, &mut messages).unwrap();
    assert!(!appended, "空 queue 不应追加消息");
    assert_eq!(messages.len(), 1, "空 queue 不应改写 messages");
}

#[tokio::test]
async fn inject_follow_up_messages_records_context_and_persists_msg_id() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = SessionManager::new(dir.path().to_path_buf());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();
    let transcript = mgr.current_transcript_path().unwrap().unwrap();

    let llm = Arc::new(MockLlmProvider::new(vec![]));
    let event_bus = Arc::new(DefaultEventBus::new());
    let follow_up_text = "<background-task-finished task_id=\"t-followup\" exit_code=\"0\">done</background-task-finished>";
    let follow_up_queue = Arc::new(parking_lot::Mutex::new(vec![ChatMessage::user(
        follow_up_text,
    )]));
    let mut loop_ = AgentLoop::new(
        llm,
        Arc::new(MockPrimitiveExecutor),
        event_bus,
        AgentLoopConfig {
            model: "gpt-4".to_string(),
            session_id: "s-followup-persist".to_string(),
            message_append_sink: Some(Arc::new(mgr.clone())),
            ..Default::default()
        },
        CancellationToken::new(),
    )
    .with_shared_follow_up_queue(follow_up_queue);
    loop_.set_context_state(Some(ContextState {
        messages: vec![],
        estimate_context_chars: 10,
        context_budget_chars: 1_000,
        context_budget_tokens: 250,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: transcript.clone(),
        latest_plan_event: None,
        preheat: Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    }));

    let mut messages = vec![ChatMessage::user("hello")];
    let appended = inject_follow_up_messages(&mut loop_, &mut messages).unwrap();
    assert!(appended);
    let follow_up = messages.last().expect("follow-up should be appended");
    assert_eq!(follow_up.role, ChatMessageRole::User);
    assert_eq!(follow_up.text_content(), Some(follow_up_text));
    assert!(
        follow_up.msg_id.is_some(),
        "follow-up 应即时落盘并拿到 msg_id"
    );

    let state = loop_.context_state.as_ref().unwrap();
    assert_eq!(
        state.estimate_context_chars,
        10 + estimate_msg_chars(follow_up)
    );
    assert_eq!(
        state.post_usage_appended_chars,
        estimate_msg_chars(follow_up)
    );

    let entries = read_entries_tail(&transcript, 10).unwrap();
    assert!(entries.into_iter().any(|entry| match entry {
        TranscriptEntry::Message(me) => {
            me.id == follow_up.msg_id
                && me.message.get("content").and_then(|v| v.as_str()) == Some(follow_up_text)
        }
        _ => false,
    }));
}
