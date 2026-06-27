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
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use tomcat::core::llm::{ChatMessage, StreamEvent};
use tomcat::core::plan_runtime::file_store::{
    plan_path_for_id, read_plan, write_plan, PlanFileState, TodoStatus,
};
use tomcat::core::plan_runtime::{PlanRuntime, TranscriptAppender};
use tomcat::core::tools::plan_tool::{create_plan, todos, update_plan};
use tomcat::{
    wire, AgentLoop, AgentLoopConfig, AgentRunOutcome, AppError, BashResult, ChatRequest,
    ChatResponse, ChatResponseChoice, DirEntry, EditFileResult, EditOperation, EventBus,
    EventContext, LlmProvider, PrimitiveExecutor, PrimitiveOperation, WriteFileResult,
};
use tracing::info;

// ─── Mock LLM ──────────────────────────────────────────────────────────────

/// 主对话 provider：依序消费预设 `chat_stream` 事件序列；`chat` 不被使用。
struct MainStreamLlm {
    streams: Mutex<VecDeque<Vec<Result<StreamEvent, AppError>>>>,
}

impl MainStreamLlm {
    fn new(streams: Vec<Vec<Result<StreamEvent, AppError>>>) -> Self {
        Self {
            streams: Mutex::new(streams.into()),
        }
    }
}

#[async_trait]
impl LlmProvider for MainStreamLlm {
    fn provider_name(&self) -> &str {
        "mock-main-stream"
    }
    async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, AppError> {
        Err(AppError::Llm("main mock chat not used".to_string()))
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
}

impl TitleChatLlm {
    fn ok(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            fail: false,
        }
    }
    fn failing() -> Self {
        Self {
            title: String::new(),
            fail: true,
        }
    }
}

#[async_trait]
impl LlmProvider for TitleChatLlm {
    fn provider_name(&self) -> &str {
        "mock-title"
    }
    async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, AppError> {
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
        Ok(Box::new(tokio_stream::iter(
            Vec::<Result<StreamEvent, AppError>>::new(),
        )))
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

/// 订阅 `turn_end`，按到达顺序记录每条 TurnEnd 的 `summaryTitle`。
fn capture_turn_end_summaries(bus: &dyn EventBus) -> Arc<Mutex<Vec<Option<String>>>> {
    let captured: Arc<Mutex<Vec<Option<String>>>> = Arc::new(Mutex::new(Vec::new()));
    let cap = Arc::clone(&captured);
    bus.on(
        wire::WIRE_TURN_END,
        Box::new(move |ctx: EventContext| {
            let title = ctx
                .payload
                .get("summaryTitle")
                .and_then(|v| v.as_str())
                .map(String::from);
            cap.lock().unwrap().push(title);
            Ok(())
        }),
    );
    captured
}

/// 跑一轮主循环并返回捕获到的 TurnEnd `summaryTitle` 序列。
async fn run_and_collect_summaries(
    main_streams: Vec<Vec<Result<StreamEvent, AppError>>>,
    title: Arc<dyn LlmProvider>,
    session_id: &str,
) -> Vec<Option<String>> {
    let llm = Arc::new(MainStreamLlm::new(main_streams));
    let primitive = Arc::new(MockPrimitive);
    let event_bus: Arc<dyn EventBus> = Arc::new(tomcat::DefaultEventBus::new());
    let captured = capture_turn_end_summaries(&*event_bus);
    let config = default_config(session_id, title);
    let mut agent = AgentLoop::new(
        llm,
        primitive,
        event_bus,
        config,
        CancellationToken::new(),
    );
    let messages = vec![ChatMessage::user("please review the files")];
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        agent.run(messages),
    )
    .await
    .expect("run() 超时 10s");
    assert!(
        matches!(outcome, AgentRunOutcome::Completed(_)),
        "AgentLoop::run 应 Completed，实际: {outcome:?}"
    );
    let summaries = captured.lock().unwrap().clone();
    summaries
}

// ─── HOME 隔离 fixture（tests 4-6 共享，进程内串行） ────────────────────────

fn home_lock() -> &'static Mutex<()> {
    static M: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();
    M.get_or_init(|| Mutex::new(()))
}

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

/// thinking + 一个 read tool 在场 → TurnEnd 携带 utility 模型生成的 `summary_title`。
#[tokio::test]
async fn turnend_emits_summary_title_when_thinking_and_tool_present() {
    common::setup_logging();
    info!(target: "test", phase = "arrange", "mock main 流式 thinking+content+1 read tool_call；title provider 返回 Reviewed 2 files");
    let title = Arc::new(TitleChatLlm::ok("Reviewed 2 files"));

    info!(target: "test", phase = "act", "驱动 AgentLoop::run 跑一轮工具回合 + 一轮 text 收敛");
    let summaries = run_and_collect_summaries(
        vec![
            thinking_read_tool_stream(1),
            text_stream("Done reviewing."),
        ],
        title,
        "sess-summary-present",
    )
    .await;

    info!(target: "test", phase = "assert", " TurnEnd summary_title 序列 = {:?}", summaries);
    assert!(
        summaries.iter().any(|s| s.as_deref() == Some("Reviewed 2 files")),
        "工具回合 TurnEnd 应携带 summary_title=Some(\"Reviewed 2 files\")，实际: {summaries:?}"
    );
}

/// 纯文本回合（无 thinking、无 tool）→ TurnEnd `summary_title == None`。
#[tokio::test]
async fn turnend_summary_title_none_when_no_thinking_no_tool() {
    common::setup_logging();
    info!(target: "test", phase = "arrange", "mock main 仅流式 ContentDelta（text-only）；title provider 不会被调用");
    let title = Arc::new(TitleChatLlm::ok("should not be used"));

    info!(target: "test", phase = "act", "驱动 AgentLoop::run 跑一轮 text-only 回合");
    let summaries =
        run_and_collect_summaries(vec![text_stream("Hello there.")], title, "sess-summary-none")
            .await;

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
}

/// title provider 失败 → 回退规则摘要（2 read → "Reviewed 2 files"）。
#[tokio::test]
async fn turnend_summary_title_falls_back_to_rule_on_utility_failure() {
    common::setup_logging();
    info!(target: "test", phase = "arrange", "mock main 流式 thinking+2 read tool_calls；title provider chat 返回 Err");
    let title = Arc::new(TitleChatLlm::failing());

    info!(target: "test", phase = "act", "驱动 AgentLoop::run 跑一轮 2-tool 回合 + 一轮 text 收敛");
    let summaries = run_and_collect_summaries(
        vec![
            thinking_read_tool_stream(2),
            text_stream("Done reviewing."),
        ],
        title,
        "sess-summary-fallback",
    )
    .await;

    info!(target: "test", phase = "assert", " TurnEnd summary_title 序列 = {:?}", summaries);
    assert!(
        summaries
            .iter()
            .any(|s| s.as_deref() == Some("Reviewed 2 files")),
        "utility 失败应回退规则摘要 \"Reviewed 2 files\"，实际: {summaries:?}"
    );
}

// ─── Tests 4-5：plan.todos / session.todos transcript 事件 ──────────────────

/// `update_plan` 执行后 transcript 出现 `event=plan.todos` 且 `todos` 数组非空。
#[tokio::test]
async fn update_plan_emits_plan_todos_event() {
    common::setup_logging();
    let _g = home_lock().lock().unwrap();
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
    let plan_todos = entries.iter().find(|v| {
        v.get("event").and_then(|e| e.as_str()) == Some(wire::WIRE_PLAN_TODOS)
    });
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
async fn todos_tool_emits_session_todos_event() {
    common::setup_logging();
    let _g = home_lock().lock().unwrap();
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
    assert_eq!(todos_arr.len(), 1, "session.todos 应含 1 条 todo，实际: {todos_arr:?}");
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

// ─── Test 7：session.title_updated 异步（fire-and-forget，留 ignore 指向 lib 单测） ─

/// 首条 user 后异步 utility 模型生成 session 标题并 emit `session.title_updated`。
///
/// 该路径由 `run_loop::maybe_spawn_semantic_session_title` 以 `tokio::spawn`
/// fire-and-forget 触发，断言需在 `run_chat_turn` 返回后轮询 spawned task 完成，
/// 时序偏 racy；且需装配完整 `ChatContext`（模型解析 / 工具定义 / 落盘 runtimes）。
/// 其**确定性语义**（占位可被语义 title 覆盖一次、语义 title 不被后续 append 回退）
/// 已落地为 lib 单测 `is_rule_derived_title_distinguishes_placeholder_from_semantic`
/// 与 `placeholder_title_is_replaced_by_semantic_then_preserved`（见
/// `tomcat/src/core/session/manager/tests/append_test.rs`）；异步 emit 的进程内
/// 复刻可后续以 `tomcat/tests/agent_loop_tests.rs` 的 `run_chat_turn` harness 为模板。
#[tokio::test]
#[ignore = "确定性语义已由 append_test.rs 两个 lib 单测覆盖；异步 emit spawn 时序 racy，留作 stretch"]
async fn session_title_updated_emitted_after_first_user() {
    common::setup_logging();
    info!(target: "test", phase = "arrange", "skipped: 语义见 append_test.rs 两单测；异步 emit 留 stretch");
    // 占位：异步 emit 复刻应装配 ChatContext（agent_loop_tests::deterministic_chat_context_fixture
    // 模板）+ 订阅 WIRE_SESSION_TITLE_UPDATED，run_chat_turn 后轮询事件。
}
