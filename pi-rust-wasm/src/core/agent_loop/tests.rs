use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::sync::Mutex;

use crate::core::llm::{ChatRequest, ChatResponse, LlmProvider, StreamEvent};
use crate::core::primitives::PrimitiveExecutor;
use crate::infra::error::AppError;
use crate::infra::event_bus::EventBus;
use crate::infra::wire;
use crate::infra::{DefaultEventBus, EventContext};

use super::convert::classify_error;
use super::*;

struct MockLlmProvider {
    /// 每次 chat_stream 调用取出一组事件（或错误）返回。
    streams: Mutex<Vec<Vec<Result<StreamEvent, AppError>>>>,
}

impl MockLlmProvider {
    fn new(streams: Vec<Vec<Result<StreamEvent, AppError>>>) -> Self {
        Self {
            streams: Mutex::new(streams),
        }
    }
}

#[async_trait::async_trait]
impl LlmProvider for MockLlmProvider {
    fn provider_name(&self) -> &str {
        "mock"
    }
    async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, AppError> {
        Err(AppError::Llm("mock chat not used".to_string()))
    }
    async fn chat_stream(
        &self,
        _req: ChatRequest,
    ) -> Result<
        Box<dyn tokio_stream::Stream<Item = Result<StreamEvent, AppError>> + Send + Unpin>,
        AppError,
    > {
        let mut guard = self.streams.lock().unwrap();
        let events = guard.remove(0);
        drop(guard);
        let stream = tokio_stream::iter(events);
        Ok(Box::new(stream))
    }
    fn count_tokens(&self, _messages: &[crate::core::llm::ChatMessage]) -> Result<u32, AppError> {
        Ok(0)
    }
}

/// Mock LLM 的 chat_stream 直接返回 Err（用于 Fatal 401 等测试）。
struct MockLlmProviderFatal {
    err: String,
}

#[async_trait::async_trait]
impl LlmProvider for MockLlmProviderFatal {
    fn provider_name(&self) -> &str {
        "mock_fatal"
    }
    async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, AppError> {
        Err(AppError::Llm("mock chat not used".to_string()))
    }
    async fn chat_stream(
        &self,
        _req: ChatRequest,
    ) -> Result<
        Box<dyn tokio_stream::Stream<Item = Result<StreamEvent, AppError>> + Send + Unpin>,
        AppError,
    > {
        Err(AppError::Llm(self.err.clone()))
    }
    fn count_tokens(&self, _messages: &[crate::core::llm::ChatMessage]) -> Result<u32, AppError> {
        Ok(0)
    }
}

struct MockPrimitiveExecutor;

/// 工具执行时 sleep，便于在测试中在另一任务里设置 abort。
struct SleepyMockPrimitive;

#[async_trait::async_trait]
impl PrimitiveExecutor for SleepyMockPrimitive {
    async fn read_file(&self, path: &str, _plugin_id: &str) -> Result<String, AppError> {
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        Ok(format!("content:{}", path))
    }
    async fn list_dir(
        &self,
        _path: &str,
        _plugin_id: &str,
    ) -> Result<Vec<crate::core::primitives::DirEntry>, AppError> {
        Ok(vec![])
    }
    async fn write_file(
        &self,
        path: &str,
        content: &str,
        overwrite: bool,
        _plugin_id: &str,
    ) -> Result<crate::core::primitives::WriteFileResult, AppError> {
        Ok(crate::core::primitives::WriteFileResult {
            path: path.to_string(),
            written: overwrite || content.is_empty(),
        })
    }
    async fn edit_file(
        &self,
        path: &str,
        _edits: Vec<crate::core::primitives::EditOperation>,
        _plugin_id: &str,
    ) -> Result<crate::core::primitives::EditFileResult, AppError> {
        Ok(crate::core::primitives::EditFileResult {
            path: path.to_string(),
            applied: true,
        })
    }
    async fn execute_bash(
        &self,
        command: &str,
        _cwd: Option<&str>,
        _plugin_id: &str,
        _argv: Option<&[String]>,
    ) -> Result<crate::core::primitives::BashResult, AppError> {
        Ok(crate::core::primitives::BashResult {
            stdout: format!("out:{}", command),
            stderr: String::new(),
            exit_code: 0,
        })
    }
    async fn require_user_confirmation(
        &self,
        _operation: crate::core::primitives::PrimitiveOperation,
        _preview: &str,
        _plugin_id: &str,
    ) -> Result<bool, AppError> {
        Ok(true)
    }
}

/// 第一次 read_file 时向 steering_queue 推入 Steering，用于测试"跳过剩余工具"。
struct SteerableMockPrimitive {
    steering_queue: Arc<parking_lot::Mutex<Vec<AgentMessage>>>,
    read_count: Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait::async_trait]
impl PrimitiveExecutor for SteerableMockPrimitive {
    async fn read_file(&self, path: &str, _plugin_id: &str) -> Result<String, AppError> {
        let n = self
            .read_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if n == 0 {
            self.steering_queue.lock().push(AgentMessage::Steering {
                text: "stop after first tool".to_string(),
                timestamp: 0,
            });
        }
        Ok(format!("content:{}", path))
    }
    async fn list_dir(
        &self,
        _path: &str,
        _plugin_id: &str,
    ) -> Result<Vec<crate::core::primitives::DirEntry>, AppError> {
        Ok(vec![])
    }
    async fn write_file(
        &self,
        path: &str,
        content: &str,
        overwrite: bool,
        _plugin_id: &str,
    ) -> Result<crate::core::primitives::WriteFileResult, AppError> {
        Ok(crate::core::primitives::WriteFileResult {
            path: path.to_string(),
            written: overwrite || content.is_empty(),
        })
    }
    async fn edit_file(
        &self,
        path: &str,
        _edits: Vec<crate::core::primitives::EditOperation>,
        _plugin_id: &str,
    ) -> Result<crate::core::primitives::EditFileResult, AppError> {
        Ok(crate::core::primitives::EditFileResult {
            path: path.to_string(),
            applied: true,
        })
    }
    async fn execute_bash(
        &self,
        command: &str,
        _cwd: Option<&str>,
        _plugin_id: &str,
        _argv: Option<&[String]>,
    ) -> Result<crate::core::primitives::BashResult, AppError> {
        Ok(crate::core::primitives::BashResult {
            stdout: format!("out:{}", command),
            stderr: String::new(),
            exit_code: 0,
        })
    }
    async fn require_user_confirmation(
        &self,
        _operation: crate::core::primitives::PrimitiveOperation,
        _preview: &str,
        _plugin_id: &str,
    ) -> Result<bool, AppError> {
        Ok(true)
    }
}

#[async_trait::async_trait]
impl PrimitiveExecutor for MockPrimitiveExecutor {
    async fn read_file(&self, path: &str, _plugin_id: &str) -> Result<String, AppError> {
        Ok(format!("content:{}", path))
    }
    async fn list_dir(
        &self,
        _path: &str,
        _plugin_id: &str,
    ) -> Result<Vec<crate::core::primitives::DirEntry>, AppError> {
        Ok(vec![])
    }
    async fn write_file(
        &self,
        path: &str,
        content: &str,
        overwrite: bool,
        _plugin_id: &str,
    ) -> Result<crate::core::primitives::WriteFileResult, AppError> {
        Ok(crate::core::primitives::WriteFileResult {
            path: path.to_string(),
            written: overwrite || content.is_empty(),
        })
    }
    async fn edit_file(
        &self,
        path: &str,
        _edits: Vec<crate::core::primitives::EditOperation>,
        _plugin_id: &str,
    ) -> Result<crate::core::primitives::EditFileResult, AppError> {
        Ok(crate::core::primitives::EditFileResult {
            path: path.to_string(),
            applied: true,
        })
    }
    async fn execute_bash(
        &self,
        command: &str,
        _cwd: Option<&str>,
        _plugin_id: &str,
        _argv: Option<&[String]>,
    ) -> Result<crate::core::primitives::BashResult, AppError> {
        Ok(crate::core::primitives::BashResult {
            stdout: format!("out:{}", command),
            stderr: String::new(),
            exit_code: 0,
        })
    }
    async fn require_user_confirmation(
        &self,
        _operation: crate::core::primitives::PrimitiveOperation,
        _preview: &str,
        _plugin_id: &str,
    ) -> Result<bool, AppError> {
        Ok(true)
    }
}

#[tokio::test]
async fn run_returns_text_when_llm_returns_text_only() {
    let stream1: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "Hello".to_string(),
        }),
        Ok(StreamEvent::ContentDelta {
            delta: " world".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let llm = Arc::new(MockLlmProvider::new(vec![stream1]));
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());
    let config = AgentLoopConfig {
        model: "gpt-4".to_string(),
        session_id: "s1".to_string(),
        ..Default::default()
    };
    let abort = Arc::new(AtomicBool::new(false));
    let mut loop_ = AgentLoop::new(llm, primitive, event_bus, config, abort);
    let messages = vec![AgentMessage::User {
        text: "hi".to_string(),
    }];
    let result = loop_.run(messages).await.unwrap();
    assert_eq!(result.final_text, "Hello world");
}

#[tokio::test]
async fn convert_to_llm_format_steering_as_user() {
    let messages = vec![AgentMessage::Steering {
        text: "stop".to_string(),
        timestamp: 0,
    }];
    let out = convert_to_llm_format(&messages);
    assert_eq!(out.len(), 1);
    assert!(matches!(
        out[0].role,
        crate::core::llm::ChatMessageRole::User
    ));
    assert_eq!(out[0].text_content(), Some("stop"));
}

#[tokio::test]
async fn convert_to_llm_format_all_variants() {
    let messages = vec![
        AgentMessage::User {
            text: "u".to_string(),
        },
        AgentMessage::System {
            text: "s".to_string(),
        },
        AgentMessage::Assistant {
            text: "a".to_string(),
            tool_calls: vec![],
        },
        AgentMessage::ToolResult {
            tool_call_id: "id1".to_string(),
            content: "c".to_string(),
            is_error: false,
        },
    ];
    let out = convert_to_llm_format(&messages);
    assert_eq!(out.len(), 4);
    assert_eq!(out[0].text_content(), Some("u"));
    assert_eq!(out[1].text_content(), Some("s"));
    assert_eq!(out[2].text_content(), Some("a"));
    assert_eq!(out[3].text_content(), Some("c"));
}

#[test]
#[allow(deprecated)]
fn compact_messages_keeps_system_and_recent() {
    let mut messages: Vec<AgentMessage> = (0..25)
        .map(|i| AgentMessage::User {
            text: format!("msg{}", i),
        })
        .collect();
    messages.insert(
        0,
        AgentMessage::System {
            text: "sys".to_string(),
        },
    );
    compact_messages(&mut messages, 5);
    assert!(messages.len() <= 6 + 5);
    let first = match &messages[0] {
        AgentMessage::System { text } => text.as_str(),
        _ => "",
    };
    assert_eq!(first, "sys");
}

#[test]
fn classify_error_retryable_429() {
    let e = AppError::Llm("API 错误 429: rate limit".to_string());
    let r = classify_error(&e);
    assert!(matches!(r, LoopError::Retryable(_)));
}

#[test]
fn classify_error_fatal_401() {
    let e = AppError::Llm("API 错误 401: unauthorized".to_string());
    let r = classify_error(&e);
    assert!(matches!(r, LoopError::Fatal(_)));
}

/// 重试：Mock LLM 先返回 429 再返回成功 -> 自动重试后得到文本。
#[tokio::test]
async fn run_retries_on_429_then_succeeds() {
    let stream_err = vec![Err(AppError::Llm("API 错误 429: rate limit".to_string()))];
    let stream_ok: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "OK".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let llm = Arc::new(MockLlmProvider::new(vec![stream_err, stream_ok]));
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());
    let config = AgentLoopConfig {
        max_attempts: 3,
        model: "gpt-4".to_string(),
        session_id: "s1".to_string(),
        ..Default::default()
    };
    let abort = Arc::new(AtomicBool::new(false));
    let mut loop_ = AgentLoop::new(llm, primitive, event_bus, config, abort);
    let messages = vec![AgentMessage::User {
        text: "hi".to_string(),
    }];
    let result = loop_.run(messages).await.unwrap();
    assert_eq!(result.final_text, "OK");
}

/// 工具循环：第 1 次 LLM 返回 read_file tool call，第 2 次返回纯文本；断言 final_text 含第 2 次文本。
#[tokio::test]
async fn run_tool_loop_calls_tool_then_returns_text() {
    let stream_tool: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ToolCallDelta {
            index: 0,
            id: Some("call_1".to_string()),
            name: Some("read_file".to_string()),
            arguments_delta: Some(r#"{"path":"/tmp/x"}"#.to_string()),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "tool_calls".to_string(),
        }),
    ];
    let stream_text: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "done".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let llm = Arc::new(MockLlmProvider::new(vec![stream_tool, stream_text]));
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());
    let config = AgentLoopConfig {
        model: "gpt-4".to_string(),
        session_id: "s1".to_string(),
        ..Default::default()
    };
    let abort = Arc::new(AtomicBool::new(false));
    let mut loop_ = AgentLoop::new(llm, primitive, event_bus, config, abort);
    let messages = vec![AgentMessage::User {
        text: "read /tmp/x".to_string(),
    }];
    let result = loop_.run(messages).await.unwrap();
    assert!(result.final_text.contains("done"));
}

/// 边界：空消息列表不崩溃，run 仍可调用（LLM 可能返回错误或空回复）。
#[tokio::test]
async fn run_empty_messages_does_not_crash() {
    let stream1: Vec<Result<StreamEvent, AppError>> = vec![Ok(StreamEvent::FinishReason {
        reason: "stop".to_string(),
    })];
    let llm = Arc::new(MockLlmProvider::new(vec![stream1]));
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());
    let config = AgentLoopConfig {
        model: "gpt-4".to_string(),
        session_id: "s1".to_string(),
        ..Default::default()
    };
    let abort = Arc::new(AtomicBool::new(false));
    let mut loop_ = AgentLoop::new(llm, primitive, event_bus, config, abort);
    let messages: Vec<AgentMessage> = vec![];
    let result = loop_.run(messages).await;
    assert!(result.is_ok());
    assert!(result.unwrap().final_text.is_empty());
}

/// Abort：工具执行前/中设置 abort_signal，run 返回 Err，agent_end 含 interrupted。
#[tokio::test]
async fn run_aborts_returns_interrupted() {
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
    let abort_signal = Arc::new(AtomicBool::new(false));
    let mut loop_ = AgentLoop::new(llm, primitive, event_bus, config, Arc::clone(&abort_signal));
    let messages = vec![AgentMessage::User {
        text: "read files".to_string(),
    }];
    let abort_for_thread = Arc::clone(&abort_signal);
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(20));
        abort_for_thread.store(true, std::sync::atomic::Ordering::SeqCst);
    });
    let result = loop_.run(messages).await;
    assert!(result.is_err());
    let captured = agent_end_error.lock().unwrap().take();
    assert_eq!(captured.as_deref(), Some("interrupted"));
}

/// 事件顺序：纯文本一轮，断言 agent_start -> turn_start -> message_start -> message_update* -> message_end -> turn_end -> agent_end。
#[tokio::test]
async fn run_emits_events_in_correct_order() {
    let stream1: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "x".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let llm = Arc::new(MockLlmProvider::new(vec![stream1]));
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());
    let order: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let expected: Vec<String> = vec![
        wire::WIRE_AGENT_START.into(),
        wire::WIRE_TURN_START.into(),
        wire::WIRE_MESSAGE_START.into(),
        wire::WIRE_MESSAGE_UPDATE.into(),
        wire::WIRE_MESSAGE_END.into(),
        wire::WIRE_TURN_END.into(),
        wire::WIRE_AGENT_END.into(),
    ];
    for ev in &expected {
        let list = Arc::clone(&order);
        let name = ev.clone();
        event_bus.on(
            &name,
            Box::new(move |ctx: EventContext| {
                list.lock().unwrap().push(ctx.event_name.clone());
                Ok(())
            }),
        );
    }
    let config = AgentLoopConfig {
        model: "gpt-4".to_string(),
        session_id: "s1".to_string(),
        ..Default::default()
    };
    let abort = Arc::new(AtomicBool::new(false));
    let mut loop_ = AgentLoop::new(llm, primitive, event_bus, config, abort);
    let messages = vec![AgentMessage::User {
        text: "hi".to_string(),
    }];
    let _ = loop_.run(messages).await.unwrap();
    let observed = order.lock().unwrap().clone();
    assert_eq!(observed, expected);
}

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
    let abort = Arc::new(AtomicBool::new(false));
    let mut loop_ = AgentLoop::new_with_steering_queue(
        llm,
        primitive,
        event_bus,
        config,
        abort,
        steering_queue,
    );
    let messages = vec![AgentMessage::User {
        text: "read two files".to_string(),
    }];
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
    let abort = Arc::new(AtomicBool::new(false));
    let mut loop_ = AgentLoop::new(llm, primitive, event_bus, config, abort);
    loop_.follow_up("next".to_string());
    let messages = vec![AgentMessage::User {
        text: "first".to_string(),
    }];
    let result = loop_.run(messages).await.unwrap();
    assert!(result.final_text.contains("B"));
}

/// Fatal 401：chat_stream 直接返回 Err，run 立即终止并返回含 401 的错误。
#[tokio::test]
async fn run_fatal_401_terminates_immediately() {
    let llm = Arc::new(MockLlmProviderFatal {
        err: "API 错误 401: unauthorized".to_string(),
    });
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());
    let config = AgentLoopConfig {
        model: "gpt-4".to_string(),
        session_id: "s1".to_string(),
        ..Default::default()
    };
    let abort = Arc::new(AtomicBool::new(false));
    let mut loop_ = AgentLoop::new(llm, primitive, event_bus, config, abort);
    let messages = vec![AgentMessage::User {
        text: "hi".to_string(),
    }];
    let result = loop_.run(messages).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("401"));
}

#[tokio::test]
async fn convert_to_llm_format_compaction_summary_as_user() {
    let messages = vec![AgentMessage::CompactionSummary {
        summary: "Earlier messages summarized.".to_string(),
    }];
    let out = convert_to_llm_format(&messages);
    assert_eq!(out.len(), 1);
    assert!(matches!(
        out[0].role,
        crate::core::llm::ChatMessageRole::User
    ));
    assert_eq!(out[0].text_content(), Some("Earlier messages summarized."));
}

#[tokio::test]
async fn agent_messages_from_chat_roundtrip() {
    use crate::core::llm::{ChatMessage, ChatMessageRole};
    let chat = vec![
        ChatMessage::system("sys"),
        ChatMessage::user("u"),
        ChatMessage::assistant("a"),
    ];
    let agent = agent_messages_from_chat(&chat);
    let back = convert_to_llm_format(&agent);
    assert_eq!(back.len(), 3);
    assert_eq!(back[0].role, ChatMessageRole::System);
    assert_eq!(back[0].text_content(), Some("sys"));
    assert_eq!(back[1].role, ChatMessageRole::User);
    assert_eq!(back[1].text_content(), Some("u"));
    assert_eq!(back[2].role, ChatMessageRole::Assistant);
    assert_eq!(back[2].text_content(), Some("a"));
}

/// chat_stream 直接返回 Err（非 stream 内 Err）时也被正确分类并终止。
#[tokio::test]
async fn run_chat_stream_returns_err_is_classified() {
    let llm = Arc::new(MockLlmProviderFatal {
        err: "API 错误 503: service unavailable".to_string(),
    });
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());
    let config = AgentLoopConfig {
        model: "gpt-4".to_string(),
        session_id: "s1".to_string(),
        ..Default::default()
    };
    let abort = Arc::new(AtomicBool::new(false));
    let mut loop_ = AgentLoop::new(llm, primitive, event_bus, config, abort);
    let messages = vec![AgentMessage::User {
        text: "hi".to_string(),
    }];
    let result = loop_.run(messages).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("503"));
}

/// context_metrics_update：首次 LLM 请求前一次 + 本轮结束前一次；每次 turn_end 前均有对应轮次的 metrics（工具轮场景）。
#[tokio::test]
async fn context_metrics_update_emitted_before_turn_end() {
    use crate::core::session::manager::ContextState;
    let stream_tool: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ToolCallDelta {
            index: 0,
            id: Some("call_m1".to_string()),
            name: Some("read_file".to_string()),
            arguments_delta: Some(r#"{"path":"/tmp/m"}"#.to_string()),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "tool_calls".to_string(),
        }),
    ];
    let stream_text: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "done".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let llm = Arc::new(MockLlmProvider::new(vec![stream_tool, stream_text]));
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());
    let order: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    for ev in &[wire::WIRE_CONTEXT_METRICS_UPDATE, wire::WIRE_TURN_END] {
        let list = Arc::clone(&order);
        let name = (*ev).to_string();
        event_bus.on(
            &name,
            Box::new(move |ctx: EventContext| {
                list.lock().unwrap().push(ctx.event_name.clone());
                Ok(())
            }),
        );
    }
    let config = AgentLoopConfig {
        model: "gpt-4".to_string(),
        session_id: "s-metrics-order".to_string(),
        ..Default::default()
    };
    let abort = Arc::new(AtomicBool::new(false));
    let mut loop_ = AgentLoop::new(llm, primitive, event_bus, config, abort);
    loop_.set_context_state(Some(ContextState {
        user_turns_list: Vec::new(),
        estimate_context_chars: 100,
        context_budget_chars: 100_000,
        context_budget_tokens: 25_000,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: std::path::PathBuf::new(),
        preheat: crate::core::compaction::preheat::Preheat::new(),
    }));
    let messages = vec![AgentMessage::User {
        text: "read".to_string(),
    }];
    let _ = loop_.run(messages).await.unwrap();
    let observed = order.lock().unwrap().clone();
    let metrics_pos = observed
        .iter()
        .position(|e| e == wire::WIRE_CONTEXT_METRICS_UPDATE);
    let turn_end_pos = observed.iter().position(|e| e == wire::WIRE_TURN_END);
    assert!(
        metrics_pos.is_some(),
        "context_metrics_update should be emitted, observed: {:?}",
        observed
    );
    assert!(
        metrics_pos.unwrap() < turn_end_pos.unwrap(),
        "context_metrics_update must precede turn_end, observed: {:?}",
        observed
    );
}

/// context_metrics_update payload 包含合法字段值。
#[tokio::test]
async fn context_metrics_update_payload_contains_valid_values() {
    use crate::core::session::manager::ContextState;
    let stream_tool: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ToolCallDelta {
            index: 0,
            id: Some("call_v1".to_string()),
            name: Some("read_file".to_string()),
            arguments_delta: Some(r#"{"path":"/tmp/v"}"#.to_string()),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "tool_calls".to_string(),
        }),
    ];
    let stream_text: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "ok".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let llm = Arc::new(MockLlmProvider::new(vec![stream_tool, stream_text]));
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());
    let payloads: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
    let payloads_clone = Arc::clone(&payloads);
    event_bus.on(
        wire::WIRE_CONTEXT_METRICS_UPDATE,
        Box::new(move |ctx: EventContext| {
            payloads_clone.lock().unwrap().push(ctx.payload.clone());
            Ok(())
        }),
    );
    let config = AgentLoopConfig {
        model: "gpt-4".to_string(),
        session_id: "s-metrics-payload".to_string(),
        ..Default::default()
    };
    let abort = Arc::new(AtomicBool::new(false));
    let mut loop_ = AgentLoop::new(llm, primitive, event_bus, config, abort);
    loop_.set_context_state(Some(ContextState {
        user_turns_list: Vec::new(),
        estimate_context_chars: 2000,
        context_budget_chars: 100_000,
        context_budget_tokens: 25_000,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: std::path::PathBuf::new(),
        preheat: crate::core::compaction::preheat::Preheat::new(),
    }));
    let messages = vec![AgentMessage::User {
        text: "validate".to_string(),
    }];
    let _ = loop_.run(messages).await.unwrap();
    let captured = payloads.lock().unwrap();
    assert!(
        !captured.is_empty(),
        "should have captured at least one context_metrics_update payload"
    );
    let p = &captured[0];
    let tokens = p["inputTokensUsed"].as_u64().unwrap();
    let ratio = p["contextUtilizationRatio"].as_f64().unwrap();
    assert!(tokens > 0, "inputTokensUsed should be > 0, got {}", tokens);
    assert!(
        (0.0..=1.0).contains(&ratio),
        "contextUtilizationRatio should be in [0,1], got {}",
        ratio
    );
    assert!(p["compactionCount"].is_u64());
    assert!(p["compactionTokensFreed"].is_u64());
    assert!(p["totalToolResultBytesPersisted"].is_u64());
}

/// 多轮工具时仅发射两次 context_metrics（首请求前 + 最终 timing ⑤ 后）；compaction_count 在后一次 payload 中单调不减于前一次。
#[tokio::test]
async fn context_metrics_compaction_count_accumulates_across_rounds() {
    use crate::core::session::manager::ContextState;
    let stream_tool1: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ToolCallDelta {
            index: 0,
            id: Some("call_a1".to_string()),
            name: Some("read_file".to_string()),
            arguments_delta: Some(r#"{"path":"/a"}"#.to_string()),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "tool_calls".to_string(),
        }),
    ];
    let stream_tool2: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ToolCallDelta {
            index: 0,
            id: Some("call_a2".to_string()),
            name: Some("read_file".to_string()),
            arguments_delta: Some(r#"{"path":"/b"}"#.to_string()),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "tool_calls".to_string(),
        }),
    ];
    let stream_end: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "fin".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let llm = Arc::new(MockLlmProvider::new(vec![
        stream_tool1,
        stream_tool2,
        stream_end,
    ]));
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());
    let counts: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
    let counts_clone = Arc::clone(&counts);
    event_bus.on(
        wire::WIRE_CONTEXT_METRICS_UPDATE,
        Box::new(move |ctx: EventContext| {
            if let Some(c) = ctx.payload.get("compactionCount").and_then(|v| v.as_u64()) {
                counts_clone.lock().unwrap().push(c);
            }
            Ok(())
        }),
    );
    let config = AgentLoopConfig {
        model: "gpt-4".to_string(),
        session_id: "s-metrics-accum".to_string(),
        ..Default::default()
    };
    let abort = Arc::new(AtomicBool::new(false));
    let mut loop_ = AgentLoop::new(llm, primitive, event_bus, config, abort);
    loop_.set_context_state(Some(ContextState {
        user_turns_list: Vec::new(),
        estimate_context_chars: 100,
        context_budget_chars: 100_000,
        context_budget_tokens: 25_000,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: std::path::PathBuf::new(),
        preheat: crate::core::compaction::preheat::Preheat::new(),
    }));
    let messages = vec![AgentMessage::User {
        text: "multi".to_string(),
    }];
    let _ = loop_.run(messages).await.unwrap();
    let captured = counts.lock().unwrap();
    assert_eq!(
        captured.len(),
        2,
        "run_reasoning_loop should emit exactly 2 context_metrics_update (before first LLM + after final reply), got {}",
        captured.len()
    );
    for window in captured.windows(2) {
        assert!(
            window[1] >= window[0],
            "compaction_count should be monotonically non-decreasing: {:?}",
            *captured
        );
    }
}

/// 无 context_state 时不发射 context_metrics_update。
#[tokio::test]
async fn context_metrics_update_skipped_when_no_context_state() {
    let stream_tool: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ToolCallDelta {
            index: 0,
            id: Some("call_n1".to_string()),
            name: Some("read_file".to_string()),
            arguments_delta: Some(r#"{"path":"/tmp/n"}"#.to_string()),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "tool_calls".to_string(),
        }),
    ];
    let stream_text: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "no ctx".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let llm = Arc::new(MockLlmProvider::new(vec![stream_tool, stream_text]));
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());
    let emitted: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let emitted_clone = Arc::clone(&emitted);
    event_bus.on(
        wire::WIRE_CONTEXT_METRICS_UPDATE,
        Box::new(move |ctx: EventContext| {
            emitted_clone.lock().unwrap().push(ctx.event_name.clone());
            Ok(())
        }),
    );
    let config = AgentLoopConfig {
        model: "gpt-4".to_string(),
        session_id: "s-no-ctx".to_string(),
        ..Default::default()
    };
    let abort = Arc::new(AtomicBool::new(false));
    let mut loop_ = AgentLoop::new(llm, primitive, event_bus, config, abort);
    let messages = vec![AgentMessage::User {
        text: "hi".to_string(),
    }];
    let _ = loop_.run(messages).await.unwrap();
    let captured = emitted.lock().unwrap();
    assert!(
        captured.is_empty(),
        "context_metrics_update should NOT be emitted without context_state, got {:?}",
        *captured
    );
}

/// 纯文本回复：首 LLM 请求前一次 + timing ⑤ 后一次（共两次），最后一次在 turn_end 之前。
#[tokio::test]
async fn context_metrics_update_emitted_on_text_only_response() {
    use crate::core::session::manager::ContextState;
    let stream_text: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "hello".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let llm = Arc::new(MockLlmProvider::new(vec![stream_text]));
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());
    let order: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    for ev in &[wire::WIRE_CONTEXT_METRICS_UPDATE, wire::WIRE_TURN_END] {
        let list = Arc::clone(&order);
        let name = (*ev).to_string();
        event_bus.on(
            &name,
            Box::new(move |ctx: EventContext| {
                list.lock().unwrap().push(ctx.event_name.clone());
                Ok(())
            }),
        );
    }
    let config = AgentLoopConfig {
        model: "gpt-4".to_string(),
        session_id: "s-text-metrics".to_string(),
        ..Default::default()
    };
    let abort = Arc::new(AtomicBool::new(false));
    let mut loop_ = AgentLoop::new(llm, primitive, event_bus, config, abort);
    loop_.set_context_state(Some(ContextState {
        user_turns_list: Vec::new(),
        estimate_context_chars: 200,
        context_budget_chars: 100_000,
        context_budget_tokens: 25_000,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: std::path::PathBuf::new(),
        preheat: crate::core::compaction::preheat::Preheat::new(),
    }));
    let messages = vec![AgentMessage::User {
        text: "hi".to_string(),
    }];
    let result = loop_.run(messages).await.unwrap();
    assert_eq!(result.final_text, "hello");
    let observed = order.lock().unwrap().clone();
    let metrics_pos = observed
        .iter()
        .position(|e| e == wire::WIRE_CONTEXT_METRICS_UPDATE);
    let turn_end_pos = observed.iter().position(|e| e == wire::WIRE_TURN_END);
    assert!(
        metrics_pos.is_some(),
        "context_metrics_update should be emitted on text-only path, observed: {:?}",
        observed
    );
    assert!(
        metrics_pos.unwrap() < turn_end_pos.unwrap(),
        "context_metrics_update must precede turn_end, observed: {:?}",
        observed
    );
}
