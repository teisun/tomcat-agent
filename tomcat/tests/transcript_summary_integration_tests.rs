//! `transcript_summary_integration_tests` — §2 黑盒集成测试（transcript-ui-restore）。
//!
//! 通过 **pub API** 端到端断言本次新增的后端对外能力：
//! - utility 模型生成的 TurnEnd `summary_title`（成功 / 纯文本 None / 失败回退规则）；
//! - `update_plan` / `todos` 工具执行后经 `PlanRuntime::write_transcript_custom`
//!   emit 的 `plan.todos` / `session.todos` transcript 自定义事件。
//!
//! 全程 mock LlmProvider，不依赖真实 API key；HOME 隔离保持并发安全。
//! 命名 `测试对象_状态_预期结果`；AAA；`tracing::info!(target:"test", phase=...)` 三节点日志。

#![allow(clippy::field_reassign_with_default)]

mod common;

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serial_test::serial;
use tokio_util::sync::CancellationToken;

use tomcat::core::llm::thinking_policy::ThinkingFormat;
use tomcat::core::llm::{ChatMessage, StreamEvent};
use tomcat::core::plan_runtime::file_store::{
    plan_path_for_id, read_plan, write_plan, PlanFileState, TodoStatus,
};
use tomcat::core::plan_runtime::{PlanRuntime, TranscriptAppender};
use tomcat::core::tools::plan_tool::{create_plan, todos, update_plan};
use tomcat::{
    init_context_state, run_chat_turn, wire, AgentLoop, AgentLoopConfig, AgentRunOutcome,
    AppConfig, AppError, BashResult, Capabilities, ChatContext, ChatRequest, ChatResponse,
    ChatResponseChoice, DirEntry, EditFileResult, EditOperation, EventBus, EventContext,
    LlmProvider, LlmResolver, LlmScene, PrimitiveExecutor, PrimitiveOperation, ResolvedCall,
    WriteFileResult,
};
use tracing::info;

// ─── Mock LLM ──────────────────────────────────────────────────────────────

/// 主对话 provider：依序消费预设 `chat_stream` 事件序列；`chat` 不被使用。
struct MainStreamLlm {
    streams: Mutex<VecDeque<Vec<Result<StreamEvent, AppError>>>>,
    chat_reply: Option<String>,
    chat_call_count: AtomicUsize,
}

impl MainStreamLlm {
    fn new(streams: Vec<Vec<Result<StreamEvent, AppError>>>) -> Self {
        Self {
            streams: Mutex::new(streams.into()),
            chat_reply: None,
            chat_call_count: AtomicUsize::new(0),
        }
    }

    fn with_chat_reply(
        streams: Vec<Vec<Result<StreamEvent, AppError>>>,
        chat_reply: impl Into<String>,
    ) -> Self {
        Self {
            streams: Mutex::new(streams.into()),
            chat_reply: Some(chat_reply.into()),
            chat_call_count: AtomicUsize::new(0),
        }
    }

    fn chat_call_count(&self) -> usize {
        self.chat_call_count.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl LlmProvider for MainStreamLlm {
    fn provider_name(&self) -> &str {
        "mock-main-stream"
    }
    async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, AppError> {
        self.chat_call_count.fetch_add(1, Ordering::Relaxed);
        let Some(reply) = &self.chat_reply else {
            return Err(AppError::Llm("main mock chat not used".to_string()));
        };
        Ok(ChatResponse {
            id: None,
            choices: vec![ChatResponseChoice {
                index: 0,
                message: ChatMessage::assistant(reply),
                finish_reason: Some("stop".to_string()),
            }],
            usage: None,
        })
    }
    async fn chat_stream(
        &self,
        _req: ChatRequest,
    ) -> Result<
        Box<dyn tokio_stream::Stream<Item = Result<StreamEvent, AppError>> + Send + Unpin>,
        AppError,
    > {
        let events = self
            .streams
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| AppError::Llm("MainStreamLlm: no more streams".to_string()))?;
        Ok(Box::new(tokio_stream::iter(events)))
    }
    fn count_tokens(&self, _messages: &[ChatMessage]) -> Result<u32, AppError> {
        Ok(0)
    }
}

/// Title / utility provider：`chat` 返回脚本化标题或错误（驱动成功 / 回退两条路径）。
struct TitleChatLlm {
    title: String,
    fail: bool,
    call_count: AtomicUsize,
}

impl TitleChatLlm {
    fn ok(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            fail: false,
            call_count: AtomicUsize::new(0),
        }
    }
    fn failing() -> Self {
        Self {
            title: String::new(),
            fail: true,
            call_count: AtomicUsize::new(0),
        }
    }

    fn call_count(&self) -> usize {
        self.call_count.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl LlmProvider for TitleChatLlm {
    fn provider_name(&self) -> &str {
        "mock-title"
    }
    async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, AppError> {
        self.call_count.fetch_add(1, Ordering::Relaxed);
        if self.fail {
            return Err(AppError::Llm("title mock failure".to_string()));
        }
        Ok(ChatResponse {
            id: None,
            choices: vec![ChatResponseChoice {
                index: 0,
                message: ChatMessage::assistant(&self.title),
                finish_reason: Some("stop".to_string()),
            }],
            usage: None,
        })
    }
    async fn chat_stream(
        &self,
        _req: ChatRequest,
    ) -> Result<
        Box<dyn tokio_stream::Stream<Item = Result<StreamEvent, AppError>> + Send + Unpin>,
        AppError,
    > {
        Ok(Box::new(tokio_stream::iter(Vec::<
            Result<StreamEvent, AppError>,
        >::new())))
    }
    fn count_tokens(&self, _messages: &[ChatMessage]) -> Result<u32, AppError> {
        Ok(0)
    }
}

// ─── Mock PrimitiveExecutor ────────────────────────────────────────────────

struct MockPrimitive;

#[async_trait]
impl PrimitiveExecutor for MockPrimitive {
    async fn read_file(&self, path: &str, _plugin_id: &str) -> Result<String, AppError> {
        Ok(format!("content:{path}"))
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
            stdout: format!("out:{command}"),
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

// ─── Helpers ───────────────────────────────────────────────────────────────

fn default_config(session_id: &str, title: Arc<dyn LlmProvider>) -> AgentLoopConfig {
    AgentLoopConfig {
        model: "mock-model".to_string(),
        session_id: session_id.to_string(),
        max_attempts: 3,
        retry_base_delay_ms: 0,
        title_provider: Some(title),
        title_model: "utility-flash".to_string(),
        ..Default::default()
    }
}

struct SceneResolver {
    main: Arc<dyn LlmProvider>,
    title: Arc<dyn LlmProvider>,
    main_model: String,
    title_model: String,
    resolve_title: bool,
}

impl SceneResolver {
    fn new(
        main: Arc<dyn LlmProvider>,
        title: Arc<dyn LlmProvider>,
        main_model: impl Into<String>,
        title_model: impl Into<String>,
        resolve_title: bool,
    ) -> Self {
        Self {
            main,
            title,
            main_model: main_model.into(),
            title_model: title_model.into(),
            resolve_title,
        }
    }
}

impl LlmResolver for SceneResolver {
    fn resolve(
        &self,
        scene: LlmScene,
        _session_override: Option<&str>,
    ) -> Result<ResolvedCall, AppError> {
        let (provider_impl, model) = match scene {
            LlmScene::Title if self.resolve_title => {
                (Arc::clone(&self.title), self.title_model.clone())
            }
            LlmScene::Title => {
                return Err(AppError::Config(
                    "title scene intentionally unresolved for test".to_string(),
                ))
            }
            _ => (Arc::clone(&self.main), self.main_model.clone()),
        };
        Ok(ResolvedCall {
            provider_impl,
            model,
            api: "mock".to_string(),
            provider: "mock".to_string(),
            base_url: None,
            key_source: "test".to_string(),
            thinking_format: ThinkingFormat::Openai,
            capabilities: Capabilities::default(),
        })
    }
}

fn deterministic_chat_context_fixture(env_key: &str) -> (tempfile::TempDir, ChatContext) {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(dir.path().to_string_lossy().to_string());
    common::apply_openai_responses_test_config(&mut cfg, env_key, None);
    // SAFETY: 测试使用独立 env key，作用域结束后显式清理。
    unsafe { std::env::set_var(env_key, "stub") };
    let ctx = ChatContext::from_config(cfg).expect("chat context should be created");
    let session_key = ctx
        .session_runtime
        .session
        .current_session_key()
        .to_string();
    ctx.session_runtime
        .session
        .create_session(&session_key, None)
        .unwrap();
    (dir, ctx)
}

fn install_scene_resolver(
    ctx: &mut ChatContext,
    main: Arc<dyn LlmProvider>,
    title: Arc<dyn LlmProvider>,
    default_model: &str,
    title_model: &str,
) {
    ctx.global_services.llm = main.clone();
    ctx.global_services.llm_resolver = Arc::new(SceneResolver::new(
        main,
        title,
        default_model,
        title_model,
        true,
    ));
}

fn install_scene_resolver_without_title(
    ctx: &mut ChatContext,
    main: Arc<dyn LlmProvider>,
    title: Arc<dyn LlmProvider>,
    default_model: &str,
    title_model: &str,
) {
    ctx.global_services.llm = main.clone();
    ctx.global_services.llm_resolver = Arc::new(SceneResolver::new(
        main,
        title,
        default_model,
        title_model,
        false,
    ));
}

/// 纯文本收敛流（一轮 text-only 回合）。
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

/// 带 thinking 快照 + content + N 个 read tool_call 的工具回合流。
fn thinking_read_tool_stream(n_reads: usize) -> Vec<Result<StreamEvent, AppError>> {
    let mut events = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "Let me read the relevant files.".to_string(),
        }),
        Ok(StreamEvent::ReasoningSnapshot {
            thinking_text: Some("Analyzing the requested files to review.".to_string()),
            reasoning_continuation: None,
            continuity: None,
        }),
    ];
    for i in 0..n_reads {
        events.push(Ok(StreamEvent::ToolCallDelta {
            index: i as u32,
            id: Some(format!("c{i}")),
            name: Some("read".to_string()),
            arguments_delta: Some(format!(r#"{{"path":"/file_{i}"}}"#)),
        }));
    }
    events.push(Ok(StreamEvent::FinishReason {
        reason: "tool_calls".to_string(),
    }));
    events
}

fn thinking_single_read_tool_stream(path: &str) -> Vec<Result<StreamEvent, AppError>> {
    vec![
        Ok(StreamEvent::ContentDelta {
            delta: "Let me inspect that file.".to_string(),
        }),
        Ok(StreamEvent::ReasoningSnapshot {
            thinking_text: Some("Inspecting the requested file before answering.".to_string()),
            reasoning_continuation: None,
            continuity: None,
        }),
        Ok(StreamEvent::ToolCallDelta {
            index: 0,
            id: Some("read-1".to_string()),
            name: Some("read".to_string()),
            arguments_delta: Some(format!(r#"{{"path":"{}"}}"#, path)),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "tool_calls".to_string(),
        }),
    ]
}

#[derive(Debug, Default, Clone)]
struct TurnSummaryCapture {
    turn_end_payloads: Vec<serde_json::Value>,
    turn_summary_updated_payloads: Vec<serde_json::Value>,
}

/// 订阅 `turn_end` / `turn.summary_updated`，记录完整 payload 供断言。
fn capture_turn_summary_events(bus: &dyn EventBus) -> Arc<Mutex<TurnSummaryCapture>> {
    let captured: Arc<Mutex<TurnSummaryCapture>> =
        Arc::new(Mutex::new(TurnSummaryCapture::default()));
    let cap = Arc::clone(&captured);
    bus.on(
        wire::WIRE_TURN_END,
        Box::new(move |ctx: EventContext| {
            cap.lock()
                .unwrap()
                .turn_end_payloads
                .push(ctx.payload.clone());
            Ok(())
        }),
    );
    let cap = Arc::clone(&captured);
    bus.on(
        wire::WIRE_TURN_SUMMARY_UPDATED,
        Box::new(move |ctx: EventContext| {
            cap.lock()
                .unwrap()
                .turn_summary_updated_payloads
                .push(ctx.payload.clone());
            Ok(())
        }),
    );
    captured
}

/// 跑一轮主循环并返回捕获到的 turn summary 相关事件。
async fn run_and_collect_summaries(
    main_streams: Vec<Vec<Result<StreamEvent, AppError>>>,
    title: Arc<dyn LlmProvider>,
    session_id: &str,
) -> TurnSummaryCapture {
    let llm = Arc::new(MainStreamLlm::new(main_streams));
    let primitive = Arc::new(MockPrimitive);
    let event_bus: Arc<dyn EventBus> = Arc::new(tomcat::DefaultEventBus::new());
    let captured = capture_turn_summary_events(&*event_bus);
    let config = default_config(session_id, title);
    let mut agent = AgentLoop::new(llm, primitive, event_bus, config, CancellationToken::new());
    let messages = vec![ChatMessage::user("please review the files")];
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(10), agent.run(messages))
        .await
        .expect("run() 超时 10s");
    assert!(
        matches!(outcome, AgentRunOutcome::Completed(_)),
        "AgentLoop::run 应 Completed，实际: {outcome:?}"
    );
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let result = captured.lock().unwrap().clone();
    result
}

// ─── HOME 隔离 fixture（tests 4-6 共享，进程内串行） ────────────────────────

fn setup_home() -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "tomcat_transcript_summary_home_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(p.join(".tomcat").join("plans")).unwrap();
    std::env::set_var("HOME", &p);
    p
}

fn cleanup_home(p: &std::path::Path) {
    let _ = std::fs::remove_dir_all(p);
}

/// 把磁盘 plan 提升到 executing 并同步内存（单测专用，绕过 build_plan 锁竞争）。
fn promote_to_exec(rt: &PlanRuntime, plan_id: &str) {
    let path = plan_path_for_id(plan_id).unwrap();
    let mut plan = read_plan(&path).unwrap();
    plan.frontmatter.state = PlanFileState::Executing;
    plan.frontmatter.session_key = Some("session-a".into());
    plan.frontmatter.session_id = Some("sid-a".into());
    write_plan(&path, &plan, 2000).unwrap();
    rt.set_executing_for_test(plan_id.to_string());
}

/// 挂一个 transcript appender spy，记录所有 `write_transcript_custom` 入参。
fn attach_transcript_spy(rt: &PlanRuntime) -> Arc<Mutex<Vec<serde_json::Value>>> {
    let captured: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
    let cap = Arc::clone(&captured);
    let appender: TranscriptAppender = Arc::new(move |extra| {
        cap.lock().unwrap().push(extra);
        Ok(())
    });
    rt.attach_transcript_appender(appender);
    captured
}

// ─── Tests 1-3：TurnEnd summary_title ───────────────────────────────────────

/// thinking + 一个 read tool 在场 → TurnEnd 立即带规则占位；稍后 emit `turn.summary_updated`。
#[tokio::test]
async fn turnend_emits_fallback_title_then_turn_summary_updated_emits_semantic_title() {
    common::setup_logging();
    info!(target: "test", phase = "arrange", "mock main 流式 thinking+content+1 read tool_call；title provider 返回更自然的 utility 标题");
    let title = Arc::new(TitleChatLlm::ok("Reviewed requested file"));

    info!(target: "test", phase = "act", "驱动 AgentLoop::run 跑一轮工具回合 + 一轮 text 收敛");
    let captured = run_and_collect_summaries(
        vec![thinking_read_tool_stream(1), text_stream("Done reviewing.")],
        title,
        "sess-summary-present",
    )
    .await;

    info!(target: "test", phase = "assert", " turn summary 相关事件 = {:?}", captured);
    let turn_end_titles: Vec<Option<String>> = captured
        .turn_end_payloads
        .iter()
        .map(|payload| {
            payload
                .get("summaryTitle")
                .and_then(|v| v.as_str())
                .map(ToOwned::to_owned)
        })
        .collect();
    assert!(
        turn_end_titles
            .iter()
            .any(|title| title.as_deref() == Some("Read path=/file_0")),
        "工具回合 TurnEnd 应立即携带规则占位标题，实际: {turn_end_titles:?}"
    );
    assert!(
        captured
            .turn_summary_updated_payloads
            .iter()
            .any(|payload| {
                payload.get("summaryTitle").and_then(|v| v.as_str())
                    == Some("Reviewed requested file")
            }),
        "utility 标题完成后应 emit turn.summary_updated=Reviewed requested file，实际: {:?}",
        captured.turn_summary_updated_payloads
    );
}

/// 纯文本回合（无 thinking、无 tool）→ TurnEnd `summary_title == None`。
#[tokio::test]
async fn turnend_summary_title_none_when_no_thinking_no_tool() {
    common::setup_logging();
    info!(target: "test", phase = "arrange", "mock main 仅流式 ContentDelta（text-only）；title provider 不会被调用");
    let title = Arc::new(TitleChatLlm::ok("should not be used"));

    info!(target: "test", phase = "act", "驱动 AgentLoop::run 跑一轮 text-only 回合");
    let captured = run_and_collect_summaries(
        vec![text_stream("Hello there.")],
        title,
        "sess-summary-none",
    )
    .await;
    let summaries: Vec<Option<String>> = captured
        .turn_end_payloads
        .iter()
        .map(|payload| {
            payload
                .get("summaryTitle")
                .and_then(|v| v.as_str())
                .map(ToOwned::to_owned)
        })
        .collect();

    info!(target: "test", phase = "assert", " TurnEnd summary_title 序列 = {:?}", summaries);
    assert_eq!(
        summaries.len(),
        1,
        "text-only 回合应只发一条 TurnEnd，实际: {summaries:?}"
    );
    assert_eq!(
        summaries[0], None,
        "无 thinking 无 tool 时 summary_title 必须为 None，实际: {:?}",
        summaries[0]
    );
    assert!(
        captured.turn_summary_updated_payloads.is_empty(),
        "text-only 回合不应 emit turn.summary_updated，实际: {:?}",
        captured.turn_summary_updated_payloads
    );
}

/// title provider 失败 → TurnEnd 保留规则摘要，且不再补发 `turn.summary_updated`。
#[tokio::test]
async fn turnend_summary_title_falls_back_to_rule_on_utility_failure() {
    common::setup_logging();
    info!(target: "test", phase = "arrange", "mock main 流式 thinking+2 read tool_calls；title provider chat 返回 Err");
    let title = Arc::new(TitleChatLlm::failing());

    info!(target: "test", phase = "act", "驱动 AgentLoop::run 跑一轮 2-tool 回合 + 一轮 text 收敛");
    let captured = run_and_collect_summaries(
        vec![thinking_read_tool_stream(2), text_stream("Done reviewing.")],
        title,
        "sess-summary-fallback",
    )
    .await;
    let summaries: Vec<Option<String>> = captured
        .turn_end_payloads
        .iter()
        .map(|payload| {
            payload
                .get("summaryTitle")
                .and_then(|v| v.as_str())
                .map(ToOwned::to_owned)
        })
        .collect();

    info!(target: "test", phase = "assert", " TurnEnd summary_title 序列 = {:?}", summaries);
    assert!(
        summaries
            .iter()
            .any(|s| s.as_deref() == Some("Reviewed 2 files")),
        "utility 失败应回退规则摘要 \"Reviewed 2 files\"，实际: {summaries:?}"
    );
    assert!(
        captured.turn_summary_updated_payloads.is_empty(),
        "utility 失败时不应再 emit turn.summary_updated，实际: {:?}",
        captured.turn_summary_updated_payloads
    );
}

/// `turn.summary_updated` 生成后应回写 assistant message `summary_title`，reload 不丢。
#[tokio::test]
#[serial]
async fn turn_summary_updated_persists_summary_title_into_assistant_message() {
    common::setup_logging();
    let env_key = "OPENAI_API_KEY_TRANSCRIPT_SUMMARY_PERSIST";
    let (work_dir, mut ctx) = deterministic_chat_context_fixture(env_key);
    let file_path = work_dir.path().join("review-me.txt");
    std::fs::write(&file_path, "hello from transcript summary persistence test").unwrap();

    let main = Arc::new(MainStreamLlm::new(vec![
        thinking_single_read_tool_stream(&file_path.to_string_lossy()),
        text_stream("Done."),
    ]));
    let title = Arc::new(TitleChatLlm::ok("Reviewed requested file"));
    install_scene_resolver(&mut ctx, main, title.clone(), "gpt-5.4", "utility-flash");
    let system_text = "system prompt";
    let mut state = init_context_state(
        &ctx.session_runtime.session,
        &ctx.config.context,
        system_text,
    )
    .expect("init_context_state");

    info!(target: "test", phase = "act", "run_chat_turn 触发 read 工具回合与异步 turn summary rewrite");
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(8),
        run_chat_turn(
            &ctx,
            "请查看这个文件",
            system_text,
            &mut state,
            CancellationToken::new(),
        ),
    )
    .await
    .expect("run_chat_turn timeout 8s")
    .expect("run_chat_turn should succeed");
    assert!(
        matches!(outcome, AgentRunOutcome::Completed(_)),
        "run_chat_turn 应 Completed，实际: {outcome:?}"
    );

    let persisted = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let entries = ctx.session_runtime.session.get_entries(32).unwrap();
            let found = entries.iter().find_map(|entry| match entry {
                tomcat::TranscriptEntry::Message(message_entry) => {
                    let role = message_entry
                        .message
                        .get("role")
                        .and_then(|v| v.as_str());
                    let has_tool_calls = message_entry
                        .message
                        .get("tool_calls")
                        .and_then(|v| v.as_array())
                        .is_some_and(|arr| !arr.is_empty());
                    let summary_title = message_entry
                        .message
                        .get("summary_title")
                        .and_then(|v| v.as_str());
                    if role == Some("assistant") && has_tool_calls {
                        summary_title.map(str::to_string)
                    } else {
                        None
                    }
                }
                _ => None,
            });
            match found {
                Some(title) if title == "Reviewed requested file" => break title,
                Some(title) => {
                    info!(target: "test", phase = "poll", "assistant summary_title currently = {:?}", title);
                }
                None => {}
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("assistant summary_title should be persisted");

    info!(target: "test", phase = "assert", "persisted assistant summary_title = {:?}", persisted);
    assert_eq!(persisted, "Reviewed requested file");

    unsafe { std::env::remove_var(env_key) };
    drop(work_dir);
}

// ─── Tests 4-5：plan.todos / session.todos transcript 事件 ──────────────────

/// `update_plan` 执行后 transcript 出现 `event=plan.todos` 且 `todos` 数组非空。
#[tokio::test]
#[serial]
async fn update_plan_emits_plan_todos_event() {
    common::setup_logging();
    info!(target: "test", phase = "arrange", "隔离 HOME + PlanRuntime + transcript spy；create_plan 并提升到 executing");
    let home = setup_home();
    let rt = PlanRuntime::new("session-a");
    let captured = attach_transcript_spy(&rt);

    rt.enter_planning().unwrap();
    let out = create_plan::execute(
        &rt,
        create_plan::CreatePlanArgs {
            goal: "ship feature X".into(),
            draft: "## Goal\nship X".into(),
            todos: vec![create_plan::TodoArg {
                id: "t1".into(),
                content: "step 1".into(),
                status: TodoStatus::Pending,
            }],
        },
    )
    .unwrap();
    let plan_id = out["plan_id"].as_str().unwrap().to_string();
    promote_to_exec(&rt, &plan_id);
    // 仅校验 update_plan 的 emit，清掉 create_plan 阶段已写入的事件。
    captured.lock().unwrap().clear();

    info!(target: "test", phase = "act", "调用 update_plan::execute 把 t1 推进到 in_progress");
    update_plan::execute(
        &rt,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id.clone()),
            path: None,
            replace: false,
            ops: vec![update_plan::UpdateOp::SetStatus {
                id: "t1".into(),
                content: None,
                status: TodoStatus::InProgress,
            }],
        },
    )
    .await
    .unwrap();

    info!(target: "test", phase = "assert", "transcript 自定义事件 {:?}", captured.lock().unwrap());
    let entries = captured.lock().unwrap().clone();
    let plan_todos = entries
        .iter()
        .find(|v| v.get("event").and_then(|e| e.as_str()) == Some(wire::WIRE_PLAN_TODOS));
    let plan_todos = plan_todos.expect("update_plan 应 emit plan.todos 事件");
    let todos_arr = plan_todos
        .get("todos")
        .and_then(|t| t.as_array())
        .expect("plan.todos 事件须带 todos 数组");
    assert!(
        !todos_arr.is_empty(),
        "plan.todos 数组应非空（含 t1），实际: {todos_arr:?}"
    );
    assert_eq!(
        plan_todos.get("plan_id").and_then(|v| v.as_str()),
        Some(plan_id.as_str()),
        "plan.todos 事件应携带 plan_id"
    );
    cleanup_home(&home);
}

/// `todos` 工具执行后 transcript 出现 `event=session.todos` 且 `todos` 数组非空。
#[tokio::test]
#[serial]
async fn todos_tool_emits_session_todos_event() {
    common::setup_logging();
    info!(target: "test", phase = "arrange", "隔离 HOME + PlanRuntime（Chat 模式）+ transcript spy");
    let home = setup_home();
    let rt = PlanRuntime::new("session-a");
    let captured = attach_transcript_spy(&rt);

    info!(target: "test", phase = "act", "调用 todos::execute upsert 一条 session scratchpad todo");
    todos::execute(
        &rt,
        None,
        todos::TodosArgs {
            new_todos: false,
            title: None,
            replace: false,
            ops: vec![todos::TodoOpArg::Upsert {
                id: "t1".into(),
                content: Some("chat scratchpad".into()),
                status: Some(TodoStatus::Pending),
            }],
        },
    )
    .unwrap();

    info!(target: "test", phase = "assert", "transcript 自定义事件 {:?}", captured.lock().unwrap());
    let entries = captured.lock().unwrap().clone();
    let session_todos = entries
        .iter()
        .find(|v| v.get("event").and_then(|e| e.as_str()) == Some(wire::WIRE_SESSION_TODOS))
        .expect("todos 工具应 emit session.todos 事件");
    let todos_arr = session_todos
        .get("todos")
        .and_then(|t| t.as_array())
        .expect("session.todos 事件须带 todos 数组");
    assert_eq!(
        todos_arr.len(),
        1,
        "session.todos 应含 1 条 todo，实际: {todos_arr:?}"
    );
    cleanup_home(&home);
}

// ─── Test 6：serve get_state（pub(crate) harness，留 ignore 指向 lib 单测） ──

/// `get_state` 响应应含 `planTodos` / `sessionTodos` 数组。
///
/// `serve::commands::handle_command` 与 `ServeState` / `ServeCommand` 均为 `pub(crate)`，
/// 集成测试二进制无法在进程内直接装配；serve stdio 子进程 harness（`serve_stdio_e2e`
/// 等）较重且需串行隔离。该断言已落地为 lib 单测
/// `serve_get_state_contains_plan_and_session_todos`（见
/// `tomcat/src/api/serve/tests/commands_test.rs`，具 `pub(crate)` 访问权，
/// 注入 session todo 后断言 `sessionTodos` 非空 + `planTodos` 为数组），此处仅留集成层占位指针。
#[tokio::test]
#[ignore = "已在 lib 单测 serve_get_state_contains_plan_and_session_todos（commands_test.rs）覆盖；集成二进制无 pub(crate) 访问权"]
async fn get_state_contains_plan_and_session_todos() {
    common::setup_logging();
    info!(target: "test", phase = "arrange", "skipped: 见 commands_test.rs::serve_get_state_contains_plan_and_session_todos");
    // 真实断言已落 lib 单测路径（commands_test.rs）。
}

// ─── Test 7-8：session.title_updated 异步 ────────────────────────────────────

/// 首条 user 先 emit 规则标题，再异步 emit 语义 session 标题。
#[tokio::test]
#[serial]
async fn session_title_updated_emitted_after_first_user() {
    common::setup_logging();
    let env_key = "OPENAI_API_KEY_TRANSCRIPT_SUMMARY_TITLE_EMIT";
    let (work_dir, mut ctx) = deterministic_chat_context_fixture(env_key);
    let captured: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let cap = Arc::clone(&captured);
    ctx.global_services.event_bus.on(
        wire::WIRE_SESSION_TITLE_UPDATED,
        Box::new(move |ctx: EventContext| {
            if let Some(title) = ctx.payload.get("title").and_then(|v| v.as_str()) {
                cap.lock().unwrap().push(title.to_string());
            }
            Ok(())
        }),
    );
    let main = Arc::new(MainStreamLlm::new(vec![text_stream("Done.")]));
    let title = Arc::new(TitleChatLlm::ok("Semantic session title"));
    install_scene_resolver(&mut ctx, main, title.clone(), "gpt-5.4", "utility-flash");
    let system_text = "system prompt";
    let mut state = init_context_state(
        &ctx.session_runtime.session,
        &ctx.config.context,
        system_text,
    )
    .expect("init_context_state");

    info!(target: "test", phase = "act", "run_chat_turn 写入首条 user，并等待异步 title event");
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        run_chat_turn(
            &ctx,
            "请帮我整理 transcript UI",
            system_text,
            &mut state,
            CancellationToken::new(),
        ),
    )
    .await
    .expect("run_chat_turn timeout 5s")
    .expect("run_chat_turn should succeed");
    assert!(
        matches!(outcome, AgentRunOutcome::Completed(_)),
        "run_chat_turn 应 Completed，实际: {outcome:?}"
    );
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if captured.lock().unwrap().len() >= 2 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("session.title_updated should arrive");

    info!(target: "test", phase = "assert", "captured session.title_updated = {:?}", captured.lock().unwrap());
    assert_eq!(
        captured.lock().unwrap().clone(),
        vec![
            "请帮我整理 transcript UI".to_string(),
            "Semantic session title".to_string(),
        ],
        "首条 user 后应先 emit 规则标题，再异步 emit 语义 session 标题"
    );
    assert_eq!(title.call_count(), 1, "title provider 应被调用一次");
    let title_on_disk = ctx
        .session_runtime
        .session
        .current_session_entry()
        .unwrap()
        .and_then(|entry| entry.title)
        .unwrap_or_default();
    assert_eq!(title_on_disk, "Semantic session title");

    unsafe { std::env::remove_var(env_key) };
    drop(work_dir);
}

/// Title scene 未解析时，会话标题应降级到主模型；事件顺序仍为 L0 先、L1 后。
#[tokio::test]
#[serial]
async fn session_title_updated_falls_back_to_main_model_when_title_scene_unresolved() {
    common::setup_logging();
    let env_key = "OPENAI_API_KEY_TRANSCRIPT_SUMMARY_TITLE_FALLBACK";
    let (work_dir, mut ctx) = deterministic_chat_context_fixture(env_key);
    type CapturedTitles = Arc<Mutex<Vec<(String, Option<String>)>>>;
    let captured: CapturedTitles = Arc::new(Mutex::new(Vec::new()));
    let cap = Arc::clone(&captured);
    ctx.global_services.event_bus.on(
        wire::WIRE_SESSION_TITLE_UPDATED,
        Box::new(move |ctx: EventContext| {
            if let Some(title) = ctx.payload.get("title").and_then(|v| v.as_str()) {
                cap.lock()
                    .unwrap()
                    .push((title.to_string(), ctx.session_id.clone()));
            }
            Ok(())
        }),
    );
    let main = Arc::new(MainStreamLlm::with_chat_reply(
        vec![text_stream("Done.")],
        "Semantic via main model",
    ));
    let title = Arc::new(TitleChatLlm::ok("Should not resolve"));
    install_scene_resolver_without_title(
        &mut ctx,
        main.clone(),
        title,
        "mimo-v2.5-pro",
        "utility-flash",
    );
    let system_text = "system prompt";
    let mut state = init_context_state(
        &ctx.session_runtime.session,
        &ctx.config.context,
        system_text,
    )
    .expect("init_context_state");

    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        run_chat_turn(
            &ctx,
            "hello",
            system_text,
            &mut state,
            CancellationToken::new(),
        ),
    )
    .await
    .expect("run_chat_turn timeout 5s")
    .expect("run_chat_turn should succeed");
    assert!(matches!(outcome, AgentRunOutcome::Completed(_)));
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if captured.lock().unwrap().len() >= 2 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("fallback session.title_updated events should arrive");

    let captured = captured.lock().unwrap().clone();
    assert_eq!(
        captured,
        vec![
            (
                "hello".to_string(),
                ctx.session_runtime.session.current_session_id().unwrap()
            ),
            (
                "Semantic via main model".to_string(),
                ctx.session_runtime.session.current_session_id().unwrap(),
            ),
        ]
    );
    assert_eq!(
        main.chat_call_count(),
        1,
        "title scene 不可解析时应恰好降级调用主模型一次"
    );
    let title_on_disk = ctx
        .session_runtime
        .session
        .current_session_entry()
        .unwrap()
        .and_then(|entry| entry.title)
        .unwrap_or_default();
    assert_eq!(title_on_disk, "Semantic via main model");

    unsafe { std::env::remove_var(env_key) };
    drop(work_dir);
}

/// 已存在用户自定义标题时，不应再白跑 utility LLM 或 emit `session.title_updated`。
#[tokio::test]
#[serial]
async fn session_title_updated_skips_when_custom_title_already_exists() {
    common::setup_logging();
    let env_key = "OPENAI_API_KEY_TRANSCRIPT_SUMMARY_TITLE_SKIP";
    let (work_dir, mut ctx) = deterministic_chat_context_fixture(env_key);
    let captured: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let cap = Arc::clone(&captured);
    ctx.global_services.event_bus.on(
        wire::WIRE_SESSION_TITLE_UPDATED,
        Box::new(move |ctx: EventContext| {
            if let Some(title) = ctx.payload.get("title").and_then(|v| v.as_str()) {
                cap.lock().unwrap().push(title.to_string());
            }
            Ok(())
        }),
    );
    let session_key = ctx
        .session_runtime
        .session
        .current_session_key()
        .to_string();
    ctx.session_runtime
        .session
        .update_session(&session_key, |entry| {
            entry.title = Some("User custom title".to_string());
        })
        .unwrap();
    let main = Arc::new(MainStreamLlm::new(vec![text_stream("Done.")]));
    let title = Arc::new(TitleChatLlm::ok("Should not be called"));
    install_scene_resolver(&mut ctx, main, title.clone(), "gpt-5.4", "utility-flash");
    let system_text = "system prompt";
    let mut state = init_context_state(
        &ctx.session_runtime.session,
        &ctx.config.context,
        system_text,
    )
    .expect("init_context_state");

    info!(target: "test", phase = "act", "run_chat_turn 在已有自定义标题的 session 内追加首条 user");
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        run_chat_turn(
            &ctx,
            "这条消息不应触发语义标题生成",
            system_text,
            &mut state,
            CancellationToken::new(),
        ),
    )
    .await
    .expect("run_chat_turn timeout 5s")
    .expect("run_chat_turn should succeed");
    assert!(
        matches!(outcome, AgentRunOutcome::Completed(_)),
        "run_chat_turn 应 Completed，实际: {outcome:?}"
    );
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    info!(target: "test", phase = "assert", "captured session.title_updated = {:?}", captured.lock().unwrap());
    assert!(
        captured.lock().unwrap().is_empty(),
        "已有自定义标题时不应 emit session.title_updated"
    );
    assert_eq!(
        title.call_count(),
        0,
        "已有自定义标题时不应调用 utility title provider"
    );

    unsafe { std::env::remove_var(env_key) };
    drop(work_dir);
}
