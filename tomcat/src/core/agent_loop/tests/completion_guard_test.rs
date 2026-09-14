//! C1 completion guard：EXEC 下计划没收口时，模型不能靠一段文字结束回合。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;

use super::super::turn_finalize::{finalize_turn_after_text, TurnOutcome};
use super::super::types::SubagentType;
use super::super::{AgentLoop, AgentLoopConfig, AgentRunOutcome};
use super::mocks::{test_binding, MockLlmProvider, MockPrimitiveExecutor};
use crate::core::compaction::preheat::Preheat;
use crate::core::llm::{ChatMessage, MessageKind, StreamEvent};
use crate::core::plan_runtime::file_store::{
    plan_path_for_id, read_plan, write_plan, PlanFile, PlanFileFrontmatter, PlanFileState,
    TodoItem, TodoKind, TodoStatus, GATE_ACCEPTANCE_TODO_CONTENT, GATE_ACCEPTANCE_TODO_ID,
    GATE_CODE_REVIEW_TODO_CONTENT, GATE_CODE_REVIEW_TODO_ID,
};
use crate::core::plan_runtime::review::Finding;
use crate::core::plan_runtime::{NextAction, PlanRuntime};
use crate::core::session::manager::{CompactionResult, ContextState, MessageAppendSink};
use crate::core::tools::pipeline::read_state::{ReadFileState, ReadStamp};
use crate::infra::config::ContextConfig;
use crate::infra::error::AppError;
use crate::infra::event_bus::{DefaultEventBus, EventBus};
use crate::infra::wire;

/// 计划文件写在 `$HOME/.tomcat/plans` 下，和 plan_tool 那批测试共享同一个进程环境变量，
/// 所以必须用同一把锁串行化，否则会互相把 HOME 抽走。
fn home_guard() -> crate::test_support::TestLockGuard<'static> {
    crate::test_support::home_env_lock().lock().unwrap()
}

fn unique_plan_id(prefix: &str) -> String {
    format!(
        "{prefix}_{}_{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    )
}

fn write_plan_file(plan_id: &str, state: PlanFileState, todos: Vec<TodoItem>) -> PathBuf {
    let path = plan_path_for_id(plan_id).unwrap();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let plan = PlanFile {
        frontmatter: PlanFileFrontmatter {
            plan_id: plan_id.to_string(),
            goal: "test".to_string(),
            state,
            session_key: Some("sess-guard".to_string()),
            session_id: Some("sid-guard".to_string()),
            created_at: "2026-07-27T00:00:00Z".to_string(),
            schema_version: 1,
            todos,
            green_build_pass: false,
            green_build_evidence: Vec::new(),
            code_review_pass: false,
            code_review_pass_at_ms: None,
            code_review_rounds: 0,
            code_review_baseline_ms: None,
            code_review_open_findings: Vec::new(),
            code_review_disputed_findings: Vec::new(),
            code_review_handoff: false,
            code_review_handoff_acknowledged: false,
            code_review_residual_findings: Vec::new(),
            completion_gate_cycles: 0,
            acceptance_commands: Vec::new(),
            unknown: serde_yaml::Mapping::new(),
        },
        body: "## body\n".to_string(),
    };
    write_plan(&path, &plan, 1_000).unwrap();
    path
}

fn cleanup_plan_file(path: &Path) {
    let _ = std::fs::remove_file(path);
    let lock = path.with_file_name(format!(
        "{}.lock",
        path.file_name().unwrap().to_string_lossy()
    ));
    let _ = std::fs::remove_file(lock);
}

fn todo(id: &str, status: TodoStatus) -> TodoItem {
    TodoItem {
        id: id.to_string(),
        content: format!("work on {id}"),
        status,
        evidence: Vec::new(),
        kind: Default::default(),
    }
}

fn empty_context_state() -> ContextState {
    ContextState {
        messages: vec![],
        estimate_context_chars: 0,
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
    }
}

#[derive(Default)]
struct RecordingMessageSink {
    messages: Mutex<Vec<serde_json::Value>>,
}

impl MessageAppendSink for RecordingMessageSink {
    fn append_message(&self, message: serde_json::Value) -> Result<String, AppError> {
        let mut messages = self.messages.lock().unwrap();
        messages.push(message);
        Ok(format!("persisted-{}", messages.len()))
    }

    fn append_message_with_id(
        &self,
        message: serde_json::Value,
        id: &str,
    ) -> Result<String, AppError> {
        self.messages.lock().unwrap().push(message);
        Ok(id.to_string())
    }

    fn append_custom_entry(&self, _extra: serde_json::Value) -> Result<(), AppError> {
        Ok(())
    }
}

fn build_agent(plan_runtime: Option<Arc<PlanRuntime>>, subagent_type: SubagentType) -> AgentLoop {
    build_agent_with_sink(plan_runtime, subagent_type, None)
}

fn build_agent_with_sink(
    plan_runtime: Option<Arc<PlanRuntime>>,
    subagent_type: SubagentType,
    message_append_sink: Option<Arc<dyn MessageAppendSink>>,
) -> AgentLoop {
    build_agent_with_config(
        plan_runtime,
        subagent_type,
        message_append_sink,
        ContextConfig::default(),
        Arc::new(ReadFileState::new()),
        String::new(),
    )
}

fn build_agent_with_config(
    plan_runtime: Option<Arc<PlanRuntime>>,
    subagent_type: SubagentType,
    message_append_sink: Option<Arc<dyn MessageAppendSink>>,
    context_config: ContextConfig,
    read_file_state: Arc<ReadFileState>,
    agent_trail_dir: String,
) -> AgentLoop {
    let mut agent = AgentLoop::new(
        test_binding(Arc::new(MockLlmProvider::new(vec![])), "gpt-4"),
        Arc::new(MockPrimitiveExecutor),
        Arc::new(DefaultEventBus::new()),
        AgentLoopConfig {
            session_id: "sess-guard".to_string(),
            plan_runtime,
            subagent_type,
            message_append_sink,
            context_config,
            read_file_state,
            agent_trail_dir,
            ..Default::default()
        },
        CancellationToken::new(),
    );
    agent.set_context_state(Some(empty_context_state()));
    agent
}

fn read_stamp(tool_call_id: &str) -> ReadStamp {
    ReadStamp {
        mtime_ms: 0,
        size: 0,
        content_hash: 0,
        offset: None,
        limit: None,
        is_partial_view: false,
        render_mode: crate::core::tools::pipeline::read_state::ReadRenderMode::Plain,
        covered_lines: Some((1, 1)),
        reached_eof: true,
        tool_call_id: Some(tool_call_id.to_string()),
    }
}

fn user_message_with_id(id: &str, content: &str) -> ChatMessage {
    let mut message = ChatMessage::user(content);
    message.msg_id = Some(id.to_string());
    message
}

async fn finalize(agent: &mut AgentLoop, messages: &mut Vec<ChatMessage>) -> TurnOutcome {
    finalize_turn_after_text(
        agent,
        messages,
        "I have finished M1, let me know what to do next.",
        0,
        Some("stop".to_string()),
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .expect("finalize should not fail")
}

#[tokio::test]
async fn layer0_does_not_rewrite_history_without_a_boundary_switch() {
    let trail = tempfile::tempdir().expect("trail directory");
    let config = ContextConfig {
        keep_recent_turns: 1,
        layer0_placeholder_threshold_chars: 10_000,
        layer0_single_result_max_chars: 50_000,
        ..Default::default()
    };
    let mut agent = build_agent_with_config(
        None,
        SubagentType::User,
        None,
        config,
        Arc::new(ReadFileState::new()),
        trail.path().display().to_string(),
    );
    let old_tool = ChatMessage::tool("old-12k", &"old".repeat(4_000));
    let newest_tool = ChatMessage::tool("new-60k", &"new".repeat(20_000));
    let state_messages = vec![
        ChatMessage::user("older turn"),
        old_tool,
        ChatMessage::user("latest turn"),
        newest_tool,
    ];
    let estimate_context_chars = state_messages
        .iter()
        .map(crate::core::session::manager::estimate_msg_chars)
        .sum();
    agent.set_context_state(Some(ContextState {
        messages: state_messages.clone(),
        estimate_context_chars,
        context_budget_chars: 80_000,
        context_budget_tokens: 20_000,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        latest_plan_event: None,
        resume_control: Default::default(),
        preheat: Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    }));
    let mut messages = state_messages;

    assert_eq!(
        finalize(&mut agent, &mut messages).await,
        TurnOutcome::Finished
    );

    let state = agent.context_state.as_ref().expect("context state");
    assert!(
        state.messages[1]
            .text_content()
            .is_some_and(|content| content.len() > 10_000),
        "the old 12K tool result must remain intact until a boundary applies"
    );
    assert_eq!(
        state.messages[3].text_content().map(str::len),
        Some(60_000),
        "the newest 60K tool result must remain in context without a boundary"
    );
    assert!(
        !trail
            .path()
            .join("tool-results/sess-guard/new-60k.txt")
            .exists(),
        "L0 must not persist a result without a successful boundary switch"
    );
}

#[tokio::test]
async fn successful_boundary_switch_runs_both_layer0_cleanup_steps() {
    let trail = tempfile::tempdir().expect("trail directory");
    let config = ContextConfig {
        keep_recent_turns: 1,
        layer0_placeholder_threshold_chars: 10_000,
        layer0_single_result_max_chars: 50_000,
        ..Default::default()
    };
    let mut agent = build_agent_with_config(
        None,
        SubagentType::User,
        None,
        config,
        Arc::new(ReadFileState::new()),
        trail.path().display().to_string(),
    );
    let state_messages = vec![
        user_message_with_id("start", "covered start"),
        user_message_with_id("covered-end", "covered end"),
        user_message_with_id("surviving-old-turn", "old result remains after summary"),
        ChatMessage::tool("old-12k", &"old".repeat(4_000)),
        user_message_with_id("last-turn", "keep this final turn"),
        ChatMessage::tool("new-60k", &"new".repeat(20_000)),
    ];
    let estimate_context_chars = state_messages
        .iter()
        .map(crate::core::session::manager::estimate_msg_chars)
        .sum();
    let mut preheat = Preheat::new();
    preheat.restore_completed(CompactionResult {
        summary_text: "summary".into(),
        covered_start_id: "start".into(),
        covered_end_id: "covered-end".into(),
        covered_count: 2,
        transcript_compaction_entry_id: None,
        estimated_covered_tokens_before: Some(10),
        estimated_summary_tokens: Some(2),
        estimated_tokens_saved: Some(8),
        preheat_elapsed_ms: 0,
    });
    agent.start_idx = state_messages.len();
    agent.context_tail_start = state_messages.len();
    agent.set_context_state(Some(ContextState {
        messages: state_messages.clone(),
        estimate_context_chars,
        context_budget_chars: 40_000,
        context_budget_tokens: 10_000,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        latest_plan_event: None,
        resume_control: Default::default(),
        preheat,
        session_obs: Default::default(),
        live: Default::default(),
    }));
    let mut messages = state_messages;

    assert_eq!(
        finalize(&mut agent, &mut messages).await,
        TurnOutcome::Finished
    );

    assert_eq!(
        messages[2].text_content(),
        Some(crate::core::compaction::TOOL_RESULT_PLACEHOLDER),
        "a successful boundary makes the surviving old turn eligible for L0-B"
    );
    let persisted = messages[4].text_content().unwrap_or("");
    assert!(
        persisted.starts_with("[Tool result persisted:"),
        "the latest historical tool result must use L0-A after boundary: {persisted}"
    );
    assert!(
        trail
            .path()
            .join("tool-results/sess-guard/new-60k.txt")
            .exists(),
        "L0-A must retain the original large result on disk"
    );
}

#[tokio::test]
async fn timing5_boundary_cleanup_matches_reference_and_emits_one_release() {
    const FINAL_TEXT: &str = "I have finished M1, let me know what to do next.";

    let trail = tempfile::tempdir().expect("trail directory");
    let config = ContextConfig {
        keep_recent_turns: 1,
        layer0_placeholder_threshold_chars: 10_000,
        layer0_single_result_max_chars: 50_000,
        ..Default::default()
    };
    let state_messages = vec![
        user_message_with_id("start", "covered start"),
        user_message_with_id("covered-end", "covered end"),
        user_message_with_id("surviving-old-turn", "old result remains after summary"),
        ChatMessage::tool("old-12k", &"old".repeat(4_000)),
        user_message_with_id("last-turn", "keep this final turn"),
        ChatMessage::tool("new-60k", &"new".repeat(20_000)),
    ];
    let estimate_context_chars = state_messages
        .iter()
        .map(crate::core::session::manager::estimate_msg_chars)
        .sum();
    let compaction_result = CompactionResult {
        summary_text: "summary".into(),
        covered_start_id: "start".into(),
        covered_end_id: "covered-end".into(),
        covered_count: 2,
        transcript_compaction_entry_id: None,
        estimated_covered_tokens_before: Some(10),
        estimated_summary_tokens: Some(2),
        estimated_tokens_saved: Some(8),
        preheat_elapsed_ms: 0,
    };

    let event_bus = Arc::new(DefaultEventBus::new());
    let release_events = Arc::new(AtomicUsize::new(0));
    let release_events_cb = Arc::clone(&release_events);
    event_bus.on(
        wire::WIRE_LAYER0_CONTEXT_RELEASE,
        Box::new(move |_| {
            release_events_cb.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }),
    );
    let mut agent = AgentLoop::new(
        test_binding(Arc::new(MockLlmProvider::new(vec![])), "gpt-4"),
        Arc::new(MockPrimitiveExecutor),
        event_bus,
        AgentLoopConfig {
            session_id: "sess-guard".to_string(),
            subagent_type: SubagentType::User,
            context_config: config,
            read_file_state: Arc::new(ReadFileState::new()),
            agent_trail_dir: trail.path().display().to_string(),
            ..Default::default()
        },
        CancellationToken::new(),
    );
    let mut preheat = Preheat::new();
    preheat.restore_completed(compaction_result);
    // This fixture models a boundary that covered already-completed history. The final assistant
    // reply appended by `finalize` is the only active-tail message.
    agent.start_idx = state_messages.len();
    agent.context_tail_start = state_messages.len();
    agent.set_context_state(Some(ContextState {
        messages: state_messages.clone(),
        estimate_context_chars,
        context_budget_chars: 40_000,
        context_budget_tokens: 10_000,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        latest_plan_event: None,
        resume_control: Default::default(),
        preheat,
        session_obs: Default::default(),
        live: Default::default(),
    }));
    let mut messages = state_messages;

    assert_eq!(
        finalize(&mut agent, &mut messages).await,
        TurnOutcome::Finished
    );

    assert_eq!(
        messages[0].kind,
        MessageKind::CompactionSummary,
        "the completed prefix must become a summary in the next request payload"
    );
    assert_eq!(
        messages[2].text_content(),
        Some("[Previous tool result replaced to save context space]"),
        "Layer 0 placeholder cleanup must rewrite historical tools"
    );
    assert!(
        messages[4]
            .text_content()
            .is_some_and(|text| text.starts_with("[Tool result persisted:")),
        "Layer 0 persistence must rewrite the latest historical tool result"
    );
    assert_eq!(messages[agent.start_idx].text_content(), Some(FINAL_TEXT));
    assert_eq!(
        release_events.load(Ordering::SeqCst),
        1,
        "moving L0 into apply must not retain a second timing⑤ release emitter"
    );
}

#[tokio::test]
async fn timing5_applies_preheat_whose_anchor_is_in_the_current_turn() {
    let switched = Arc::new(AtomicUsize::new(0));
    let errors = Arc::new(AtomicUsize::new(0));
    let event_bus = Arc::new(DefaultEventBus::new());
    let switched_cb = Arc::clone(&switched);
    event_bus.on(
        wire::WIRE_BOUNDARY_SWITCHED,
        Box::new(move |_| {
            switched_cb.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }),
    );
    let errors_cb = Arc::clone(&errors);
    event_bus.on(
        wire::WIRE_COMPACTION_ERROR,
        Box::new(move |_| {
            errors_cb.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }),
    );

    let sink = Arc::new(RecordingMessageSink::default());
    let mut historical = user_message_with_id("historical", &"h".repeat(900));
    historical.timestamp = Some("2026-09-14T00:00:00Z".to_string());
    let mut current = user_message_with_id("current", &"c".repeat(900));
    current.timestamp = Some("2026-09-14T00:00:01Z".to_string());
    let mut state = empty_context_state();
    state.messages = vec![historical.clone()];
    state.estimate_context_chars = 1_800;
    state.context_budget_chars = 2_000;
    state.context_budget_tokens = 500;
    state.preheat.restore_completed(CompactionResult {
        summary_text: "summary".to_string(),
        covered_start_id: "historical".to_string(),
        covered_end_id: "current".to_string(),
        covered_count: 2,
        transcript_compaction_entry_id: Some("cmp-historical-current".to_string()),
        estimated_covered_tokens_before: None,
        estimated_summary_tokens: None,
        estimated_tokens_saved: None,
        preheat_elapsed_ms: 0,
    });

    let mut agent = AgentLoop::new(
        test_binding(Arc::new(MockLlmProvider::new(vec![])), "gpt-4"),
        Arc::new(MockPrimitiveExecutor),
        Arc::clone(&event_bus) as Arc<dyn EventBus>,
        AgentLoopConfig {
            session_id: "sess-timing5-current-anchor".to_string(),
            message_append_sink: Some(sink.clone()),
            ..Default::default()
        },
        CancellationToken::new(),
    );
    agent.start_idx = 2;
    agent.context_tail_start = 2;
    agent.set_context_state(Some(state));
    let mut messages = vec![ChatMessage::system("sys"), historical, current];

    assert_eq!(
        finalize_turn_after_text(
            &mut agent,
            &mut messages,
            "final response",
            1,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap(),
        TurnOutcome::Finished
    );

    assert_eq!(switched.load(Ordering::SeqCst), 1);
    assert_eq!(errors.load(Ordering::SeqCst), 0);
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[1].kind, MessageKind::CompactionSummary);
    assert_eq!(messages[2].text_content(), Some("final response"));
    assert_eq!(agent.start_idx, 2);
    assert_eq!(
        messages[agent.start_idx..].len(),
        1,
        "the returned new-message tail must exclude the summary already represented in history"
    );
    let persisted = sink.messages.lock().unwrap();
    assert_eq!(
        persisted.len(),
        1,
        "only the final surviving tail message is appended; the summary is not duplicated"
    );
    assert_eq!(
        persisted[0]
            .get("content")
            .and_then(serde_json::Value::as_str),
        Some("final response")
    );
}

#[tokio::test]
async fn layer0_without_boundary_keeps_read_stamp_and_tool_result_intact() {
    let read_state = Arc::new(ReadFileState::new());
    let stamp_path = PathBuf::from("/layer0-low-watermark-stamp");
    read_state.put(stamp_path.clone(), read_stamp("old-12k"));
    let config = ContextConfig {
        keep_recent_turns: 1,
        layer0_placeholder_threshold_chars: 10_000,
        ..Default::default()
    };
    let mut agent = build_agent_with_config(
        None,
        SubagentType::User,
        None,
        config,
        Arc::clone(&read_state),
        String::new(),
    );
    let old_tool_text = "old".repeat(4_000);
    let state_messages = vec![
        ChatMessage::user("older turn"),
        ChatMessage::tool("old-12k", &old_tool_text),
        ChatMessage::user("latest turn"),
        ChatMessage::tool("latest-small", "small result"),
    ];
    agent.set_context_state(Some(ContextState {
        messages: state_messages.clone(),
        estimate_context_chars: 40_000,
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
    }));
    let mut messages = state_messages;

    assert_eq!(
        finalize(&mut agent, &mut messages).await,
        TurnOutcome::Finished
    );

    let state = agent.context_state.as_ref().expect("context state");
    assert_eq!(
        state.messages[1].text_content(),
        Some(old_tool_text.as_str()),
        "without a successful boundary, Layer 0 must not rewrite old results"
    );
    assert!(
        read_state.get(&stamp_path).is_some(),
        "a read stamp may only be invalidated after its visible result is evicted"
    );
}

#[tokio::test]
async fn guard_blocks_handback_while_todos_remain() {
    let _home = home_guard();
    let plan_id = unique_plan_id("guard_remaining");
    let plan_path = write_plan_file(
        &plan_id,
        PlanFileState::Executing,
        vec![
            todo("t1", TodoStatus::Completed),
            todo("t2", TodoStatus::InProgress),
            todo("t3", TodoStatus::Pending),
        ],
    );
    let plan_runtime = PlanRuntime::new("sess-guard");
    plan_runtime.seed_active_plan_for_test(plan_id.clone(), PlanFileState::Executing);

    let mut agent = build_agent(Some(Arc::clone(&plan_runtime)), SubagentType::User);
    let mut messages = vec![ChatMessage::user("start building")];
    let outcome = finalize(&mut agent, &mut messages).await;

    assert_eq!(outcome, TurnOutcome::Continue, "计划没收口不得结束回合");
    let injected = messages.last().unwrap();
    assert_eq!(injected.kind, MessageKind::Nudge);
    let text = injected.text_content().unwrap_or("");
    assert!(text.contains("remaining work todos"), "text={text}");
    assert!(text.contains("t2 (in_progress)"), "text={text}");
    assert!(text.contains("t3 (pending)"), "text={text}");

    cleanup_plan_file(&plan_path);
}

#[tokio::test]
async fn guard_persists_nudge_with_its_distinct_kind() {
    let _home = home_guard();
    let plan_id = unique_plan_id("guard_persisted_nudge");
    let plan_path = write_plan_file(
        &plan_id,
        PlanFileState::Executing,
        vec![todo("t1", TodoStatus::Pending)],
    );
    let plan_runtime = PlanRuntime::new("sess-guard");
    plan_runtime.seed_active_plan_for_test(plan_id, PlanFileState::Executing);
    let sink = Arc::new(RecordingMessageSink::default());
    let sink_for_agent: Arc<dyn MessageAppendSink> = sink.clone();

    let mut agent =
        build_agent_with_sink(Some(plan_runtime), SubagentType::User, Some(sink_for_agent));
    let mut messages = vec![ChatMessage::user("start building")];
    assert_eq!(
        finalize(&mut agent, &mut messages).await,
        TurnOutcome::Continue
    );

    let persisted = sink.messages.lock().unwrap();
    assert_eq!(persisted.len(), 2, "assistant reply and nudge both persist");
    assert_eq!(persisted[1]["role"], "user");
    assert_eq!(persisted[1]["kind"], "nudge");
    assert_eq!(
        messages
            .last()
            .and_then(|message| message.msg_id.as_deref()),
        Some("persisted-2"),
        "the in-memory nudge must use its transcript row id"
    );

    cleanup_plan_file(&plan_path);
}

#[tokio::test]
async fn guard_blocks_handback_when_todos_done_but_review_pushed_back() {
    // 41 项 todo 全勾完、计划文件仍是 executing —— 只可能是 code review 没过。
    let _home = home_guard();
    let plan_id = unique_plan_id("guard_review");
    let plan_path = write_plan_file(
        &plan_id,
        PlanFileState::Executing,
        vec![
            todo("t1", TodoStatus::Completed),
            todo("t2", TodoStatus::Completed),
        ],
    );
    let plan_runtime = PlanRuntime::new("sess-guard");
    plan_runtime.seed_active_plan_for_test(plan_id.clone(), PlanFileState::Executing);
    let findings = vec![
        Finding::new(
            "concern".into(),
            "logic".into(),
            "missing null check".into(),
        )
        .with_reference("F01"),
        Finding::new("nit".into(), "tests".into(), "no regression test".into())
            .with_reference("F02"),
    ];
    let mut plan = read_plan(&plan_path).unwrap();
    plan.frontmatter.code_review_open_findings = findings.clone();
    write_plan(&plan_path, &plan, 1_000).unwrap();

    let mut agent = build_agent(Some(plan_runtime), SubagentType::User);
    let mut messages = vec![ChatMessage::user("start building")];
    let outcome = finalize(&mut agent, &mut messages).await;

    assert_eq!(outcome, TurnOutcome::Continue);
    let text = messages.last().unwrap().text_content().unwrap_or("");
    assert!(
        text.contains("unresolved code-review findings"),
        "text={text}"
    );
    assert!(
        text.contains("F01 [concern] logic: missing null check"),
        "text={text}"
    );
    assert!(
        text.contains("F02 [nit] tests: no regression test"),
        "text={text}"
    );

    cleanup_plan_file(&plan_path);
}

#[tokio::test]
async fn exhausted_budget_keeps_the_agent_working_until_acceptance_can_start() {
    let _home = home_guard();
    let plan_id = unique_plan_id("guard_review_budget_exhausted");
    let plan_path = write_plan_file(
        &plan_id,
        PlanFileState::Executing,
        vec![
            todo("t1", TodoStatus::Completed),
            TodoItem {
                id: GATE_CODE_REVIEW_TODO_ID.into(),
                content: GATE_CODE_REVIEW_TODO_CONTENT.into(),
                status: TodoStatus::Completed,
                evidence: Vec::new(),
                kind: TodoKind::GateCodeReview,
            },
            TodoItem {
                id: GATE_ACCEPTANCE_TODO_ID.into(),
                content: GATE_ACCEPTANCE_TODO_CONTENT.into(),
                status: TodoStatus::Pending,
                evidence: Vec::new(),
                kind: TodoKind::GateAcceptance,
            },
        ],
    );
    let plan_runtime = PlanRuntime::new("sess-guard");
    plan_runtime.set_max_code_review_rounds(1);
    let mut plan = read_plan(&plan_path).unwrap();
    plan.frontmatter.code_review_rounds = 1;
    plan.frontmatter.code_review_pass = true;
    plan.frontmatter.code_review_residual_findings =
        vec!["F01 [P1] logic: missing null check".into()];
    plan.frontmatter.code_review_open_findings = vec![Finding::new(
        "concern".into(),
        "logic".into(),
        "missing null check".into(),
    )];
    write_plan(&plan_path, &plan, 1_000).unwrap();
    plan_runtime.bind_plan_file_for_test(plan_path.clone());

    let mut agent = build_agent(Some(Arc::clone(&plan_runtime)), SubagentType::User);
    let mut messages = vec![ChatMessage::user("start building")];
    assert_eq!(
        finalize(&mut agent, &mut messages).await,
        TurnOutcome::Continue,
        "预算耗尽前尚有 finding 时，必须引导模型修复并触发 acceptance"
    );
    let text = messages
        .last()
        .and_then(|message| message.text_content())
        .unwrap_or("");
    assert!(text.contains("configured round budget"), "text={text}");
    assert!(
        text.contains("run every declared `acceptance_commands` entry"),
        "text={text}"
    );
    let current_plan = read_plan(&plan_path).unwrap();
    let instruction = plan_runtime
        .next_action(&current_plan.frontmatter)
        .await
        .instruction();
    assert_eq!(
        text,
        format!("Plan `{plan_id}`: {instruction} Do not summarize or hand back.")
    );
    assert!(
        messages
            .iter()
            .any(|message| message.kind == MessageKind::Nudge),
        "预算耗尽但尚未放行时，必须追加收口引导"
    );

    cleanup_plan_file(&plan_path);
}

#[tokio::test]
async fn exhausted_infra_retries_stop_completion_guard() {
    let _home = home_guard();
    let plan_id = unique_plan_id("guard_review_infra_retries_exhausted");
    let plan_path = write_plan_file(
        &plan_id,
        PlanFileState::Executing,
        vec![todo("t1", TodoStatus::Completed)],
    );
    let plan_runtime = PlanRuntime::new("sess-guard");
    plan_runtime.seed_active_plan_for_test(plan_id.clone(), PlanFileState::Executing);
    plan_runtime.bind_plan_file_for_test(plan_path.clone());
    for _ in 0..3 {
        plan_runtime.bump_review_infra_retry(&plan_id);
    }
    assert!(
        plan_runtime.code_review_infra_retry_exhausted(&plan_id),
        "three technical review failures must exhaust the bounded retry budget"
    );

    let mut agent = build_agent(Some(plan_runtime), SubagentType::User);
    let mut messages = vec![ChatMessage::user("start building")];
    assert_eq!(
        finalize(&mut agent, &mut messages).await,
        TurnOutcome::Finished,
        "exhausted infrastructure retries are a handoff state, not another completion nudge"
    );
    assert!(
        messages
            .iter()
            .all(|message| message.kind != MessageKind::Nudge),
        "exhausted infrastructure retries must not append a completion nudge"
    );

    cleanup_plan_file(&plan_path);
}

#[tokio::test]
async fn guard_stops_after_two_zero_progress_nudges_and_hands_back() {
    let _home = home_guard();
    let plan_id = unique_plan_id("guard_cap");
    let plan_path = write_plan_file(
        &plan_id,
        PlanFileState::Executing,
        vec![todo("t1", TodoStatus::Pending)],
    );
    let plan_runtime = PlanRuntime::new("sess-guard");
    plan_runtime.seed_active_plan_for_test(plan_id.clone(), PlanFileState::Executing);
    plan_runtime.bind_plan_file_for_test(plan_path.clone());
    let stalled_events = Arc::new(Mutex::new(Vec::new()));
    {
        let stalled_events = Arc::clone(&stalled_events);
        plan_runtime.attach_transcript_appender(Arc::new(move |event| {
            stalled_events.lock().unwrap().push(event);
            Ok(())
        }));
    }

    let mut agent = build_agent(Some(plan_runtime), SubagentType::User);
    let mut messages = vec![ChatMessage::user("start building")];
    for round in 0..2 {
        assert_eq!(
            finalize(&mut agent, &mut messages).await,
            TurnOutcome::Continue,
            "第 {round} 轮仍应注入"
        );
    }
    assert_eq!(
        finalize(&mut agent, &mut messages).await,
        TurnOutcome::Finished,
        "连续两次零进度 nudge 后必须交还用户，不能无限打转"
    );
    let stalled_events = stalled_events.lock().unwrap();
    assert_eq!(stalled_events.len(), 1);
    assert_eq!(stalled_events[0]["event"], "plan.completion_guard.stalled");
    assert_eq!(stalled_events[0]["plan_id"], plan_id);
    assert_eq!(stalled_events[0]["idle_nudges"], 2);
    assert_eq!(stalled_events[0]["phase"], "continue_work");
    assert_eq!(stalled_events[0]["open_findings_count"], 0);

    cleanup_plan_file(&plan_path);
}

#[tokio::test]
async fn guard_progress_resets_the_zero_progress_nudge_counter() {
    let _home = home_guard();
    let plan_id = unique_plan_id("guard_progress_resets");
    let plan_path = write_plan_file(
        &plan_id,
        PlanFileState::Executing,
        vec![todo("t1", TodoStatus::Pending)],
    );
    let plan_runtime = PlanRuntime::new("sess-guard");
    plan_runtime.bind_plan_file_for_test(plan_path.clone());

    let action = NextAction::ContinueWork {
        remaining_work: vec!["- t1 (pending)".into()],
    };
    assert!(
        !plan_runtime
            .note_completion_guard_nudge(&plan_id, &action)
            .await
    );
    assert!(
        !plan_runtime
            .note_completion_guard_nudge(&plan_id, &action)
            .await
    );

    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let mut plan = read_plan(&plan_path).unwrap();
    plan.body.push_str("progress was recorded\n");
    write_plan(&plan_path, &plan, 1_000).unwrap();

    assert!(
        !plan_runtime
            .note_completion_guard_nudge(&plan_id, &action)
            .await,
        "a plan write between nudges must reset the idle counter"
    );
    assert!(
        !plan_runtime
            .note_completion_guard_nudge(&plan_id, &action)
            .await
    );
    assert!(
        plan_runtime
            .note_completion_guard_nudge(&plan_id, &action)
            .await,
        "only two further no-progress nudges may stall the plan"
    );

    cleanup_plan_file(&plan_path);
}

#[tokio::test]
async fn guard_does_not_fire_once_the_plan_file_leaves_executing() {
    let _home = home_guard();
    let plan_id = unique_plan_id("guard_completed");
    let plan_path = write_plan_file(
        &plan_id,
        PlanFileState::Completed,
        vec![todo("t1", TodoStatus::Completed)],
    );
    let plan_runtime = PlanRuntime::new("sess-guard");
    plan_runtime.seed_active_plan_for_test(plan_id.clone(), PlanFileState::Executing);
    plan_runtime.bind_plan_file_for_test(plan_path.clone());

    let mut agent = build_agent(Some(plan_runtime), SubagentType::User);
    let mut messages = vec![ChatMessage::user("start building")];
    assert_eq!(
        finalize(&mut agent, &mut messages).await,
        TurnOutcome::Finished
    );

    cleanup_plan_file(&plan_path);
}

#[tokio::test]
async fn guard_never_fires_outside_exec() {
    let mut agent = build_agent(Some(PlanRuntime::new("sess-guard")), SubagentType::User);
    let mut messages = vec![ChatMessage::user("just chatting")];
    assert_eq!(
        finalize(&mut agent, &mut messages).await,
        TurnOutcome::Finished,
        "CHAT 模式不注入"
    );

    let planning = PlanRuntime::new("sess-guard");
    planning.enter_plan().unwrap();
    let mut agent = build_agent(Some(planning), SubagentType::User);
    let mut messages = vec![ChatMessage::user("write me a plan")];
    assert_eq!(
        finalize(&mut agent, &mut messages).await,
        TurnOutcome::Finished,
        "PLAN 模式不注入"
    );
}

#[tokio::test]
async fn guard_never_fires_for_non_root_subagents() {
    let _home = home_guard();
    let plan_id = unique_plan_id("guard_non_root");
    let plan_path = write_plan_file(
        &plan_id,
        PlanFileState::Executing,
        vec![todo("t1", TodoStatus::Pending)],
    );
    let plan_runtime = PlanRuntime::new("sess-guard");
    plan_runtime.seed_active_plan_for_test(plan_id.clone(), PlanFileState::Executing);

    for subagent_type in [
        SubagentType::PlanReviewer,
        SubagentType::CodeReviewer,
        SubagentType::Verifier,
        SubagentType::Explorer,
    ] {
        let mut agent = build_agent(Some(Arc::clone(&plan_runtime)), subagent_type);
        let mut messages = vec![ChatMessage::user("start building")];
        assert_eq!(
            finalize(&mut agent, &mut messages).await,
            TurnOutcome::Finished,
            "{:?} 不应触发 completion guard",
            subagent_type
        );
    }

    cleanup_plan_file(&plan_path);
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

fn tool_stream(id: &str) -> Vec<Result<StreamEvent, AppError>> {
    vec![
        Ok(StreamEvent::ToolCallDelta {
            index: 0,
            id: Some(id.to_string()),
            name: Some("read".to_string()),
            arguments_delta: Some(r#"{"path":"/tmp/x"}"#.to_string()),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "tool_calls".to_string(),
        }),
    ]
}

#[tokio::test]
async fn guard_cap_survives_tool_rounds_between_text_turns() {
    let _home = home_guard();
    let plan_id = unique_plan_id("guard_cap_tool_rounds");
    let plan_path = write_plan_file(
        &plan_id,
        PlanFileState::Executing,
        vec![todo("t1", TodoStatus::Pending)],
    );
    let plan_runtime = PlanRuntime::new("sess-guard");
    plan_runtime.seed_active_plan_for_test(plan_id.clone(), PlanFileState::Executing);

    let mut streams = Vec::new();
    for idx in 0..2 {
        streams.push(text_stream(&format!("guarded text {idx}")));
        streams.push(tool_stream(&format!("call_{idx}")));
    }
    streams.push(text_stream("final handback"));

    let llm = Arc::new(MockLlmProvider::new(streams));
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());
    let config = AgentLoopConfig {
        session_id: "sess-guard".to_string(),
        plan_runtime: Some(plan_runtime),
        subagent_type: SubagentType::User,
        ..Default::default()
    };
    let mut agent = AgentLoop::new(
        test_binding(llm, "gpt-4"),
        primitive,
        event_bus,
        config,
        CancellationToken::new(),
    );
    agent.set_context_state(Some(empty_context_state()));

    let outcome = agent.run(vec![ChatMessage::user("do work")]).await;
    let AgentRunOutcome::Completed(result) = outcome else {
        panic!("guard should hand back after hitting cap");
    };
    assert!(
        result.final_text.ends_with("final handback"),
        "unexpected final_text: {}",
        result.final_text
    );

    cleanup_plan_file(&plan_path);
}
