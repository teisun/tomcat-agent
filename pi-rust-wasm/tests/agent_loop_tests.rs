//! 集成测试：AgentLoop 核心行为黑盒验证（TASK-14 T1-P1-005）。
//! 覆盖验收标准：Steering / FollowUp / Abort / AgentEvent 顺序 / 错误分类与重试。
//! 遵循 INTEGRATION_TEST_SPEC：AAA 模式、日志门禁（第 9 章）、鲁棒性边界（第 10 章）。

mod common;

use async_trait::async_trait;
use pi_wasm::{
    wire, AgentLoop, AgentLoopConfig, AppError, BashResult, ChatMessage, ChatRequest, ChatResponse,
    DefaultEventBus, DirEntry, EditFileResult, EditOperation, EventBus, EventContext, LlmProvider,
    PrimitiveExecutor, PrimitiveOperation, StreamEvent, WriteFileResult,
};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;
use tracing::{info, info_span};

// ────────────────────── Mock 实现 ──────────────────────────────────────────

/// 依序消费预设 stream 的 LLM Mock（黑盒集成测试专用，不依赖 #[cfg(test)] 内部实现）。
struct MockLlm {
    streams: Mutex<VecDeque<Vec<Result<StreamEvent, AppError>>>>,
}

impl MockLlm {
    fn new(streams: Vec<Vec<Result<StreamEvent, AppError>>>) -> Self {
        Self {
            streams: Mutex::new(streams.into()),
        }
    }
}

#[async_trait]
impl LlmProvider for MockLlm {
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
        let events = guard
            .pop_front()
            .ok_or_else(|| AppError::Llm("MockLlm: no more streams".to_string()))?;
        drop(guard);
        Ok(Box::new(tokio_stream::iter(events)))
    }
    fn count_tokens(&self, _messages: &[ChatMessage]) -> Result<u32, AppError> {
        Ok(0)
    }
}

/// 立即返回给定错误的 LLM Mock（用于 Fatal / Retryable 测试）。
struct MockLlmFatal {
    error: String,
}

#[async_trait]
impl LlmProvider for MockLlmFatal {
    fn provider_name(&self) -> &str {
        "mock_fatal"
    }
    async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, AppError> {
        Err(AppError::Llm(self.error.clone()))
    }
    async fn chat_stream(
        &self,
        _req: ChatRequest,
    ) -> Result<
        Box<dyn tokio_stream::Stream<Item = Result<StreamEvent, AppError>> + Send + Unpin>,
        AppError,
    > {
        Err(AppError::Llm(self.error.clone()))
    }
    fn count_tokens(&self, _messages: &[ChatMessage]) -> Result<u32, AppError> {
        Ok(0)
    }
}

/// 简单 PrimitiveExecutor Mock：所有方法均返回成功占位值。
struct MockPrimitive;

#[async_trait]
impl PrimitiveExecutor for MockPrimitive {
    async fn read_file(&self, path: &str, _plugin_id: &str) -> Result<String, AppError> {
        Ok(format!("content:{}", path))
    }
    async fn list_dir(&self, _path: &str, _plugin_id: &str) -> Result<Vec<DirEntry>, AppError> {
        Ok(vec![])
    }
    async fn write_file(
        &self,
        path: &str,
        content: &str,
        overwrite: bool,
        _plugin_id: &str,
    ) -> Result<WriteFileResult, AppError> {
        Ok(WriteFileResult {
            path: path.to_string(),
            written: overwrite || !content.is_empty(),
        })
    }
    async fn edit_file(
        &self,
        path: &str,
        _edits: Vec<EditOperation>,
        _plugin_id: &str,
    ) -> Result<EditFileResult, AppError> {
        Ok(EditFileResult {
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
    ) -> Result<BashResult, AppError> {
        Ok(BashResult {
            stdout: format!("out:{}", command),
            stderr: String::new(),
            exit_code: 0,
        })
    }
    async fn require_user_confirmation(
        &self,
        _operation: PrimitiveOperation,
        _preview: &str,
        _plugin_id: &str,
    ) -> Result<bool, AppError> {
        Ok(true)
    }
}

/// 第一次 execute_bash 时返回 Err（用于「工具错误不终止 Loop」测试）。
struct ErrorOnFirstBashPrimitive {
    call_count: Arc<AtomicUsize>,
}

#[async_trait]
impl PrimitiveExecutor for ErrorOnFirstBashPrimitive {
    async fn read_file(&self, path: &str, _plugin_id: &str) -> Result<String, AppError> {
        Ok(format!("content:{}", path))
    }
    async fn list_dir(&self, _path: &str, _plugin_id: &str) -> Result<Vec<DirEntry>, AppError> {
        Ok(vec![])
    }
    async fn write_file(
        &self,
        path: &str,
        content: &str,
        overwrite: bool,
        _plugin_id: &str,
    ) -> Result<WriteFileResult, AppError> {
        Ok(WriteFileResult {
            path: path.to_string(),
            written: overwrite || !content.is_empty(),
        })
    }
    async fn edit_file(
        &self,
        path: &str,
        _edits: Vec<EditOperation>,
        _plugin_id: &str,
    ) -> Result<EditFileResult, AppError> {
        Ok(EditFileResult {
            path: path.to_string(),
            applied: true,
        })
    }
    async fn execute_bash(
        &self,
        _command: &str,
        _cwd: Option<&str>,
        _plugin_id: &str,
        _argv: Option<&[String]>,
    ) -> Result<BashResult, AppError> {
        let n = self.call_count.fetch_add(1, Ordering::SeqCst);
        if n == 0 {
            Err(AppError::Permission("permission denied".to_string()))
        } else {
            Ok(BashResult {
                stdout: "ok".to_string(),
                stderr: String::new(),
                exit_code: 0,
            })
        }
    }
    async fn require_user_confirmation(
        &self,
        _operation: PrimitiveOperation,
        _preview: &str,
        _plugin_id: &str,
    ) -> Result<bool, AppError> {
        Ok(true)
    }
}

/// 工具执行时 sleep，使 Abort 能在执行期间生效。
struct SlowMockPrimitive;

#[async_trait]
impl PrimitiveExecutor for SlowMockPrimitive {
    async fn read_file(&self, path: &str, _plugin_id: &str) -> Result<String, AppError> {
        tokio::time::sleep(tokio::time::Duration::from_millis(80)).await;
        Ok(format!("content:{}", path))
    }
    async fn list_dir(&self, _path: &str, _plugin_id: &str) -> Result<Vec<DirEntry>, AppError> {
        Ok(vec![])
    }
    async fn write_file(
        &self,
        path: &str,
        content: &str,
        overwrite: bool,
        _plugin_id: &str,
    ) -> Result<WriteFileResult, AppError> {
        Ok(WriteFileResult {
            path: path.to_string(),
            written: overwrite || !content.is_empty(),
        })
    }
    async fn edit_file(
        &self,
        path: &str,
        _edits: Vec<EditOperation>,
        _plugin_id: &str,
    ) -> Result<EditFileResult, AppError> {
        Ok(EditFileResult {
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
    ) -> Result<BashResult, AppError> {
        tokio::time::sleep(tokio::time::Duration::from_millis(80)).await;
        Ok(BashResult {
            stdout: format!("out:{}", command),
            stderr: String::new(),
            exit_code: 0,
        })
    }
    async fn require_user_confirmation(
        &self,
        _operation: PrimitiveOperation,
        _preview: &str,
        _plugin_id: &str,
    ) -> Result<bool, AppError> {
        Ok(true)
    }
}

// ────────────────────── 辅助构建 ───────────────────────────────────────────

fn default_config(session_id: &str) -> AgentLoopConfig {
    AgentLoopConfig {
        model: "mock-model".to_string(),
        session_id: session_id.to_string(),
        max_attempts: 3,
        retry_base_delay_ms: 0, // 集成测试中将延迟置零，避免等待
        ..Default::default()
    }
}

fn text_stream(text: &str) -> Vec<Result<StreamEvent, AppError>> {
    vec![
        Ok(StreamEvent::ContentDelta {
            delta: text.to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ]
}

fn tool_call_stream(id: &str, name: &str, args: &str) -> Vec<Result<StreamEvent, AppError>> {
    vec![
        Ok(StreamEvent::ToolCallDelta {
            index: 0,
            id: Some(id.to_string()),
            name: Some(name.to_string()),
            arguments_delta: Some(args.to_string()),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "tool_calls".to_string(),
        }),
    ]
}

// ────────────────────── 集成测试用例 ───────────────────────────────────────

/// [AgentLoop 基础] AgentLoop 接收纯文本响应后返回 final_text
///
/// 验证：run() 返回 Ok，final_text 包含 LLM 返回的文本
/// 意义：TASK-14 5.2 三层循环骨架正向路径门禁
#[tokio::test]
async fn test_agent_loop_simple_text_response() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = info_span!("test_agent_loop_simple_text_response").entered();

    info!("Arrange: 构造 MockLlm（纯文本流）、MockPrimitive、DefaultEventBus");
    let llm = Arc::new(MockLlm::new(vec![text_stream("Hello from AgentLoop")]));
    let primitive = Arc::new(MockPrimitive);
    let event_bus = Arc::new(DefaultEventBus::new());
    let config = default_config("sess-simple");
    let abort = CancellationToken::new();
    let mut agent = AgentLoop::new(llm, primitive, event_bus, config, abort);
    let messages = vec![ChatMessage::user("say hello")];

    info!("Act: 调用 AgentLoop::run()");
    let result = tokio::time::timeout(std::time::Duration::from_secs(10), agent.run(messages))
        .await
        .map_err(|_| "run() 超时 10s")?
        .unwrap();

    info!("Assert: final_text 包含 LLM 回复: {:?}", result.final_text);
    assert!(
        result.final_text.contains("Hello from AgentLoop"),
        "final_text 应包含 LLM 回复，实际: {:?}",
        result.final_text
    );

    Ok(())
}

/// [Abort 机制] abort() 调用后当前工具完成即中断，agent_end 携带 interrupted 错误
///
/// 验证：run() 返回 Err，event_bus 捕获到 agent_end.error="interrupted"
/// 意义：TASK-14 5.5 AtomicBool Abort 信号——防止恶意/超时工具调用失控（5.5 验收标准）
#[tokio::test]
async fn test_agent_loop_abort_stops_after_current_tool() -> Result<(), Box<dyn std::error::Error>>
{
    common::setup_logging();
    let _span = info_span!("test_agent_loop_abort_stops_after_current_tool").entered();

    info!("Arrange: LLM 返回两个 read_file 工具调用，SlowMockPrimitive 增加延迟触发 abort");
    let streams = vec![vec![
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
    ]];
    let llm = Arc::new(MockLlm::new(streams));
    let primitive = Arc::new(SlowMockPrimitive);
    let event_bus = Arc::new(DefaultEventBus::new());

    let captured_error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let err_clone = Arc::clone(&captured_error);
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

    let config = default_config("sess-abort");
    let abort_signal = CancellationToken::new();
    let mut agent = AgentLoop::new(llm, primitive, event_bus, config, abort_signal.clone());
    let messages = vec![ChatMessage::user("read files")];

    info!("Act: 在后台线程 20ms 后触发 abort()，同时 run() 执行中");
    let abort_for_thread = abort_signal.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(20));
        abort_for_thread.cancel();
    });

    let result = tokio::time::timeout(std::time::Duration::from_secs(10), agent.run(messages))
        .await
        .map_err(|_| "run() 超时 10s")?;

    info!(
        "Assert: run() 返回 Interrupted，agent_end.error=interrupted; result={:?}",
        result.is_interrupted()
    );
    assert!(result.is_interrupted(), "abort 后 run() 应返回 Interrupted");
    let captured = captured_error.lock().unwrap().take();
    assert_eq!(
        captured.as_deref(),
        Some("interrupted"),
        "agent_end.error 应为 interrupted"
    );

    Ok(())
}

/// [FollowUp 机制] follow_up() 追加消息后 Loop 在同一会话上下文继续第二轮
///
/// 验证：run() 返回 Ok，final_text 包含第二轮 LLM 回复（"B"）
/// 意义：TASK-14 5.4 FollowUp 机制——支持会话追加而无需新建 Loop
#[tokio::test]
async fn test_agent_loop_follow_up_continues_in_same_session(
) -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = info_span!("test_agent_loop_follow_up_continues_in_same_session").entered();

    info!("Arrange: 两轮 LLM stream（A/B），通过 follow_up() 预注入第二轮触发消息");
    let llm = Arc::new(MockLlm::new(vec![text_stream("A"), text_stream("B")]));
    let primitive = Arc::new(MockPrimitive);
    let event_bus = Arc::new(DefaultEventBus::new());
    let config = default_config("sess-followup");
    let abort = CancellationToken::new();
    let mut agent = AgentLoop::new(llm, primitive, event_bus, config, abort);

    agent.follow_up("continue please".to_string());
    let messages = vec![ChatMessage::user("first message")];

    info!("Act: 调用 AgentLoop::run()，follow_up 已预注入");
    let result = tokio::time::timeout(std::time::Duration::from_secs(10), agent.run(messages))
        .await
        .map_err(|_| "run() 超时 10s")?
        .unwrap();

    info!(
        "Assert: final_text 包含第二轮回复 'B': {:?}",
        result.final_text
    );
    assert!(
        result.final_text.contains('B'),
        "FollowUp 后 final_text 应包含第二轮回复 'B'，实际: {:?}",
        result.final_text
    );

    Ok(())
}

/// [工具错误不终止 Loop] 工具执行出错后错误内容回注 LLM，Loop 继续并返回最终文本
///
/// 验证：run() 返回 Ok（不 panic），final_text 包含 LLM 在收到错误后的回复
/// 意义：TASK-14 验收标准「工具错误：不终止 Loop，错误内容回注 LLM」
#[tokio::test]
async fn test_agent_loop_tool_error_does_not_terminate_loop(
) -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = info_span!("test_agent_loop_tool_error_does_not_terminate_loop").entered();

    info!("Arrange: Stream1 返回 execute_bash 工具调用；ErrorOnFirstBash 首次返回 Err；Stream2 返回 recovered 文本");
    let stream_tool = tool_call_stream("bash1", "execute_bash", r#"{"command":"ls","cwd":null}"#);
    let stream_recovered = text_stream("recovered after error");
    let llm = Arc::new(MockLlm::new(vec![stream_tool, stream_recovered]));
    let call_count = Arc::new(AtomicUsize::new(0));
    let primitive = Arc::new(ErrorOnFirstBashPrimitive {
        call_count: Arc::clone(&call_count),
    });
    let event_bus = Arc::new(DefaultEventBus::new());
    let config = default_config("sess-tool-error");
    let abort = CancellationToken::new();
    let mut agent = AgentLoop::new(llm, primitive, event_bus, config, abort);
    let messages = vec![ChatMessage::user("run bash")];

    info!("Act: 调用 run()，工具首次执行将 Err");
    let result = tokio::time::timeout(std::time::Duration::from_secs(10), agent.run(messages))
        .await
        .map_err(|_| "run() 超时 10s")?
        .unwrap();

    info!(
        "Assert: run() 返回 Ok，final_text 包含 recovered: {:?}",
        result.final_text
    );
    assert!(
        result.final_text.contains("recovered"),
        "工具错误后 Loop 应继续并返回 LLM 后续回复，实际: {:?}",
        result.final_text
    );

    Ok(())
}

/// [可重试错误退避重试] RateLimit(429) 自动退避重试后成功
///
/// 验证：第一次 LLM 返回 429 错误；第二次（重试）返回成功文本；run() 返回 Ok
/// 意义：TASK-14 5.7 错误分类 Retryable 指数退避——集成验证重试路径（鲁棒性 §10）
#[tokio::test]
async fn test_agent_loop_retryable_error_retries_and_succeeds(
) -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = info_span!("test_agent_loop_retryable_error_retries_and_succeeds").entered();

    info!(
        "Arrange: Stream1 返回 429 错误事件；Stream2 返回成功文本；retry_base_delay_ms=0 避免等待"
    );
    let stream_err = vec![Err(AppError::Llm(
        "API 错误 429: rate limit exceeded".to_string(),
    ))];
    let stream_ok = text_stream("retried ok");
    let llm = Arc::new(MockLlm::new(vec![stream_err, stream_ok]));
    let primitive = Arc::new(MockPrimitive);
    let event_bus = Arc::new(DefaultEventBus::new());
    let config = AgentLoopConfig {
        model: "mock-model".to_string(),
        session_id: "sess-retry".to_string(),
        max_attempts: 3,
        retry_base_delay_ms: 0,
        ..Default::default()
    };
    let abort = CancellationToken::new();
    let mut agent = AgentLoop::new(llm, primitive, event_bus, config, abort);
    let messages = vec![ChatMessage::user("hi")];

    info!("Act: 调用 run()，期望自动重试后成功");
    let result = tokio::time::timeout(std::time::Duration::from_secs(10), agent.run(messages))
        .await
        .map_err(|_| "run() 超时 10s")?
        .unwrap();

    info!(
        "Assert: run() 返回 Ok，final_text='retried ok': {:?}",
        result.final_text
    );
    assert_eq!(
        result.final_text, "retried ok",
        "429 重试后应返回第二次 LLM 文本，实际: {:?}",
        result.final_text
    );

    Ok(())
}

/// [工具事件 pi-mono 五段序] 单工具轮内先发观察向 tool_execution_*，再发钩子 tool_call/tool_result
///
/// 验证：EventBus 上事件名序列为 tool_execution_start → tool_call → tool_result → tool_execution_end（子序列）
/// 意义：与 [events.md](../openspec/specs/architecture/plugin-system/events.md) 工具链对照一致
#[tokio::test]
async fn test_agent_loop_tool_pi_mono_event_subsequence() -> Result<(), Box<dyn std::error::Error>>
{
    common::setup_logging();
    let _span = info_span!("test_agent_loop_tool_pi_mono_event_subsequence").entered();

    let stream_tool = tool_call_stream("bash1", "execute_bash", r#"{"command":"ls","cwd":null}"#);
    let stream_text = text_stream("done");
    let llm = Arc::new(MockLlm::new(vec![stream_tool, stream_text]));
    let primitive = Arc::new(MockPrimitive);
    let event_bus = Arc::new(DefaultEventBus::new());

    let observed: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let watch: Vec<&str> = vec![
        wire::WIRE_AGENT_START,
        wire::WIRE_TURN_START,
        wire::WIRE_MESSAGE_START,
        wire::WIRE_MESSAGE_END,
        wire::WIRE_TOOL_EXECUTION_START,
        wire::WIRE_TOOL_CALL,
        wire::WIRE_TOOL_RESULT,
        wire::WIRE_TOOL_EXECUTION_END,
        wire::WIRE_TURN_END,
        wire::WIRE_TURN_START,
        wire::WIRE_MESSAGE_START,
        wire::WIRE_MESSAGE_UPDATE,
        wire::WIRE_MESSAGE_END,
        wire::WIRE_TURN_END,
        wire::WIRE_AGENT_END,
    ];
    for ev in &watch {
        let list = Arc::clone(&observed);
        let name = (*ev).to_string();
        event_bus.on(
            &name,
            Box::new(move |ctx: EventContext| {
                list.lock().unwrap().push(ctx.event_name.clone());
                Ok(())
            }),
        );
    }

    let config = default_config("sess-tool-pi-mono-order");
    let abort = CancellationToken::new();
    let mut agent = AgentLoop::new(llm, primitive, event_bus, config, abort);
    let messages = vec![ChatMessage::user("run ls")];

    let _ = tokio::time::timeout(std::time::Duration::from_secs(10), agent.run(messages))
        .await
        .map_err(|_| "run() 超时 10s")?
        .unwrap();

    let actual = observed.lock().unwrap().clone();
    let needle = [
        wire::WIRE_TOOL_EXECUTION_START,
        wire::WIRE_TOOL_CALL,
        wire::WIRE_TOOL_RESULT,
        wire::WIRE_TOOL_EXECUTION_END,
    ];
    let mut j = 0usize;
    for ev in &actual {
        if j < needle.len() && ev.as_str() == needle[j] {
            j += 1;
        }
    }
    assert_eq!(
        j,
        needle.len(),
        "应出现子序列 {:?}，实际事件: {:?}",
        needle,
        actual
    );

    Ok(())
}

/// [Fatal 401 立即终止] 401 错误不重试，run() 立即返回 Err 且消息含 401
///
/// 验证：run() 返回 Err，错误描述包含 "401"；不会触发重试（即不消耗第二个 stream）
/// 意义：TASK-14 5.7 Fatal 错误立即终止——防止无效 API Key 情况下无限重试（鲁棒性 §10）
#[tokio::test]
async fn test_agent_loop_fatal_error_401_terminates_immediately(
) -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = info_span!("test_agent_loop_fatal_error_401_terminates_immediately").entered();

    info!("Arrange: MockLlmFatal 返回 401 错误，max_attempts=3 但不应重试");
    let llm = Arc::new(MockLlmFatal {
        error: "API 错误 401: unauthorized".to_string(),
    });
    let primitive = Arc::new(MockPrimitive);
    let event_bus = Arc::new(DefaultEventBus::new());
    let config = AgentLoopConfig {
        model: "mock-model".to_string(),
        session_id: "sess-fatal".to_string(),
        max_attempts: 3,
        retry_base_delay_ms: 0,
        ..Default::default()
    };
    let abort = CancellationToken::new();
    let mut agent = AgentLoop::new(llm, primitive, event_bus, config, abort);
    let messages = vec![ChatMessage::user("hi")];

    info!("Act: 调用 run()，期望立即返回 Err（无重试）");
    let result = tokio::time::timeout(std::time::Duration::from_secs(10), agent.run(messages))
        .await
        .map_err(|_| "run() 超时 10s")?;

    info!("Assert: run() 返回 Err，错误含 '401'");
    assert!(result.is_err(), "401 Fatal 应导致 run() 返回 Err");
    let err_str = result.unwrap_err().to_string();
    assert!(
        err_str.contains("401"),
        "错误信息应包含 '401'，实际: {:?}",
        err_str
    );

    Ok(())
}

/// [AgentEvent 发布顺序] 纯文本一轮正向路径下，事件顺序符合规范
///
/// 验证：agent_start → turn_start → message_start → message_update → message_end → turn_end → agent_end
/// 意义：TASK-14 5.6 AgentEvent 全生命周期节点发布——外部订阅者依赖事件顺序（集成验收标准）
#[tokio::test]
async fn test_agent_loop_events_published_in_correct_order(
) -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = info_span!("test_agent_loop_events_published_in_correct_order").entered();

    info!("Arrange: 注册 agent_start/turn_start/message_start/message_update/message_end/turn_end/agent_end 事件收集器");
    let llm = Arc::new(MockLlm::new(vec![text_stream("event-order")]));
    let primitive = Arc::new(MockPrimitive);
    let event_bus = Arc::new(DefaultEventBus::new());

    let observed: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let tracked_events: Vec<String> = vec![
        wire::WIRE_AGENT_START.into(),
        wire::WIRE_TURN_START.into(),
        wire::WIRE_MESSAGE_START.into(),
        wire::WIRE_MESSAGE_UPDATE.into(),
        wire::WIRE_MESSAGE_END.into(),
        wire::WIRE_TURN_END.into(),
        wire::WIRE_AGENT_END.into(),
    ];
    for ev_name in &tracked_events {
        let list = Arc::clone(&observed);
        let name = ev_name.clone();
        event_bus.on(
            &name,
            Box::new(move |ctx: EventContext| {
                list.lock().unwrap().push(ctx.event_name.clone());
                Ok(())
            }),
        );
    }

    let config = default_config("sess-events");
    let abort = CancellationToken::new();
    let mut agent = AgentLoop::new(llm, primitive, event_bus, config, abort);
    let messages = vec![ChatMessage::user("hi")];

    info!("Act: 调用 run()");
    let _ = tokio::time::timeout(std::time::Duration::from_secs(10), agent.run(messages))
        .await
        .map_err(|_| "run() 超时 10s")?
        .unwrap();

    info!("Assert: 验证事件发布顺序与规范一致");
    let actual = observed.lock().unwrap().clone();
    assert_eq!(
        actual, tracked_events,
        "事件顺序应为 {:?}，实际为 {:?}",
        tracked_events, actual
    );

    Ok(())
}

/// [Steering 机制] 预注入 Steering 消息后，工具批次中第二个工具被跳过，Loop 继续
///
/// 验证：LLM 返回两个工具调用，Steering 注入后仅第一个工具执行；后续 LLM 收到 steering 返回 "steered"
/// 意义：TASK-14 5.3 Steering 机制——外部线程可中断当前工具批次，实现实时重定向（5.3 验收标准）
#[tokio::test]
async fn test_agent_loop_steering_skips_remaining_tools() -> Result<(), Box<dyn std::error::Error>>
{
    common::setup_logging();
    let _span = info_span!("test_agent_loop_steering_skips_remaining_tools").entered();

    info!("Arrange: LLM 返回两个 read_file 工具调用；预注入 steer(\"redirect\")；第二轮 LLM 返回 steered");
    let stream_tools = vec![
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
    let stream_steered = text_stream("steered");
    let llm = Arc::new(MockLlm::new(vec![stream_tools, stream_steered]));
    let primitive = Arc::new(MockPrimitive);
    let event_bus = Arc::new(DefaultEventBus::new());
    let config = default_config("sess-steering");
    let abort = CancellationToken::new();
    let mut agent = AgentLoop::new(llm, primitive, event_bus, config, abort);

    // 预注入 Steering 消息，工具批次第一个完成后将被检测并跳过剩余工具
    agent.steer("redirect me now".to_string());
    let messages = vec![ChatMessage::user("read two files")];

    info!("Act: 调用 run()，Steering 已预注入");
    let result = tokio::time::timeout(std::time::Duration::from_secs(10), agent.run(messages))
        .await
        .map_err(|_| "run() 超时 10s")?
        .unwrap();

    info!("Assert: final_text 包含 'steered': {:?}", result.final_text);
    assert!(
        result.final_text.contains("steered"),
        "Steering 后 final_text 应包含 LLM 第二轮回复 'steered'，实际: {:?}",
        result.final_text
    );

    Ok(())
}

/// [ContextMetricsUpdate 事件发射] 设置 ContextState 后：首次 LLM 请求前 + 本轮最终回复后各一次
///
/// 验证：至少一次 context_metrics_update，payload 含合法字段；首次 metrics 出现在首次 turn_end 之前
/// 意义：ContextMetricsUpdate 双点发射（中间 tool round 不发）——可观测性与 CLI 刷屏平衡
#[tokio::test]
async fn test_context_metrics_update_event_published() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = info_span!("test_context_metrics_update_event_published").entered();

    info!("Arrange: LLM 返回 read_file 工具调用 → 纯文本结束；AgentLoop 注入 ContextState");
    let stream_tool = tool_call_stream("cm1", "read_file", r#"{"path":"/tmp/cm"}"#);
    let stream_text = text_stream("metrics done");
    let llm = Arc::new(MockLlm::new(vec![stream_tool, stream_text]));
    let primitive = Arc::new(MockPrimitive);
    let event_bus = Arc::new(DefaultEventBus::new());

    let events_order: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let payloads: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));

    // 订阅 context_metrics_update：收集顺序 + payload
    let order_clone = Arc::clone(&events_order);
    let payloads_clone = Arc::clone(&payloads);
    event_bus.on(
        wire::WIRE_CONTEXT_METRICS_UPDATE,
        Box::new(move |ctx: EventContext| {
            order_clone.lock().unwrap().push(ctx.event_name.clone());
            payloads_clone.lock().unwrap().push(ctx.payload.clone());
            Ok(())
        }),
    );
    // 订阅 turn_end：收集顺序
    let order_clone2 = Arc::clone(&events_order);
    event_bus.on(
        wire::WIRE_TURN_END,
        Box::new(move |ctx: EventContext| {
            order_clone2.lock().unwrap().push(ctx.event_name.clone());
            Ok(())
        }),
    );

    let config = default_config("sess-ctx-metrics");
    let abort = CancellationToken::new();
    let mut agent = AgentLoop::new(llm, primitive, event_bus, config, abort);

    use pi_wasm::ContextState;
    agent.set_context_state(Some(ContextState {
        messages: Vec::new(),
        estimate_context_chars: 500,
        context_budget_chars: 100_000,
        context_budget_tokens: 25_000,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: std::path::PathBuf::new(),
        preheat: pi_wasm::core::compaction::preheat::Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    }));

    let messages = vec![ChatMessage::user("test metrics")];

    info!("Act: 调用 AgentLoop::run()");
    let result = tokio::time::timeout(std::time::Duration::from_secs(10), agent.run(messages))
        .await
        .map_err(|_| "run() 超时 10s")?
        .unwrap();

    info!("Assert: context_metrics_update 事件被发射、payload 合法、顺序正确");
    assert!(result.final_text.contains("metrics done"));

    let captured_payloads = payloads.lock().unwrap();
    assert!(
        !captured_payloads.is_empty(),
        "至少应捕获一个 context_metrics_update 事件"
    );
    let p = &captured_payloads[0];
    assert!(
        p["inputTokensUsed"].as_u64().is_some(),
        "payload 应含 inputTokensUsed"
    );
    assert!(
        p["contextUtilizationRatio"].as_f64().is_some(),
        "payload 应含 contextUtilizationRatio"
    );
    assert!(
        p["compactionCount"].as_u64().is_some(),
        "payload 应含 compactionCount"
    );
    assert!(
        p["compactionTokensFreed"].as_u64().is_some(),
        "payload 应含 compactionTokensFreed"
    );
    assert!(
        p["totalToolResultBytesPersisted"].as_u64().is_some(),
        "payload 应含 totalToolResultBytesPersisted"
    );
    assert!(
        p["preheatResultPending"].is_boolean(),
        "payload 应含 preheatResultPending"
    );

    let order = events_order.lock().unwrap();
    let metrics_pos = order
        .iter()
        .position(|e| e == wire::WIRE_CONTEXT_METRICS_UPDATE);
    let turn_end_pos = order.iter().position(|e| e == wire::WIRE_TURN_END);
    assert!(
        metrics_pos.is_some() && turn_end_pos.is_some(),
        "应同时捕获 context_metrics_update 和 turn_end，实际: {:?}",
        *order
    );
    assert!(
        metrics_pos.unwrap() < turn_end_pos.unwrap(),
        "context_metrics_update 应在 turn_end 之前，实际: {:?}",
        *order
    );

    Ok(())
}

/// [边界：空消息列表] 传入空消息列表时 run() 不崩溃，返回 Ok 且 final_text 为空
///
/// 验证：run([]) 返回 Ok("")（鲁棒性边界：不触发 panic，符合 INTEGRATION_TEST_ROBUSTNESS §2）
/// 意义：防御性边界——空上下文不应导致 AgentLoop 崩溃
#[tokio::test]
async fn test_agent_loop_empty_messages_does_not_crash() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = info_span!("test_agent_loop_empty_messages_does_not_crash").entered();

    info!("Arrange: 空消息列表 + LLM 返回 stop");
    let llm = Arc::new(MockLlm::new(vec![vec![Ok(StreamEvent::FinishReason {
        reason: "stop".to_string(),
    })]]));
    let primitive = Arc::new(MockPrimitive);
    let event_bus = Arc::new(DefaultEventBus::new());
    let config = default_config("sess-empty");
    let abort = CancellationToken::new();
    let mut agent = AgentLoop::new(llm, primitive, event_bus, config, abort);

    info!("Act: run([]) 调用");
    let result = tokio::time::timeout(std::time::Duration::from_secs(10), agent.run(vec![]))
        .await
        .map_err(|_| "run() 超时 10s")?
        .unwrap();

    info!(
        "Assert: 返回 Ok，final_text 为空字符串: {:?}",
        result.final_text
    );
    assert!(
        result.final_text.is_empty(),
        "空消息时 final_text 应为空，实际: {:?}",
        result.final_text
    );

    Ok(())
}
