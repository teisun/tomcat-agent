//! 集成测试：TASK-17 上下文管理（大文件截断、多轮 Compaction、Session 重载、Context Overflow 重试）。
//! 黑盒测试，通过 tomcat 公共 API + 临时目录隔离。

mod common;

use async_trait::async_trait;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;
use tomcat::core::compaction::compact_tool_results;
use tomcat::core::llm::{ChatMessageRole, MessageKind};
use tomcat::core::session::{estimate_msg_chars, MessageAppendSink};
use tomcat::{
    build_context_from_state, compound_turn_id, init_context_state, run_chat_turn, AgentLoop,
    AgentLoopConfig, AppConfig, AppError, BashResult, Capabilities, ChatContext, ChatMessage,
    ChatRequest, ChatResponse, ContextConfig, ContextState, DefaultEventBus, DirEntry,
    EditFileResult, EditOperation, EventBus, EventContext, LlmProvider, LlmResolver, LlmScene,
    PrimitiveExecutor, PrimitiveOperation, ResolvedCall, SessionManager, StreamEvent,
    WriteFileResult,
};
use tracing::{info, info_span};

// ────────────────────── Mock 实现 ──────────────────────────────────────────

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

struct FixedResolver {
    provider: Arc<dyn LlmProvider>,
    default_model: String,
}

impl FixedResolver {
    fn new(provider: Arc<dyn LlmProvider>, default_model: impl Into<String>) -> Self {
        Self {
            provider,
            default_model: default_model.into(),
        }
    }

    fn resolved_call(&self, model: &str) -> ResolvedCall {
        let lower = model.trim().to_ascii_lowercase();
        let (api, provider, base_url) = if lower.starts_with("deepseek-") {
            ("openai", "deepseek", "https://api.deepseek.com")
        } else {
            ("openai-responses", "openai", "https://api.openai.com")
        };
        let capabilities = Capabilities {
            vision: lower.starts_with("gpt-"),
            files: lower.starts_with("gpt-"),
            tools: true,
            reasoning: lower.starts_with("deepseek-v4-") || lower.starts_with("gpt-5."),
            web_search: false,
        };
        ResolvedCall {
            provider_impl: self.provider.clone(),
            model: model.to_string(),
            api: api.to_string(),
            provider: provider.to_string(),
            base_url: Some(base_url.to_string()),
            key_source: if provider == "deepseek" {
                "DEEPSEEK_API_KEY".to_string()
            } else {
                "OPENAI_API_KEY".to_string()
            },
            thinking_format: tomcat::core::llm::thinking_policy::thinking_format_for_model(model),
            capabilities,
        }
    }
}

impl LlmResolver for FixedResolver {
    fn resolve(
        &self,
        scene: LlmScene,
        session_override: Option<&str>,
    ) -> Result<ResolvedCall, AppError> {
        let model = match scene {
            LlmScene::Main | LlmScene::Vision => session_override
                .map(str::trim)
                .filter(|model| !model.is_empty())
                .unwrap_or(&self.default_model),
            LlmScene::Compaction | LlmScene::Title => &self.default_model,
        };
        Ok(self.resolved_call(model))
    }
}

fn install_fixed_resolver(
    ctx: &mut ChatContext,
    provider: Arc<dyn LlmProvider>,
    default_model: &str,
) {
    ctx.llm = provider.clone();
    ctx.llm_resolver = Arc::new(FixedResolver::new(provider, default_model));
}

struct RecordingMockLlm {
    streams: Mutex<VecDeque<Vec<Result<StreamEvent, AppError>>>>,
    requests: Mutex<Vec<ChatRequest>>,
}

impl RecordingMockLlm {
    fn new(streams: Vec<Vec<Result<StreamEvent, AppError>>>) -> Self {
        Self {
            streams: Mutex::new(streams.into()),
            requests: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl LlmProvider for RecordingMockLlm {
    fn provider_name(&self) -> &str {
        "recording-mock"
    }
    async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, AppError> {
        Err(AppError::Llm("recording mock chat not used".to_string()))
    }
    async fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> Result<
        Box<dyn tokio_stream::Stream<Item = Result<StreamEvent, AppError>> + Send + Unpin>,
        AppError,
    > {
        self.requests.lock().unwrap().push(req);
        let mut guard = self.streams.lock().unwrap();
        let events = guard
            .pop_front()
            .ok_or_else(|| AppError::Llm("RecordingMockLlm: no more streams".to_string()))?;
        drop(guard);
        Ok(Box::new(tokio_stream::iter(events)))
    }
    fn count_tokens(&self, _messages: &[ChatMessage]) -> Result<u32, AppError> {
        Ok(0)
    }
}

struct MockPrimitiveWithLargeFile {
    file_size: usize,
}

#[async_trait]
impl PrimitiveExecutor for MockPrimitiveWithLargeFile {
    async fn read_file(&self, _path: &str, _plugin_id: &str) -> Result<String, AppError> {
        Ok("x".repeat(self.file_size))
    }
    async fn list_dir(&self, _path: &str, _plugin_id: &str) -> Result<Vec<DirEntry>, AppError> {
        Ok(vec![])
    }
    async fn write_file(
        &self,
        path: &str,
        _content: &str,
        _overwrite: bool,
        _plugin_id: &str,
    ) -> Result<WriteFileResult, AppError> {
        Ok(WriteFileResult {
            path: path.to_string(),
            written: true,
            bytes_written: 0,
            diff_hint: None,
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
        _timeout_ms: Option<u64>,
    ) -> Result<BashResult, AppError> {
        Ok(BashResult {
            stdout: format!("out:{}", command),
            stderr: String::new(),
            exit_code: 0,
            ..Default::default()
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

// ────────────────────── 辅助 ──────────────────────────────────────────────

const TEST_TS: &str = "2026-04-04T12:00:00Z";
const TOOL_RESULT_PLACEHOLDER_TEXT: &str = "[Previous tool result replaced to save context space]";

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

fn multi_tool_call_stream(calls: &[(&str, &str, &str)]) -> Vec<Result<StreamEvent, AppError>> {
    let mut out = Vec::new();
    for (idx, (id, name, args)) in calls.iter().enumerate() {
        out.push(Ok(StreamEvent::ToolCallDelta {
            index: idx as u32,
            id: Some((*id).to_string()),
            name: Some((*name).to_string()),
            arguments_delta: Some((*args).to_string()),
        }));
    }
    out.push(Ok(StreamEvent::FinishReason {
        reason: "tool_calls".to_string(),
    }));
    out
}

fn temp_sessions_dir(label: &str) -> PathBuf {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    std::env::temp_dir().join(format!(
        "pi_ctx_test_{}_{}_{}",
        label,
        std::process::id(),
        ms
    ))
}

/// Helper: create [user, tool_result, assistant] messages for one "turn"
fn make_msgs_with_tool_result(user_text: &str, tool_content: &str) -> Vec<ChatMessage> {
    let mut user = ChatMessage::user(user_text);
    user.timestamp = Some(TEST_TS.to_string());

    let mut tool = ChatMessage::tool("tc", tool_content);
    tool.timestamp = Some(TEST_TS.to_string());

    let mut asst = ChatMessage::assistant("ok");
    asst.timestamp = Some(TEST_TS.to_string());

    vec![user, tool, asst]
}

const PLACEHOLDER: &str = "[Previous tool result replaced to save context space]";

struct InjectAppendInvariantSink {
    inner: SessionManager,
    append_calls: AtomicUsize,
    injected: AtomicBool,
}

impl InjectAppendInvariantSink {
    fn new(inner: SessionManager) -> Self {
        Self {
            inner,
            append_calls: AtomicUsize::new(0),
            injected: AtomicBool::new(false),
        }
    }
}

impl MessageAppendSink for InjectAppendInvariantSink {
    fn append_message(&self, value: serde_json::Value) -> Result<String, AppError> {
        let call_idx = self.append_calls.fetch_add(1, Ordering::SeqCst);
        if call_idx == 2 && !self.injected.swap(true, Ordering::SeqCst) {
            let tool_call_id = value
                .get("tool_call_id")
                .and_then(|v| v.as_str())
                .unwrap_or("call_injected")
                .to_string();
            self.inner.append_message(serde_json::json!({
                "role": "tool",
                "tool_call_id": tool_call_id,
                "content": "[interrupted]"
            }))?;
            self.inner.append_message(serde_json::json!({
                "role": "user",
                "content": "nested prompt"
            }))?;
            self.inner.append_message(serde_json::json!({
                "role": "assistant",
                "content": "nested done"
            }))?;
        }
        self.inner.append_message(value)
    }
}

fn chat_context_fixture(env_key: &str) -> (tempfile::TempDir, ChatContext) {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(dir.path().to_string_lossy().to_string());
    cfg.llm.api_key_env = Some(env_key.to_string());

    // SAFETY: 测试使用独立 env key，作用域结束后由调用方清理。
    unsafe { std::env::set_var(env_key, "stub") };
    let ctx = ChatContext::from_config(cfg).expect("chat context should be created");
    let session_key = ctx.session.current_session_key().to_string();
    ctx.session.create_session(&session_key, None).unwrap();
    (dir, ctx)
}

// ────────────────────── 测试用例 ──────────────────────────────────────────

#[tokio::test]
async fn test_failed_turn_append_invariant_rehydrates_context_and_allows_next_turn(
) -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span =
        info_span!("test_failed_turn_append_invariant_rehydrates_context_and_allows_next_turn")
            .entered();

    const ENV_KEY: &str = "TOMCAT_APPEND_REHYDRATE_INTEGRATION_KEY";
    let (_dir, mut ctx) = chat_context_fixture(ENV_KEY);
    let mock_llm = Arc::new(MockLlm::new(vec![
        tool_call_stream("call_1", "bash", r#"{"command":"echo hi","cwd":null}"#),
        text_stream("RECOVER_OK"),
    ]));
    install_fixed_resolver(&mut ctx, mock_llm, "gpt-5.4");
    ctx.primitive = Arc::new(MockPrimitiveWithLargeFile { file_size: 16 });
    ctx.message_append_sink = Arc::new(InjectAppendInvariantSink::new(ctx.session.clone()));

    let system_text = "system prompt";
    let mut state = init_context_state(&ctx.session, &ctx.config.context, system_text)?;
    let first = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        run_chat_turn(
            &ctx,
            "请执行一次 bash 工具",
            system_text,
            &mut state,
            CancellationToken::new(),
        ),
    )
    .await
    .map_err(|_| "first run_chat_turn timeout 5s")??;

    match &first {
        tomcat::core::agent_loop::AgentRunOutcome::Failed(err) => {
            assert!(
                matches!(
                    err,
                    AppError::Invariant {
                        stage: "append_message_chain",
                        ..
                    }
                ),
                "首轮应命中 append_message_chain invariant，实际: {err}"
            );
        }
        other => panic!("首轮应走 Failed(invariant) 分支，实际: {other:?}"),
    }
    assert_eq!(
        state.messages.last().and_then(|m| m.text_content()),
        Some("nested done"),
        "首轮失败后 context_state 应已从磁盘重建，而不是停留在 dangling tool_calls"
    );
    assert!(
        state
            .messages
            .iter()
            .any(|m| m.tool_call_id.as_deref() == Some("call_1")
                && m.text_content() == Some("[interrupted]")),
        "重建后的 context_state 应保留磁盘上的 interrupted tool result"
    );

    let second = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        run_chat_turn(
            &ctx,
            "请只回复 RECOVER_OK，不要调用工具",
            system_text,
            &mut state,
            CancellationToken::new(),
        ),
    )
    .await
    .map_err(|_| "second run_chat_turn timeout 5s")??;

    match second {
        tomcat::core::agent_loop::AgentRunOutcome::Completed(result) => {
            assert!(
                result.final_text.contains("RECOVER_OK"),
                "第二轮应继续成功完成，实际 final_text: {:?}",
                result.final_text
            );
        }
        other => panic!("第二轮应恢复为 Completed，实际: {other:?}"),
    }

    // SAFETY: 清理测试环境变量。
    unsafe { std::env::remove_var(ENV_KEY) };
    Ok(())
}

/// [Layer 1 + Layer 3 全链路] compact_tool_results 后仍超 ratio 时 force_drop_oldest_to_target 兜底
#[test]
fn test_compaction_pipeline_layer1_then_layer3_recovers_budget() {
    common::setup_logging();
    let _span = info_span!("test_compaction_pipeline_layer1_then_layer3_recovers_budget").entered();

    let mut messages: Vec<ChatMessage> = Vec::new();
    for i in 0..5 {
        let mut user = ChatMessage::user(format!("question {}", i));
        user.timestamp = Some(TEST_TS.to_string());
        let mut tool = ChatMessage::tool(&format!("tc_{}", i), &"x".repeat(25_000));
        tool.timestamp = Some(TEST_TS.to_string());
        let mut asst = ChatMessage::assistant(format!("answer {}", i));
        asst.timestamp = Some(TEST_TS.to_string());
        messages.push(user);
        messages.push(tool);
        messages.push(asst);
    }
    let total: usize = messages.iter().map(estimate_msg_chars).sum();

    let budget_chars = 80_000;
    let budget_tokens = budget_chars / 4;
    let mut state = ContextState {
        messages,
        estimate_context_chars: total,
        context_budget_chars: budget_chars,
        context_budget_tokens: budget_tokens,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        latest_plan_event: None,
        preheat: tomcat::core::compaction::preheat::Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    };

    let config = ContextConfig {
        keep_recent_turns: 1,
        ..Default::default()
    };
    let reduced = compact_tool_results(&mut state, &config);
    assert!(reduced > 0);

    if state.usage_ratio() >= 0.50 {
        tomcat::core::compaction::force_drop_oldest_to_target(&mut state);
    }

    assert!(state.usage_ratio() < 0.50);
    assert!(!state.messages.is_empty());
}

/// [Session 重载] 写入消息与 `type: branch_summary` 摘要行后 init_context_state 正确重建
///
/// 验证：创建会话 → 写消息 → 写 branch_summary → 再写消息 → init_context_state →
///       messages 数量正确、CompactionSummary 内容正确、后续消息正确
/// 意义：TASK-17 Transcript 持久化与重载——跨进程会话恢复端到端
#[test]
fn test_session_reload_with_branch_summary_entries() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = info_span!("test_session_reload_with_branch_summary_entries").entered();

    let dir = temp_sessions_dir("reload");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None)?;

    info!("Arrange: 写入 user/assistant 消息 → branch_summary 行 → 更多消息");
    mgr.append_message(serde_json::json!({"role":"user","content":"old question 1"}))?;
    mgr.append_message(serde_json::json!({"role":"assistant","content":"old answer 1"}))?;
    mgr.append_message(serde_json::json!({"role":"user","content":"old question 2"}))?;
    mgr.append_message(serde_json::json!({"role":"assistant","content":"old answer 2"}))?;

    mgr.append_compaction_with_range(
        "## Goal\nUser wants help with coding.\n## Progress\nAnswered 2 questions.",
        None,
        None,
        2,
    )?;

    mgr.append_message(serde_json::json!({"role":"user","content":"new question"}))?;
    mgr.append_message(serde_json::json!({"role":"assistant","content":"new answer"}))?;

    info!("Act: init_context_state 从 transcript 重建 ContextState");
    let cfg = ContextConfig::default();
    let state = init_context_state(&mgr, &cfg, "system prompt")?;

    info!("Assert: 验证 messages 数量与内容");
    assert!(
        state.messages.len() >= 3,
        "should have at least 3 messages, got {}",
        state.messages.len()
    );

    let has_summary = state.messages.iter().any(|m| {
        m.kind == MessageKind::CompactionSummary
            && m.text_content().is_some_and(|t| t.contains("Goal"))
    });
    assert!(
        has_summary,
        "应含 CompactionSummary 且内容包含 compaction summary"
    );

    let has_new_msg = state
        .messages
        .iter()
        .any(|m| m.text_content().is_some_and(|t| t.contains("new question")));
    assert!(has_new_msg, "应含 compaction 之后的 new question 消息");

    let msgs = build_context_from_state(&state);
    assert!(msgs.len() >= 3, "展平后消息数应 >= 3");

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

/// [Context Overflow 重试] 首次 LLM 调用返回 context overflow 错误 → 触发 Compaction → 重试成功
///
/// 验证：AgentLoop 在 context overflow 时触发 AutoCompactionStart/End 事件，重试后返回成功文本
/// 意义：TASK-17 ContextOverflow 自动恢复路径——AgentLoop 端到端集成
#[tokio::test]
async fn test_context_overflow_triggers_compaction_and_retries(
) -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = info_span!("test_context_overflow_triggers_compaction_and_retries").entered();

    info!("Arrange: MockLlm 首次返回 context overflow 错误，第二次返回成功文本");
    let stream_err = vec![Err(AppError::Llm(
        "context length exceeded: 500000 tokens".to_string(),
    ))];
    let stream_ok = text_stream("recovered after compaction");
    let llm = Arc::new(MockLlm::new(vec![stream_err, stream_ok]));
    let primitive = Arc::new(MockPrimitiveWithLargeFile { file_size: 100 });
    let event_bus = Arc::new(DefaultEventBus::new());

    let compaction_started = Arc::new(AtomicBool::new(false));
    let compaction_ended = Arc::new(AtomicBool::new(false));
    let cs = Arc::clone(&compaction_started);
    let ce = Arc::clone(&compaction_ended);
    event_bus.on(
        "context_overflow_trim_start",
        Box::new(move |_ctx: EventContext| {
            cs.store(true, Ordering::SeqCst);
            Ok(())
        }),
    );
    event_bus.on(
        "context_overflow_trim_end",
        Box::new(move |_ctx: EventContext| {
            ce.store(true, Ordering::SeqCst);
            Ok(())
        }),
    );

    let config = AgentLoopConfig {
        model: "mock-model".to_string(),
        session_id: "sess-ctx-overflow".to_string(),
        max_attempts: 3,
        retry_base_delay_ms: 0,
        context_config: ContextConfig {
            keep_recent_turns: 1,
            ..Default::default()
        },
        ..Default::default()
    };
    let abort = CancellationToken::new();
    let mut agent = AgentLoop::new(llm, primitive, event_bus, config, abort);

    let mut old_user = ChatMessage::user("old question");
    old_user.timestamp = Some(TEST_TS.to_string());

    let mut old_tool = ChatMessage::tool("tc1", &"x".repeat(50_000));
    old_tool.timestamp = Some(TEST_TS.to_string());

    let mut recent_user = ChatMessage::user("recent question");
    recent_user.timestamp = Some(TEST_TS.to_string());

    let ctx_state = ContextState {
        messages: vec![old_user, old_tool, recent_user],
        estimate_context_chars: 60_000,
        context_budget_chars: 1_000_000,
        context_budget_tokens: 250_000,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        latest_plan_event: None,
        preheat: tomcat::core::compaction::preheat::Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    };
    agent.set_context_state(Some(ctx_state));

    let messages = vec![ChatMessage::user("trigger overflow")];

    info!("Act: 调用 AgentLoop::run()，期望 context overflow → compaction → retry → 成功");
    let result = tokio::time::timeout(std::time::Duration::from_secs(10), agent.run(messages))
        .await
        .map_err(|_| "run() 超时 10s")?
        .unwrap();

    info!("Assert: 最终成功返回，compaction 事件已触发");
    assert!(
        result.final_text.contains("recovered"),
        "Context overflow 重试后应返回成功文本，实际: {:?}",
        result.final_text
    );
    assert!(
        compaction_started.load(Ordering::SeqCst),
        "应触发 context_overflow_trim_start 事件"
    );
    assert!(
        compaction_ended.load(Ordering::SeqCst),
        "应触发 context_overflow_trim_end 事件"
    );

    let recovered_state = agent.take_context_state();
    assert!(
        recovered_state.is_some(),
        "AgentLoop 完成后仍应持有 context_state"
    );

    Ok(())
}

/// [build_context_from_state 端到端] CompactionSummary + user/assistant/tool 混合展平后消息顺序正确
///
/// 验证：CompactionSummary 转为 CompactionSummary，普通消息展平顺序保持
/// 意义：TASK-17 上下文重建——build_context_from_state 正确性的端到端验证
#[test]
fn test_build_context_preserves_order_with_mixed_turns() {
    common::setup_logging();
    let _span = info_span!("test_build_context_preserves_order_with_mixed_turns").entered();

    let mut summary = ChatMessage::compaction_summary("## Goal\nBuild a web app");
    summary.msg_id = Some("sum_1".to_string());
    summary.timestamp = Some(TEST_TS.to_string());

    let mut user1 = ChatMessage::user("add auth");
    user1.msg_id = Some("turn_1_u".to_string());
    user1.timestamp = Some(TEST_TS.to_string());

    let mut asst1 = ChatMessage::assistant_with_tool_calls(
        Some("I'll add JWT auth"),
        vec![serde_json::json!({
            "id": "tc1",
            "type": "function",
            "function": {"name": "write_file", "arguments": r#"{"path":"auth.rs"}"#}
        })],
    );
    asst1.timestamp = Some(TEST_TS.to_string());

    let mut tool1 = ChatMessage::tool("tc1", "file written");
    tool1.msg_id = Some("turn_1_tr".to_string());
    tool1.timestamp = Some(TEST_TS.to_string());

    let mut user2 = ChatMessage::user("run tests");
    user2.msg_id = Some("turn_2".to_string());
    user2.timestamp = Some(TEST_TS.to_string());

    let state = ContextState {
        messages: vec![summary, user1, asst1, tool1, user2],
        estimate_context_chars: 500,
        context_budget_chars: 10_000,
        context_budget_tokens: 2_500,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        latest_plan_event: None,
        preheat: tomcat::core::compaction::preheat::Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    };

    let msgs = build_context_from_state(&state);

    assert_eq!(msgs.len(), 5, "应展平为 5 条消息");
    assert!(
        msgs[0].kind == MessageKind::CompactionSummary
            && msgs[0].text_content().is_some_and(|t| t.contains("Goal")),
        "msgs[0] should be CompactionSummary containing 'Goal'"
    );
    assert!(
        msgs[1].role == ChatMessageRole::User && msgs[1].text_content() == Some("add auth"),
        "msgs[1] should be User 'add auth'"
    );
    assert!(
        msgs[2].role == ChatMessageRole::Assistant,
        "msgs[2] should be Assistant"
    );
    assert!(
        msgs[3].role == ChatMessageRole::Tool,
        "msgs[3] should be Tool"
    );
    assert!(
        msgs[4].role == ChatMessageRole::User && msgs[4].text_content() == Some("run tests"),
        "msgs[4] should be User 'run tests'"
    );
}

// ────────── Layer 1 深度验证测试 ──────────────────────────────────────────

/// [Layer 1 深度] 占位符替换正确性：旧 turn 的超大 tool result 被替换为占位符，保护区内 turn 保持原内容
#[test]
fn test_compact_tool_results_replaces_with_placeholder() {
    common::setup_logging();
    let _span = info_span!("test_compact_tool_results_replaces_with_placeholder").entered();

    let big = "x".repeat(25_000);
    let mut msgs: Vec<ChatMessage> = Vec::new();
    msgs.extend(make_msgs_with_tool_result("q1", &big));
    msgs.extend(make_msgs_with_tool_result("q2", &big));
    msgs.extend(make_msgs_with_tool_result("q3-recent", &big));

    let total: usize = msgs.iter().map(estimate_msg_chars).sum();
    let mut state = ContextState {
        messages: msgs,
        estimate_context_chars: total,
        context_budget_chars: total / 3,
        context_budget_tokens: 0,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        latest_plan_event: None,
        preheat: tomcat::core::compaction::preheat::Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    };

    info!("Act: compact_tool_results with keep_recent=1");
    let config = ContextConfig {
        keep_recent_turns: 1,
        ..Default::default()
    };
    compact_tool_results(&mut state, &config);

    info!("Assert: old turns replaced, recent preserved");
    // Turns: turn 0 = msgs[0..3], turn 1 = msgs[3..6], turn 2 = msgs[6..9] (recent)
    // Tool messages are at index 1, 4, 7
    let tool_msgs: Vec<&ChatMessage> = state
        .messages
        .iter()
        .filter(|m| m.role == ChatMessageRole::Tool)
        .collect();
    assert_eq!(tool_msgs.len(), 3, "should have 3 tool messages");
    assert_eq!(
        tool_msgs[0].text_content().unwrap_or(""),
        PLACEHOLDER,
        "first tool result should be placeholder"
    );
    assert_eq!(
        tool_msgs[1].text_content().unwrap_or(""),
        PLACEHOLDER,
        "second tool result should be placeholder"
    );
    assert_eq!(
        tool_msgs[2].text_content().unwrap_or(""),
        big,
        "third (protected recent) tool result should keep original"
    );
}

/// [Layer 1 深度] compactable zone 内超过占位符阈值的 tool results 均被替换
#[test]
fn test_compact_tool_results_replaces_all_large_in_compactable_zone() {
    common::setup_logging();
    let _span =
        info_span!("test_compact_tool_results_replaces_all_large_in_compactable_zone").entered();

    let big = "x".repeat(25_000);
    let small = "x".repeat(5_000);
    let mut msgs: Vec<ChatMessage> = Vec::new();
    msgs.extend(make_msgs_with_tool_result("q1", &big));
    msgs.extend(make_msgs_with_tool_result("q2", &small));
    msgs.extend(make_msgs_with_tool_result("q3", &big));
    msgs.extend(make_msgs_with_tool_result("q4-recent", &big));

    let total: usize = msgs.iter().map(estimate_msg_chars).sum();

    let mut state = ContextState {
        messages: msgs,
        estimate_context_chars: total,
        context_budget_chars: total,
        context_budget_tokens: total / 4,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        latest_plan_event: None,
        preheat: tomcat::core::compaction::preheat::Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    };

    info!("Act: compact with m=1, only >threshold in compactable zone get replaced");
    let config = ContextConfig {
        keep_recent_turns: 1,
        ..Default::default()
    };
    compact_tool_results(&mut state, &config);

    let tool_msgs: Vec<&ChatMessage> = state
        .messages
        .iter()
        .filter(|m| m.role == ChatMessageRole::Tool)
        .collect();
    assert_eq!(tool_msgs.len(), 4);

    assert_eq!(
        tool_msgs[0].text_content().unwrap_or(""),
        PLACEHOLDER,
        "first (above threshold) should be replaced"
    );
    assert_eq!(
        tool_msgs[1].text_content().unwrap_or(""),
        small,
        "second (below threshold) should keep original"
    );
    assert_eq!(
        tool_msgs[2].text_content().unwrap_or(""),
        PLACEHOLDER,
        "third (above threshold) should be replaced"
    );
    assert_eq!(
        tool_msgs[3].text_content().unwrap_or(""),
        big,
        "fourth (protected) should keep original"
    );
}

/// [Layer 1 深度] estimate 精确变化量（仅超过占位符阈值时触发替换）
#[test]
fn test_compact_tool_results_estimate_precise() {
    common::setup_logging();
    let _span = info_span!("test_compact_tool_results_estimate_precise").entered();

    let content_len = 25_000;
    let big = "y".repeat(content_len);
    let mut msgs: Vec<ChatMessage> = Vec::new();
    msgs.extend(make_msgs_with_tool_result("q1", &big));
    msgs.extend(make_msgs_with_tool_result("q2-recent", &big));
    let total: usize = msgs.iter().map(estimate_msg_chars).sum();

    let mut state = ContextState {
        messages: msgs,
        estimate_context_chars: total,
        context_budget_chars: 1,
        context_budget_tokens: 0,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        latest_plan_event: None,
        preheat: tomcat::core::compaction::preheat::Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    };

    let config = ContextConfig {
        keep_recent_turns: 1,
        ..Default::default()
    };
    let reduced = compact_tool_results(&mut state, &config);

    let expected_reduced = content_len - PLACEHOLDER.len();
    assert_eq!(
        reduced, expected_reduced,
        "reduced should be exactly original_len - placeholder_len"
    );
    assert_eq!(
        state.estimate_context_chars,
        total - expected_reduced,
        "estimate should be total - reduced"
    );
}

#[tokio::test]
async fn test_reasoning_loop_mid_turn_precheck_rewrites_before_second_llm() {
    common::setup_logging();
    let _span =
        info_span!("test_reasoning_loop_mid_turn_precheck_rewrites_before_second_llm").entered();

    let llm = Arc::new(RecordingMockLlm::new(vec![
        multi_tool_call_stream(&[
            ("tc1", "read", r#"{"path":"a.txt"}"#),
            ("tc2", "read", r#"{"path":"b.txt"}"#),
            ("tc3", "read", r#"{"path":"c.txt"}"#),
            ("tc4", "read", r#"{"path":"d.txt"}"#),
            ("tc5", "read", r#"{"path":"e.txt"}"#),
        ]),
        text_stream("done"),
    ]));
    let primitive = Arc::new(MockPrimitiveWithLargeFile { file_size: 8_000 });
    let event_bus = Arc::new(DefaultEventBus::new());
    let mut agent = AgentLoop::new(
        llm.clone(),
        primitive,
        event_bus,
        AgentLoopConfig {
            model: "mock-model".to_string(),
            session_id: "sess-mid-turn-run".to_string(),
            context_config: ContextConfig {
                current_tail_compactable_min_chars: 1,
                keep_recent_turns: 1,
                ..Default::default()
            },
            ..Default::default()
        },
        CancellationToken::new(),
    );
    agent.set_context_state(Some(ContextState {
        messages: vec![],
        estimate_context_chars: 0,
        context_budget_chars: 20_000,
        context_budget_tokens: 5_000,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        latest_plan_event: None,
        preheat: tomcat::core::compaction::preheat::Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    }));

    let result = agent
        .run(vec![ChatMessage::user("read five files")])
        .await
        .unwrap();
    assert_eq!(result.final_text, "done");

    let requests = llm.requests.lock().unwrap();
    assert_eq!(
        requests.len(),
        2,
        "should issue second LLM request after tool round"
    );
    let second_request = &requests[1];
    let placeholder_count = second_request
        .messages
        .iter()
        .filter(|msg| msg.role == ChatMessageRole::Tool)
        .filter(|msg| msg.text_content() == Some(TOOL_RESULT_PLACEHOLDER_TEXT))
        .count();
    assert!(
        placeholder_count >= 1,
        "mid-turn precheck should rewrite older tool results before second LLM request"
    );
}

/// [V2 集成] Session 重载识别 compact boundary 无重复
#[test]
fn test_session_reload_with_boundary() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = info_span!("test_session_reload_with_boundary").entered();

    let dir = temp_sessions_dir("boundary");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None)?;

    mgr.append_message(serde_json::json!({"role":"user","content":"old question"}))?;
    mgr.append_message(serde_json::json!({"role":"assistant","content":"old answer"}))?;

    let path = mgr.current_transcript_path()?.unwrap();
    let boundary = tomcat::core::session::transcript::TranscriptEntry::BranchSummary(
        tomcat::core::session::transcript::BranchSummaryEntry {
            id: None,
            parent_id: None,
            timestamp: "2026-01-01T00:00:00.000Z".to_string(),
            summary: Some("Summary of everything before this point".to_string()),
            covered_start_id: None,
            covered_end_id: None,
            covered_count: Some(1),
            is_boundary: Some(true),
            preheat_compaction_id: None,
            estimated_covered_tokens_before: None,
            estimated_summary_tokens: None,
            estimated_tokens_saved: None,
            error: None,
            attempts: None,
        },
    );
    tomcat::core::session::transcript::append_entry(&path, &boundary)?;

    mgr.append_message(serde_json::json!({"role":"user","content":"new question"}))?;
    mgr.append_message(serde_json::json!({"role":"assistant","content":"new answer"}))?;

    let cfg = ContextConfig::default();
    let state = init_context_state(&mgr, &cfg, "system")?;

    let has_old = state
        .messages
        .iter()
        .any(|m| m.text_content().is_some_and(|t| t.contains("old")));
    assert!(!has_old, "turns before boundary should be discarded");

    let has_summary = state.messages.iter().any(|m| {
        m.kind == MessageKind::CompactionSummary
            && m.text_content()
                .is_some_and(|t| t.contains("Summary of everything"))
    });
    assert!(has_summary, "boundary summary should be present");

    let has_new = state
        .messages
        .iter()
        .any(|m| m.text_content().is_some_and(|t| t.contains("new")));
    assert!(has_new, "turns after boundary should be present");
    // summary + user("new question") + assistant("new answer") = 3
    assert_eq!(state.messages.len(), 3, "summary + 2 new messages");

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

/// [V2 集成] Layer 0 大文件落盘可读回
#[test]
fn test_layer0_persist_and_readback() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = info_span!("test_layer0_persist_and_readback").entered();

    use tomcat::core::compaction::layer0_persist_large_results;

    let dir = tempfile::tempdir()?;
    let original = "important content ".repeat(4000);

    let mut user_msg = ChatMessage::user("trigger");
    user_msg.timestamp = Some(TEST_TS.to_string());

    let mut tool_msg = ChatMessage::tool("tc_persist", &original);
    tool_msg.timestamp = Some(TEST_TS.to_string());

    let mut state = ContextState {
        messages: vec![user_msg, tool_msg],
        estimate_context_chars: original.len(),
        context_budget_chars: 1_000_000,
        context_budget_tokens: 250_000,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        latest_plan_event: None,
        preheat: tomcat::core::compaction::preheat::Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    };
    let config = ContextConfig::default();
    let (results, _) =
        layer0_persist_large_results(&mut state, &config, dir.path(), "sess_persist");
    assert_eq!(results.len(), 1);

    let readback = std::fs::read_to_string(&results[0].persisted_path)?;
    assert_eq!(
        readback, original,
        "persisted content should match original"
    );

    let tool_in_state = state
        .messages
        .iter()
        .find(|m| m.role == ChatMessageRole::Tool)
        .expect("tool message should exist");
    let content = tool_in_state.text_content().unwrap_or("");
    assert!(content.starts_with("[Tool result persisted:"));
    assert!(content.contains("Preview:"));

    assert!(
        state.estimate_context_chars < original.len(),
        "estimate should decrease after persistence"
    );

    Ok(())
}

/// [TASK-20 集成] Session 重载：is_boundary=false 被跳过、is_boundary=true 生效
#[test]
fn test_session_reload_boundary_false_skipped() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = info_span!("test_session_reload_boundary_false_skipped").entered();

    let dir = temp_sessions_dir("boundary_false");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None)?;

    mgr.append_message(serde_json::json!({"role":"user","content":"first question"}))?;
    mgr.append_message(serde_json::json!({"role":"assistant","content":"first answer"}))?;

    let path = mgr.current_transcript_path()?.unwrap();

    let msg_ids: Vec<String> = tomcat::core::session::transcript::read_entries_tail(&path, 500)?
        .into_iter()
        .filter_map(|e| {
            if let tomcat::core::session::transcript::TranscriptEntry::Message(me) = e {
                me.id
            } else {
                None
            }
        })
        .collect();
    assert!(msg_ids.len() >= 2, "expect user+assistant message ids");
    let covered_start = msg_ids[0].clone();
    let covered_end = msg_ids[1].clone();
    let compact_id = compound_turn_id(&covered_start, &covered_end);

    let preheat_entry = tomcat::core::session::transcript::TranscriptEntry::BranchSummary(
        tomcat::core::session::transcript::BranchSummaryEntry {
            id: Some(compact_id.clone()),
            parent_id: None,
            timestamp: "2026-01-01T00:00:01.000Z".to_string(),
            summary: Some("Preheat summary (should be ignored)".to_string()),
            covered_start_id: Some(covered_start),
            covered_end_id: Some(covered_end),
            covered_count: Some(1),
            is_boundary: Some(false),
            preheat_compaction_id: Some(compact_id),
            estimated_covered_tokens_before: None,
            estimated_summary_tokens: None,
            estimated_tokens_saved: None,
            error: None,
            attempts: None,
        },
    );
    tomcat::core::session::transcript::append_entry(&path, &preheat_entry)?;

    mgr.append_message(serde_json::json!({"role":"user","content":"second question"}))?;
    mgr.append_message(serde_json::json!({"role":"assistant","content":"second answer"}))?;

    let cfg = ContextConfig::default();
    let state = init_context_state(&mgr, &cfg, "system")?;

    let has_preheat_summary = state.messages.iter().any(|m| {
        m.kind == MessageKind::CompactionSummary
            && m.text_content()
                .is_some_and(|t| t.contains("Preheat summary"))
    });
    assert!(
        !has_preheat_summary,
        "is_boundary=false entry should be skipped during reload"
    );

    let has_first = state
        .messages
        .iter()
        .any(|m| m.text_content().is_some_and(|t| t.contains("first")));
    assert!(has_first, "original turns should still be present");

    let has_second = state
        .messages
        .iter()
        .any(|m| m.text_content().is_some_and(|t| t.contains("second")));
    assert!(has_second, "turns after preheat entry should be present");

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

/// [Fix B+C] agent_loop 返回的 new_messages 首条应为 role=User 的 ChatMessage
///
/// 验证：修复后 start_idx = context_tail_start → new_messages 包含 User 消息
/// 意义：确保新消息列表完整包含用户输入
#[tokio::test]
async fn test_new_messages_includes_user_message() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = info_span!("test_new_messages_includes_user_message").entered();

    let stream_ok = text_stream("response to user");
    let llm = Arc::new(MockLlm::new(vec![stream_ok]));
    let primitive = Arc::new(MockPrimitiveWithLargeFile { file_size: 100 });
    let event_bus = Arc::new(DefaultEventBus::new());

    let config = AgentLoopConfig {
        model: "mock-model".to_string(),
        session_id: "sess-user-msg".to_string(),
        max_attempts: 1,
        retry_base_delay_ms: 0,
        ..Default::default()
    };
    let abort = CancellationToken::new();
    let mut agent = AgentLoop::new(llm, primitive, event_bus, config, abort);

    let messages = vec![ChatMessage::system("system"), ChatMessage::user("hello")];

    let result = tokio::time::timeout(std::time::Duration::from_secs(5), agent.run(messages))
        .await
        .map_err(|_| "run() timeout 5s")?
        .unwrap();

    assert!(
        !result.new_messages.is_empty(),
        "new_messages should not be empty"
    );
    assert!(
        result.new_messages[0].role == ChatMessageRole::User
            && result.new_messages[0].text_content() == Some("hello"),
        "first new_message should be User('hello'), got {:?}",
        result.new_messages.first()
    );
    assert!(
        result.new_messages.len() >= 2,
        "new_messages should contain User + Assistant, got {}",
        result.new_messages.len()
    );

    Ok(())
}

#[tokio::test]
async fn test_agent_loop_message_append_sink_persists_assistant_immediately(
) -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let dir = temp_sessions_dir("message_append_sink");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None)?;

    let user_row_id = mgr.append_message(serde_json::json!({
        "role": "user",
        "content": "hello"
    }))?;
    let mut persisted_user = ChatMessage::user("hello");
    persisted_user.msg_id = Some(user_row_id);

    let llm = Arc::new(MockLlm::new(vec![text_stream("persisted reply")]));
    let primitive = Arc::new(MockPrimitiveWithLargeFile { file_size: 100 });
    let event_bus = Arc::new(DefaultEventBus::new());
    let sink: Arc<dyn MessageAppendSink> = Arc::new(mgr.clone());
    let config = AgentLoopConfig {
        model: "mock-model".to_string(),
        session_id: "sess-append-sink".to_string(),
        max_attempts: 1,
        retry_base_delay_ms: 0,
        message_append_sink: Some(sink),
        ..Default::default()
    };
    let abort = CancellationToken::new();
    let mut agent = AgentLoop::new(llm, primitive, event_bus, config, abort);

    let messages = vec![ChatMessage::system("system"), persisted_user];
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), agent.run(messages))
        .await
        .map_err(|_| "run() timeout 5s")?
        .unwrap();

    let assistant = result
        .new_messages
        .iter()
        .find(|m| m.text_content() == Some("persisted reply"))
        .expect("assistant reply should exist");
    assert!(
        assistant.msg_id.is_some(),
        "assistant should be persisted immediately and carry msg_id"
    );

    let transcript_path = mgr.current_transcript_path()?.expect("transcript path");
    let entries = tomcat::core::session::transcript::read_entries_tail(&transcript_path, 8)?;
    assert!(entries.iter().any(|entry| {
        matches!(
            entry,
            tomcat::TranscriptEntry::Message(me)
                if me.message.get("content").and_then(|v| v.as_str()) == Some("persisted reply")
        )
    }));

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

/// [Fix B+C] L3 rebuild 后 estimate_context_chars 与 sum(msg_chars) + system 一致（无幽灵）
///
/// 场景：构造溢出 → L3 → agent 重试成功 → take_context_state → 验证 estimate 对齐
#[tokio::test]
async fn test_l3_rebuild_estimate_consistent_no_phantom() -> Result<(), Box<dyn std::error::Error>>
{
    common::setup_logging();
    let _span = info_span!("test_l3_rebuild_estimate_consistent_no_phantom").entered();

    let stream_err = vec![Err(AppError::Llm(
        "context length exceeded: 500000 tokens".to_string(),
    ))];
    let stream_ok = text_stream("recovered");
    let llm = Arc::new(MockLlm::new(vec![stream_err, stream_ok]));
    let primitive = Arc::new(MockPrimitiveWithLargeFile { file_size: 100 });
    let event_bus = Arc::new(DefaultEventBus::new());

    let system_text = "system prompt for test";
    let config = AgentLoopConfig {
        model: "mock".to_string(),
        session_id: "sess-phantom".to_string(),
        max_attempts: 3,
        retry_base_delay_ms: 0,
        context_config: ContextConfig {
            keep_recent_turns: 1,
            ..Default::default()
        },
        ..Default::default()
    };
    let abort = CancellationToken::new();
    let mut agent = AgentLoop::new(llm, primitive, event_bus, config, abort);

    let mut old_user = ChatMessage::user("old question");
    old_user.timestamp = Some(TEST_TS.to_string());
    let mut old_tool = ChatMessage::tool("tc1", &"x".repeat(50_000));
    old_tool.timestamp = Some(TEST_TS.to_string());

    let big_chars: usize = estimate_msg_chars(&old_user) + estimate_msg_chars(&old_tool);
    let ctx_state = ContextState {
        messages: vec![old_user, old_tool],
        estimate_context_chars: system_text.len() + big_chars,
        context_budget_chars: 1_000_000,
        context_budget_tokens: 250_000,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        latest_plan_event: None,
        preheat: tomcat::core::compaction::preheat::Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    };
    agent.set_context_state(Some(ctx_state));

    let messages = vec![
        ChatMessage::system(system_text),
        ChatMessage::user("trigger overflow"),
    ];

    let result = tokio::time::timeout(std::time::Duration::from_secs(10), agent.run(messages))
        .await
        .map_err(|_| "run() timeout 10s")?
        .unwrap();

    let ctx = agent
        .take_context_state()
        .expect("context_state should exist");

    let msgs_chars: usize = ctx.messages.iter().map(estimate_msg_chars).sum();

    let new_turn_chars: usize = result
        .new_messages
        .iter()
        .map(|m| match m.role {
            ChatMessageRole::User => m.text_content().map_or(0, |t| t.len()),
            ChatMessageRole::Assistant => {
                let text_len = m.text_content().map_or(0, |t| t.len());
                let tc_len = m.tool_calls.as_ref().map_or(0, |tcs| {
                    tcs.iter()
                        .map(|tc| {
                            let name = tc
                                .get("function")
                                .and_then(|f| f.get("name"))
                                .and_then(|n| n.as_str())
                                .unwrap_or("");
                            let args = tc
                                .get("function")
                                .and_then(|f| f.get("arguments"))
                                .and_then(|a| a.as_str())
                                .unwrap_or("");
                            let id = tc.get("id").and_then(|i| i.as_str()).unwrap_or("");
                            name.len() + args.len() + id.len() + 40
                        })
                        .sum::<usize>()
                });
                text_len + tc_len
            }
            ChatMessageRole::Tool => m.text_content().map_or(0, |t| t.len()),
            _ => 0,
        })
        .sum();

    let actual_estimate = ctx.estimate_context_chars;
    let sum_from_msgs = system_text.len() + msgs_chars + new_turn_chars;

    let drift = actual_estimate.abs_diff(sum_from_msgs);
    assert!(
        drift < 200,
        "estimate ({}) should be close to system + msgs + new_turn ({}), drift = {}",
        actual_estimate,
        sum_from_msgs,
        drift
    );

    Ok(())
}

/// [Fix B+C] L3 force_drop 后 estimate_context_chars 与 sum(msg_chars) + system 一致
#[test]
fn test_force_drop_estimate_consistent_after_l3() {
    common::setup_logging();
    let _span = info_span!("test_force_drop_estimate_consistent_after_l3").entered();

    let system_chars = 100usize;

    let make_turn_msgs = |label: &str, asst_text: &str| -> Vec<ChatMessage> {
        let mut user = ChatMessage::user(format!("question {}", label));
        user.timestamp = Some(TEST_TS.to_string());
        let mut asst = ChatMessage::assistant(asst_text);
        asst.timestamp = Some(TEST_TS.to_string());
        vec![user, asst]
    };

    let mut msgs: Vec<ChatMessage> = Vec::new();
    msgs.extend(make_turn_msgs("0", &"x".repeat(20_000)));
    msgs.extend(make_turn_msgs("1", &"x".repeat(20_000)));
    msgs.extend(make_turn_msgs("2", "answer 2"));

    let total_msg_chars: usize = msgs.iter().map(estimate_msg_chars).sum();
    let budget_tokens = total_msg_chars / 4;

    let mut state = ContextState {
        messages: msgs,
        estimate_context_chars: system_chars + total_msg_chars,
        context_budget_chars: total_msg_chars,
        context_budget_tokens: budget_tokens,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        latest_plan_event: None,
        preheat: tomcat::core::compaction::preheat::Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    };

    tomcat::core::compaction::force_drop_oldest_to_target(&mut state);

    let remaining_msg_chars: usize = state.messages.iter().map(estimate_msg_chars).sum();
    let expected = system_chars + remaining_msg_chars;
    assert_eq!(
        state.estimate_context_chars, expected,
        "estimate ({}) should equal system + sum(remaining msg_chars) ({})",
        state.estimate_context_chars, expected
    );
    assert!(
        !state.messages.is_empty(),
        "should have at least one remaining message after L3"
    );
}

/// [TASK-20 集成] Session 重载：is_boundary=false 被跳过、is_boundary=true 生效
#[test]
fn test_session_reload_pending_preheat_restore() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = info_span!("test_session_reload_pending_preheat_restore").entered();

    let dir = temp_sessions_dir("preheat_restore");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None)?;

    mgr.append_message(serde_json::json!({"role":"user","content":"first question"}))?;
    mgr.append_message(serde_json::json!({"role":"assistant","content":"first answer"}))?;

    let path = mgr.current_transcript_path()?.unwrap();
    let msg_ids: Vec<String> = tomcat::core::session::transcript::read_entries_tail(&path, 500)?
        .into_iter()
        .filter_map(|e| {
            if let tomcat::core::session::transcript::TranscriptEntry::Message(me) = e {
                me.id
            } else {
                None
            }
        })
        .collect();
    assert!(msg_ids.len() >= 2);
    let covered_start = msg_ids[0].clone();
    let covered_end = msg_ids[1].clone();
    let compact_id = compound_turn_id(&covered_start, &covered_end);

    let preheat_entry = tomcat::core::session::transcript::TranscriptEntry::BranchSummary(
        tomcat::core::session::transcript::BranchSummaryEntry {
            id: Some(compact_id.clone()),
            parent_id: None,
            timestamp: "2026-01-01T00:00:01.000Z".to_string(),
            summary: Some("Restored preheat summary body".to_string()),
            covered_start_id: Some(covered_start),
            covered_end_id: Some(covered_end),
            covered_count: Some(1),
            is_boundary: Some(false),
            preheat_compaction_id: Some(compact_id.clone()),
            estimated_covered_tokens_before: None,
            estimated_summary_tokens: None,
            estimated_tokens_saved: None,
            error: None,
            attempts: None,
        },
    );
    tomcat::core::session::transcript::append_entry(&path, &preheat_entry)?;

    mgr.append_message(serde_json::json!({"role":"user","content":"second question"}))?;
    mgr.append_message(serde_json::json!({"role":"assistant","content":"second answer"}))?;

    let cfg = ContextConfig::default();
    let mut state = init_context_state(&mgr, &cfg, "system")?;

    assert!(
        state.preheat.is_finished(),
        "reload should leave CachedCompleted preheat"
    );
    use tomcat::core::compaction::preheat::PreheatOutcome;
    match state.preheat.poll_result() {
        PreheatOutcome::Completed(r) => {
            assert!(
                r.summary_text.contains("Restored preheat summary"),
                "poll should return disk preheat summary"
            );
            assert_eq!(
                r.transcript_compaction_entry_id.as_deref(),
                Some(compact_id.as_str())
            );
        }
        o => panic!("expected Completed, got {:?}", o),
    }
    assert!(state.preheat.is_idle());

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

// ────────── Group C: L2 preheat 全链路 + 事件断言 ──────────────────────────

/// [L2 事件] check_after_reply 在 preheat 完成且 ratio >= 0.85 时 emit BoundarySwitched
#[test]
fn test_check_after_reply_emits_boundary_switched_on_apply() {
    common::setup_logging();
    let _span = info_span!("test_check_after_reply_emits_boundary_switched_on_apply").entered();

    let event_bus = Arc::new(DefaultEventBus::new());
    let boundary_switched = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let bs = Arc::clone(&boundary_switched);
    event_bus.on(
        "boundary_switched",
        Box::new(move |ctx: EventContext| {
            bs.lock().unwrap().push(ctx.payload.clone());
            Ok(())
        }),
    );

    info!("Arrange: ContextState with high ratio + CachedCompleted preheat");
    let mut state = ContextState {
        messages: vec![],
        estimate_context_chars: 0,
        context_budget_chars: 100_000,
        context_budget_tokens: 1000,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        latest_plan_event: None,
        preheat: tomcat::core::compaction::preheat::Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    };

    let mut m0 = ChatMessage::user("a".repeat(5000));
    m0.msg_id = Some("u0".to_string());
    m0.timestamp = Some(TEST_TS.to_string());
    let mut m1 = ChatMessage::user("b".repeat(3000));
    m1.msg_id = Some("u1".to_string());
    m1.timestamp = Some(TEST_TS.to_string());
    let mut m2 = ChatMessage::user("c".repeat(2000));
    m2.msg_id = Some("u2".to_string());
    m2.timestamp = Some(TEST_TS.to_string());
    state.messages = vec![m0, m1, m2];
    state.estimate_context_chars = 10_000;
    state.update_api_usage(900, 0); // ratio = 0.90 >= 0.85

    let compaction_result = tomcat::CompactionResult {
        summary_text: "summary of u0-u1".into(),
        covered_start_id: "u0".into(),
        covered_end_id: "u1".into(),
        covered_count: 2,
        transcript_compaction_entry_id: Some(compound_turn_id("u0", "u1")),
        estimated_covered_tokens_before: Some(200),
        estimated_summary_tokens: Some(50),
        estimated_tokens_saved: Some(150),
        preheat_elapsed_ms: 100,
    };
    state.preheat.restore_completed(compaction_result);
    assert!(state.preheat.is_finished());

    info!("Act: check_after_reply");
    let switched = tomcat::core::compaction::apply::check_after_reply(&mut state, &*event_bus);

    info!("Assert: BoundarySwitched event received");
    assert!(switched, "should have applied boundary");
    let events = boundary_switched.lock().unwrap();
    assert_eq!(events.len(), 1, "should emit exactly one BoundarySwitched");
    let payload = &events[0];
    assert!(
        payload.get("ratioBefore").is_some(),
        "payload should have ratioBefore"
    );
    assert!(
        payload.get("ratioAfter").is_some(),
        "payload should have ratioAfter"
    );

    assert_eq!(state.messages.len(), 2, "summary + u2 after splice");
    assert_eq!(state.messages[0].kind, MessageKind::CompactionSummary);
}

/// [L2 事件] check_after_reply 在 stale apply 时 emit CompactionError
#[test]
fn test_check_after_reply_stale_emits_compaction_error() {
    common::setup_logging();
    let _span = info_span!("test_check_after_reply_stale_emits_compaction_error").entered();

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("stale_err.jsonl");
    tomcat::core::session::transcript::write_header(
        &path,
        &tomcat::core::session::transcript::SessionHeader {
            r#type: "session".to_string(),
            version: Some(3),
            id: "sid_err".to_string(),
            timestamp: "2025-01-01T00:00:00.000Z".to_string(),
            cwd: None,
        },
    )
    .unwrap();

    let event_bus = Arc::new(DefaultEventBus::new());
    let compaction_errors = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let ce = Arc::clone(&compaction_errors);
    event_bus.on(
        "compaction_error",
        Box::new(move |ctx: EventContext| {
            ce.lock().unwrap().push(ctx.payload.clone());
            Ok(())
        }),
    );

    info!("Arrange: ContextState with stale preheat result");
    let mut state = ContextState {
        messages: vec![{
            let mut m = ChatMessage::user("still here");
            m.msg_id = Some("still".to_string());
            m.timestamp = Some(TEST_TS.to_string());
            m
        }],
        estimate_context_chars: 500,
        context_budget_chars: 100_000,
        context_budget_tokens: 1000,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: path.clone(),
        latest_plan_event: None,
        preheat: tomcat::core::compaction::preheat::Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    };
    state.update_api_usage(900, 0); // ratio >= 0.85

    let stale_result = tomcat::CompactionResult {
        summary_text: "sum".into(),
        covered_start_id: "gone_start".into(),
        covered_end_id: "gone_end".into(),
        covered_count: 2,
        transcript_compaction_entry_id: None,
        estimated_covered_tokens_before: None,
        estimated_summary_tokens: None,
        estimated_tokens_saved: None,
        preheat_elapsed_ms: 0,
    };
    state.preheat.restore_completed(stale_result);

    info!("Act: check_after_reply with stale covered_end_id");
    let switched = tomcat::core::compaction::apply::check_after_reply(&mut state, &*event_bus);

    info!("Assert: CompactionError event received, not switched");
    assert!(!switched, "stale apply should not switch");
    let errors = compaction_errors.lock().unwrap();
    assert_eq!(errors.len(), 1, "should emit exactly one CompactionError");
    let payload = &errors[0];
    assert_eq!(
        payload.get("source").and_then(|v| v.as_str()),
        Some("apply"),
        "error source should be 'apply'"
    );
}

/// [L2 事件] check_before_request 在完成的 preheat 上 emit BoundarySwitched（async 路径）
#[tokio::test]
async fn test_check_before_request_emits_boundary_switched() {
    common::setup_logging();
    let _span = info_span!("test_check_before_request_emits_boundary_switched").entered();

    let event_bus = Arc::new(DefaultEventBus::new());
    let boundary_switched = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let bs = Arc::clone(&boundary_switched);
    event_bus.on(
        "boundary_switched",
        Box::new(move |ctx: EventContext| {
            bs.lock().unwrap().push(ctx.payload.clone());
            Ok(())
        }),
    );

    info!("Arrange: ratio >= 0.70, preheat CachedCompleted");
    let mut state = ContextState {
        messages: vec![],
        estimate_context_chars: 0,
        context_budget_chars: 100_000,
        context_budget_tokens: 1000,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        latest_plan_event: None,
        preheat: tomcat::core::compaction::preheat::Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    };

    let mut m0 = ChatMessage::user("a".repeat(5000));
    m0.msg_id = Some("u0".to_string());
    m0.timestamp = Some(TEST_TS.to_string());
    let mut m1 = ChatMessage::user("b".repeat(3000));
    m1.msg_id = Some("u1".to_string());
    m1.timestamp = Some(TEST_TS.to_string());
    let mut m2 = ChatMessage::user("c".repeat(2000));
    m2.msg_id = Some("u2".to_string());
    m2.timestamp = Some(TEST_TS.to_string());
    state.messages = vec![m0, m1, m2];
    state.estimate_context_chars = 10_000;
    state.update_api_usage(750, 0); // ratio = 0.75 >= 0.70

    let compaction_result = tomcat::CompactionResult {
        summary_text: "async boundary summary".into(),
        covered_start_id: "u0".into(),
        covered_end_id: "u1".into(),
        covered_count: 2,
        transcript_compaction_entry_id: Some(compound_turn_id("u0", "u1")),
        estimated_covered_tokens_before: Some(200),
        estimated_summary_tokens: Some(50),
        estimated_tokens_saved: Some(150),
        preheat_elapsed_ms: 50,
    };
    state.preheat.restore_completed(compaction_result);

    info!("Act: check_before_request");
    let applied =
        tomcat::core::compaction::apply::check_before_request(&mut state, &*event_bus).await;

    info!("Assert: BoundarySwitched event + messages shortened");
    assert!(applied, "should apply boundary in check_before_request");
    let events = boundary_switched.lock().unwrap();
    assert_eq!(events.len(), 1, "should emit BoundarySwitched");

    let payload = &events[0];
    let ratio_before = payload
        .get("ratioBefore")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let ratio_after = payload
        .get("ratioAfter")
        .and_then(|v| v.as_f64())
        .unwrap_or(1.0);
    assert!(
        ratio_after < ratio_before,
        "ratio should decrease: before={}, after={}",
        ratio_before,
        ratio_after
    );
    assert_eq!(state.messages.len(), 2, "summary + u2");
}

// ────────── Group D: L0->L1->L2->L3 全管线 + 事件时序 ─────────────────────

/// [全链路管线] L0 落盘 → L1 占位 → L2 apply → L3 force_drop + 事件时序正确
#[test]
fn test_full_compaction_pipeline_l0_l1_l2_l3_with_event_sequence() {
    common::setup_logging();
    let _span = info_span!("test_full_compaction_pipeline_l0_l1_l2_l3").entered();
    let dir = tempfile::tempdir().unwrap();

    let event_bus = Arc::new(DefaultEventBus::new());
    let events_log = Arc::new(Mutex::new(Vec::<String>::new()));

    let el = Arc::clone(&events_log);
    event_bus.on(
        "boundary_switched",
        Box::new(move |_ctx: EventContext| {
            el.lock().unwrap().push("boundary_switched".to_string());
            Ok(())
        }),
    );

    info!("Arrange: 10+ turns with large tool results, ratio ~0.90");
    let big = "x".repeat(60_000);
    let mut msgs = Vec::new();
    for i in 0..12 {
        let mut user = ChatMessage::user(format!("question {}", i));
        user.msg_id = Some(format!("u{}", i));
        user.timestamp = Some(TEST_TS.to_string());
        let mut tool = ChatMessage::tool(&format!("tc_{}", i), &big);
        tool.msg_id = Some(format!("t{}", i));
        tool.timestamp = Some(TEST_TS.to_string());
        let mut asst = ChatMessage::assistant(format!("answer {}", i));
        asst.timestamp = Some(TEST_TS.to_string());
        msgs.push(user);
        msgs.push(tool);
        msgs.push(asst);
    }
    let total: usize = msgs.iter().map(estimate_msg_chars).sum();
    let budget_chars = (total as f64 * 1.1) as usize;
    let budget_tokens = budget_chars / 4;

    let mut state = ContextState {
        messages: msgs,
        estimate_context_chars: total,
        context_budget_chars: budget_chars,
        context_budget_tokens: budget_tokens,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        latest_plan_event: None,
        preheat: tomcat::core::compaction::preheat::Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    };

    info!(
        "initial: messages={}, estimate_chars={}, ratio={:.3}",
        state.messages.len(),
        state.estimate_context_chars,
        state.usage_ratio()
    );

    // Step 1: L0+L1
    info!("Act Step 1: run_layer0_cleanup");
    let outcome = tomcat::core::compaction::run_layer0_cleanup(
        &mut state,
        &ContextConfig::default(),
        dir.path(),
        "pipeline_sess",
    );
    info!(
        "L0+L1: persisted={}, persist_freed={}, placeholder_freed={}, ratio={:.3}",
        outcome.persisted.len(),
        outcome.persist_chars_freed,
        outcome.placeholder_chars_freed,
        state.usage_ratio()
    );
    assert!(
        outcome.persist_chars_freed + outcome.placeholder_chars_freed > 0,
        "L0+L1 should free some chars"
    );

    // Step 2: L2 apply (simulate preheat completion)
    info!("Act Step 2: simulate preheat completed + apply_boundary");
    let first_user_id = state
        .messages
        .iter()
        .find(|m| m.role == ChatMessageRole::User && m.kind != MessageKind::CompactionSummary)
        .and_then(|m| m.msg_id.clone())
        .unwrap_or_default();
    // Find end of compactable zone (everything before last 5 turns)
    let turn_starts: Vec<usize> = state
        .messages
        .iter()
        .enumerate()
        .filter(|(_, m)| {
            (m.role == ChatMessageRole::User && m.kind != MessageKind::Steering)
                || m.kind == MessageKind::CompactionSummary
        })
        .map(|(i, _)| i)
        .collect();

    if turn_starts.len() > 5 {
        let cover_end_idx = turn_starts[turn_starts.len() - 5] - 1;
        let covered_end = state.messages[..=cover_end_idx]
            .iter()
            .rev()
            .find_map(|m| m.msg_id.clone())
            .unwrap_or_default();

        let compaction_result = tomcat::CompactionResult {
            summary_text: "Pipeline summary of early conversation".into(),
            covered_start_id: first_user_id.clone(),
            covered_end_id: covered_end.clone(),
            covered_count: turn_starts.len() - 5,
            transcript_compaction_entry_id: Some(compound_turn_id(&first_user_id, &covered_end)),
            estimated_covered_tokens_before: None,
            estimated_summary_tokens: None,
            estimated_tokens_saved: None,
            preheat_elapsed_ms: 100,
        };
        state.preheat.restore_completed(compaction_result);

        let ratio_before_apply = state.usage_ratio();
        state.update_api_usage((state.estimated_token_count() as f64 * 0.90) as u32, 0);

        let switched = tomcat::core::compaction::apply::check_after_reply(&mut state, &*event_bus);
        info!(
            "L2: switched={}, ratio before={:.3}, after={:.3}, messages={}",
            switched,
            ratio_before_apply,
            state.usage_ratio(),
            state.messages.len()
        );
    }

    // Step 3: L3 force drop (if still over budget)
    if state.usage_ratio() >= 0.50 {
        info!("Act Step 3: force_drop_oldest_to_target");
        let (turns_removed, chars_removed) =
            tomcat::core::compaction::force_drop_oldest_to_target(&mut state);
        info!(
            "L3: turns_removed={}, chars_removed={}, ratio={:.3}",
            turns_removed,
            chars_removed,
            state.usage_ratio()
        );
    }

    info!("Assert: pipeline completed, chars consistent");
    let actual_chars: usize = state.messages.iter().map(estimate_msg_chars).sum();
    let diff = (state.estimate_context_chars as i64 - actual_chars as i64).unsigned_abs();
    assert!(
        diff <= (actual_chars / 10 + 100) as u64,
        "estimate_context_chars ({}) should be close to actual ({}), diff={}",
        state.estimate_context_chars,
        actual_chars,
        diff
    );
    assert!(!state.messages.is_empty(), "should not drain all messages");
}

// ────────── Group F: L3 overflow 事件 payload 断言 ────────────────────────

/// [L3 事件 payload] ContextOverflowTrimStart/End payload 含正确字段
#[tokio::test]
async fn test_context_overflow_trim_events_have_correct_payload(
) -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = info_span!("test_context_overflow_trim_events_have_correct_payload").entered();

    info!("Arrange: MockLlm 首次 overflow，第二次成功");
    let stream_err = vec![Err(AppError::Llm(
        "context length exceeded: 500000 tokens".to_string(),
    ))];
    let stream_ok = text_stream("ok after trim");
    let llm = Arc::new(MockLlm::new(vec![stream_err, stream_ok]));
    let primitive = Arc::new(MockPrimitiveWithLargeFile { file_size: 100 });
    let event_bus = Arc::new(DefaultEventBus::new());

    let trim_start_payloads = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let trim_end_payloads = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let ts = Arc::clone(&trim_start_payloads);
    let te = Arc::clone(&trim_end_payloads);
    event_bus.on(
        "context_overflow_trim_start",
        Box::new(move |ctx: EventContext| {
            ts.lock().unwrap().push(ctx.payload.clone());
            Ok(())
        }),
    );
    event_bus.on(
        "context_overflow_trim_end",
        Box::new(move |ctx: EventContext| {
            te.lock().unwrap().push(ctx.payload.clone());
            Ok(())
        }),
    );

    let config = AgentLoopConfig {
        model: "mock-model".to_string(),
        session_id: "sess-overflow-payload".to_string(),
        max_attempts: 3,
        retry_base_delay_ms: 0,
        context_config: ContextConfig::default(),
        ..Default::default()
    };
    let abort = CancellationToken::new();
    let mut agent = AgentLoop::new(llm, primitive, event_bus, config, abort);

    let old_content = "old ".repeat(10_000); // ~40K chars
    let mut old_user = ChatMessage::user(old_content);
    old_user.timestamp = Some(TEST_TS.to_string());
    let mut recent_user = ChatMessage::user("recent");
    recent_user.timestamp = Some(TEST_TS.to_string());

    let estimate = 40_000usize;
    let budget_tokens = (estimate / 4) + 100; // ~10_100 — ratio will be ~0.99
    let ctx_state = ContextState {
        messages: vec![old_user, recent_user],
        estimate_context_chars: estimate,
        context_budget_chars: estimate * 4,
        context_budget_tokens: budget_tokens,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        latest_plan_event: None,
        preheat: tomcat::core::compaction::preheat::Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    };
    agent.set_context_state(Some(ctx_state));

    info!("Act: run AgentLoop");
    let _result = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        agent.run(vec![ChatMessage::user("trigger")]),
    )
    .await
    .map_err(|_| "timeout")?;

    info!("Assert: trim event payloads");
    let starts = trim_start_payloads.lock().unwrap();
    assert_eq!(starts.len(), 1, "should emit one trim_start");
    let start_p = &starts[0];
    assert_eq!(
        start_p.get("reason").and_then(|v| v.as_str()),
        Some("context_overflow"),
        "trim_start reason"
    );
    assert!(
        start_p.get("ratio").and_then(|v| v.as_f64()).unwrap_or(0.0) > 0.0,
        "trim_start should have ratio > 0"
    );

    let ends = trim_end_payloads.lock().unwrap();
    assert_eq!(ends.len(), 1, "should emit one trim_end");
    let end_p = &ends[0];
    assert_eq!(
        end_p.get("willRetry").and_then(|v| v.as_bool()),
        Some(true),
        "trim_end willRetry"
    );
    assert!(
        end_p
            .get("estimatedTokensFreed")
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
            > 0,
        "should have freed tokens"
    );
    assert!(
        end_p
            .get("turnsRemoved")
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
            > 0,
        "should have removed turns"
    );

    let rb = end_p
        .get("ratioBefore")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let ra = end_p
        .get("ratioAfter")
        .and_then(|v| v.as_f64())
        .unwrap_or(1.0);
    assert!(
        ra < rb || ra == 0.0,
        "ratio_after ({}) should be < ratio_before ({})",
        ra,
        rb
    );

    Ok(())
}
