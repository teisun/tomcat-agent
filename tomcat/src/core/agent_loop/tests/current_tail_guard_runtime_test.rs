use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serial_test::serial;
use tokio_util::sync::CancellationToken;

use super::super::current_tail_guard;
use super::super::types::SubagentType;
use super::super::{AgentLoop, AgentLoopConfig};
use super::mocks::{test_binding, MockPrimitiveExecutor};
use crate::core::compaction::preheat::Preheat;
use crate::core::compaction::TOOL_RESULT_PLACEHOLDER;
use crate::core::llm::{
    ChatMessage, ChatRequest, ChatResponse, ChatResponseChoice, LlmProvider, MessageKind,
    StreamEvent,
};
use crate::core::plan_runtime::file_store::{
    plan_path_for_id, write_plan, PlanFile, PlanFileFrontmatter, PlanFileState, TodoItem,
    TodoStatus,
};
use crate::core::plan_runtime::PlanRuntime;
use crate::core::session::manager::{
    estimate_msg_chars, ApiUsage, ContextState, PlanEventKind, PlanEventRef,
};
use crate::core::session::transcript::{
    append_entry, read_entries_tail, MessageEntry, ToolResultsCompactedEntry, TranscriptEntry,
};
use crate::infra::config::ContextConfig;
use crate::infra::error::AppError;
use crate::infra::event_bus::DefaultEventBus;
use crate::{init_context_state, SessionManager};

struct ChatOnlyMockLlm {
    summary_text: String,
}

#[async_trait]
impl LlmProvider for ChatOnlyMockLlm {
    fn provider_name(&self) -> &str {
        "chat_only_mock"
    }

    async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, AppError> {
        Ok(ChatResponse {
            id: None,
            choices: vec![ChatResponseChoice {
                index: 0,
                message: ChatMessage::assistant(&self.summary_text),
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

#[tokio::test]
async fn mid_turn_guard_reduced_tail_is_recomputed_after_reload() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = SessionManager::new(dir.path().to_path_buf());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();
    let transcript = mgr.current_transcript_path().unwrap().unwrap();

    let system = ChatMessage::system("sys");
    let mut user = ChatMessage::user("read everything");
    user.msg_id = Some("u1".to_string());
    let mut assistant = assistant_with_tool_calls(&[
        ("tc1", "read"),
        ("tc2", "read"),
        ("tc3", "read"),
        ("tc4", "read"),
        ("tc5", "read"),
    ]);
    assistant.msg_id = Some("a1".to_string());
    let mut tools = vec![
        tool_message("tr1", "tc1", &"x".repeat(12_000)),
        tool_message("tr2", "tc2", &"y".repeat(8_000)),
        tool_message("tr3", "tc3", &"z".repeat(8_000)),
        tool_message("tr4", "tc4", &"p".repeat(8_000)),
        tool_message("tr5", "tc5", &"q".repeat(8_000)),
    ];
    append_transcript_message(&transcript, &user);
    append_transcript_message(&transcript, &assistant);
    for tool in &tools {
        append_transcript_message(&transcript, tool);
    }

    let mut messages = vec![system, user, assistant];
    messages.append(&mut tools);
    let tail_chars: usize = messages.iter().skip(1).map(estimate_msg_chars).sum();

    let config = ContextConfig {
        current_tail_compactable_min_chars: 1,
        current_tail_single_result_max_chars: 10_000,
        ..Default::default()
    };
    let mut agent = AgentLoop::new(
        test_binding(
            Arc::new(ChatOnlyMockLlm {
                summary_text: "unused".to_string(),
            }),
            "gpt-4",
        ),
        Arc::new(MockPrimitiveExecutor),
        Arc::new(DefaultEventBus::new()),
        AgentLoopConfig {
            session_id: "sess-mid-turn-reload".to_string(),
            agent_trail_dir: dir.path().to_string_lossy().to_string(),
            context_config: config.clone(),
            ..Default::default()
        },
        CancellationToken::new(),
    );
    agent.start_idx = 1;
    agent.context_tail_start = 1;
    agent.set_context_state(Some(ContextState {
        messages: vec![],
        estimate_context_chars: tail_chars,
        context_budget_chars: 20_000,
        context_budget_tokens: 5_000,
        last_api_usage: Some(ApiUsage {
            prompt_tokens: 0,
            completion_tokens: 0,
        }),
        post_usage_appended_chars: tail_chars,
        transcript_path: transcript.clone(),
        latest_plan_event: None,
        resume_control: Default::default(),
        preheat: Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    }));

    current_tail_guard::maybe_reduce_before_next_llm(&mut agent, &mut messages)
        .await
        .unwrap();

    let persisted_entries = read_entries_tail(&transcript, 32).unwrap();
    assert!(
        persisted_entries.iter().any(|entry| {
            matches!(
                entry,
                TranscriptEntry::Message(message)
                    if message.id.as_deref() == Some("tr2")
                        && message.message["content"]
                            .as_str()
                            .is_some_and(|content| content == "y".repeat(8_000))
            )
        }),
        "placeholder compaction must retain the original JSONL result"
    );
    assert!(
        !persisted_entries
            .iter()
            .any(|entry| matches!(entry, TranscriptEntry::ToolResultsCompacted(_))),
        "placeholder compaction must not persist runtime-only markers"
    );
    let reloaded = init_context_state(&mgr, &config, "sys").unwrap();
    let texts: Vec<_> = reloaded
        .messages
        .iter()
        .filter_map(|msg| msg.text_content())
        .collect();
    assert!(texts
        .iter()
        .any(|text| text.starts_with("[Tool result persisted:")));
    assert!(
        texts.iter().any(|text| text == &"y".repeat(8_000)),
        "reload should restore raw results that were only placeholdered in memory"
    );
    assert!(
        !texts.contains(&TOOL_RESULT_PLACEHOLDER),
        "without a persisted marker, reload must not preserve runtime placeholders"
    );
}

#[test]
fn legacy_tool_results_compacted_marker_is_ignored_on_reload() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = SessionManager::new(dir.path().to_path_buf());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();
    let transcript = mgr.current_transcript_path().unwrap().unwrap();

    let mut user = ChatMessage::user("read the file");
    user.msg_id = Some("u1".to_string());
    let mut assistant = assistant_with_tool_calls(&[("tc1", "read")]);
    assistant.msg_id = Some("a1".to_string());
    let tool = tool_message("tr1", "tc1", &"raw result".repeat(2_000));
    append_transcript_message(&transcript, &user);
    append_transcript_message(&transcript, &assistant);
    append_transcript_message(&transcript, &tool);
    append_entry(
        &transcript,
        &TranscriptEntry::ToolResultsCompacted(ToolResultsCompactedEntry {
            id: Some("legacy-marker".to_string()),
            parent_id: None,
            timestamp: "2026-05-30T16:00:01Z".to_string(),
            message_ids: vec!["tr1".to_string()],
        }),
    )
    .unwrap();

    let state = init_context_state(&mgr, &ContextConfig::default(), "sys").unwrap();
    let texts: Vec<_> = state
        .messages
        .iter()
        .filter_map(|message| message.text_content())
        .collect();
    assert!(
        texts.iter().any(|text| *text == "raw result".repeat(2_000)),
        "legacy markers are accepted for transcript compatibility but ignored by hydrate"
    );
    assert!(!texts.contains(&TOOL_RESULT_PLACEHOLDER));
}

#[tokio::test]
async fn resume_without_marker_reduces_before_first_request() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = SessionManager::new(dir.path().to_path_buf());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();
    let transcript = mgr.current_transcript_path().unwrap().unwrap();

    let mut user = ChatMessage::user("continue the oversized resumed turn");
    user.msg_id = Some("u1".to_string());
    let mut assistant = assistant_with_tool_calls(&[("tc1", "write")]);
    assistant.msg_id = Some("a1".to_string());
    let tool = tool_message("tr1", "tc1", &"raw resumed output\n".repeat(1_000));
    append_transcript_message(&transcript, &user);
    append_transcript_message(&transcript, &assistant);
    append_transcript_message(&transcript, &tool);

    let config = ContextConfig {
        current_tail_compactable_min_chars: 1,
        ..Default::default()
    };
    let mut state = init_context_state(&mgr, &config, "sys").unwrap();
    state.context_budget_chars = 4_000;
    state.context_budget_tokens = 1_000;
    let mut messages = vec![ChatMessage::system("sys")];
    messages.extend(state.messages.clone());
    let mut agent = AgentLoop::new(
        test_binding(
            Arc::new(ChatOnlyMockLlm {
                summary_text: "collapsed resumed context".to_string(),
            }),
            "gpt-4",
        ),
        Arc::new(MockPrimitiveExecutor),
        Arc::new(DefaultEventBus::new()),
        AgentLoopConfig {
            session_id: "resume-no-marker".to_string(),
            agent_trail_dir: dir.path().to_string_lossy().to_string(),
            context_config: config,
            ..Default::default()
        },
        CancellationToken::new(),
    );
    agent.start_idx = 1;
    agent.context_tail_start = 1;
    agent.set_context_state(Some(state));

    current_tail_guard::maybe_reduce_before_next_llm(&mut agent, &mut messages)
        .await
        .unwrap();

    assert_eq!(messages[1].kind, MessageKind::CompactionSummary);
    assert!(
        !agent.context_state.as_ref().unwrap().is_over_budget(),
        "the raw transcript must be reduced before its first resumed request"
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn collapse_to_branch_summary_keeps_executing_snapshot() {
    let plan_id = unique_plan_id("exec_keepalive");
    let plan_path = write_plan_file(
        &plan_id,
        PlanFileState::Executing,
        vec![
            TodoItem {
                id: "t1".to_string(),
                content: "step pending".to_string(),
                status: TodoStatus::Pending,
                evidence: Vec::new(),
                kind: Default::default(),
            },
            TodoItem {
                id: "t2".to_string(),
                content: "step active".to_string(),
                status: TodoStatus::InProgress,
                evidence: Vec::new(),
                kind: Default::default(),
            },
        ],
    );

    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("collapse_exec.jsonl");
    crate::core::session::transcript::write_header(
        &transcript,
        &crate::core::session::transcript::SessionHeader {
            r#type: "session".to_string(),
            version: Some(1),
            id: "sid".to_string(),
            timestamp: "2026-05-30T16:00:00Z".to_string(),
            cwd: None,
            project_root: None,
        },
    )
    .unwrap();

    let plan_runtime = PlanRuntime::new("sess-plan-exec");
    plan_runtime.bind_plan_file_for_test(plan_path.clone());

    let system = ChatMessage::system("sys");
    let mut user = ChatMessage::user("u".repeat(4_000));
    user.msg_id = Some("u1".to_string());
    let mut assistant = ChatMessage::assistant("a".repeat(4_000));
    assistant.msg_id = Some("a1".to_string());
    append_transcript_message(&transcript, &user);
    append_transcript_message(&transcript, &assistant);

    let mut messages = vec![system, user, assistant];
    let tail_chars: usize = messages.iter().skip(1).map(estimate_msg_chars).sum();

    let mut agent = AgentLoop::new(
        test_binding(
            Arc::new(ChatOnlyMockLlm {
                summary_text: "continue with execution".to_string(),
            }),
            "gpt-4",
        ),
        Arc::new(MockPrimitiveExecutor),
        Arc::new(DefaultEventBus::new()),
        AgentLoopConfig {
            session_id: "sess-collapse-exec".to_string(),
            plan_runtime: Some(plan_runtime),
            ..Default::default()
        },
        CancellationToken::new(),
    );
    agent.start_idx = 1;
    agent.context_tail_start = 1;
    agent.set_context_state(Some(ContextState {
        messages: vec![],
        estimate_context_chars: tail_chars,
        context_budget_chars: 200,
        context_budget_tokens: 50,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: transcript.clone(),
        latest_plan_event: Some(PlanEventRef {
            kind: PlanEventKind::Build,
            plan_id: plan_id.clone(),
            path: plan_path.clone(),
        }),
        resume_control: Default::default(),
        preheat: Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    }));

    current_tail_guard::maybe_reduce_before_next_llm(&mut agent, &mut messages)
        .await
        .unwrap();

    let summary = &messages[1];
    let text = summary.text_content().unwrap_or("");
    assert_eq!(summary.kind, MessageKind::CompactionSummary);
    assert!(text.starts_with("<control_state>"));
    assert!(text.contains("mode: chat"));
    assert!(text.contains("plan_file_state: executing"));
    // Progress 由计划文件渲染，模型说什么都覆盖不掉。
    assert!(text.contains("step active"));
    assert!(text.contains("step pending"));

    cleanup_plan_file(&plan_path);
}

#[tokio::test]
#[serial(env_lock)]
async fn collapse_to_branch_summary_keeps_pending_snapshot_when_no_in_progress_exists() {
    let plan_id = unique_plan_id("pending_keepalive");
    let plan_path = write_plan_file(
        &plan_id,
        PlanFileState::Pending,
        vec![
            TodoItem {
                id: "t1".to_string(),
                content: "first pending".to_string(),
                status: TodoStatus::Pending,
                evidence: Vec::new(),
                kind: Default::default(),
            },
            TodoItem {
                id: "t2".to_string(),
                content: "second pending".to_string(),
                status: TodoStatus::Pending,
                evidence: Vec::new(),
                kind: Default::default(),
            },
        ],
    );

    let plan_runtime = PlanRuntime::new("sess-plan-pending");
    plan_runtime.bind_plan_file_for_test(plan_path.clone());

    let system = ChatMessage::system("sys");
    let user = ChatMessage::user("u".repeat(4_000));
    let assistant = ChatMessage::assistant("a".repeat(4_000));
    let mut messages = vec![system, user, assistant];
    let tail_chars: usize = messages.iter().skip(1).map(estimate_msg_chars).sum();

    let mut agent = AgentLoop::new(
        test_binding(
            Arc::new(ChatOnlyMockLlm {
                summary_text: "continue with pending work".to_string(),
            }),
            "gpt-4",
        ),
        Arc::new(MockPrimitiveExecutor),
        Arc::new(DefaultEventBus::new()),
        AgentLoopConfig {
            session_id: "sess-collapse-pending".to_string(),
            plan_runtime: Some(plan_runtime),
            ..Default::default()
        },
        CancellationToken::new(),
    );
    agent.start_idx = 1;
    agent.context_tail_start = 1;
    agent.set_context_state(Some(ContextState {
        messages: vec![],
        estimate_context_chars: tail_chars,
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

    current_tail_guard::maybe_reduce_before_next_llm(&mut agent, &mut messages)
        .await
        .unwrap();

    let text = messages[1].text_content().unwrap_or("");
    assert!(text.contains("mode: chat"));
    assert!(text.contains("plan_file_state: pending"));
    assert!(text.contains("first pending"));

    cleanup_plan_file(&plan_path);
}

#[tokio::test]
async fn collapse_to_branch_summary_omits_control_snapshot_for_child_agents_without_plan_runtime() {
    let system = ChatMessage::system("sys");
    let user = ChatMessage::user("u".repeat(4_000));
    let assistant = ChatMessage::assistant("a".repeat(4_000));
    let mut messages = vec![system, user, assistant];
    let tail_chars: usize = messages.iter().skip(1).map(estimate_msg_chars).sum();

    let mut agent = AgentLoop::new(
        test_binding(
            Arc::new(ChatOnlyMockLlm {
                summary_text: "child summary".to_string(),
            }),
            "gpt-4",
        ),
        Arc::new(MockPrimitiveExecutor),
        Arc::new(DefaultEventBus::new()),
        AgentLoopConfig {
            session_id: "sess-collapse-child".to_string(),
            subagent_type: SubagentType::CodeReviewer,
            plan_runtime: None,
            ..Default::default()
        },
        CancellationToken::new(),
    );
    agent.start_idx = 1;
    agent.context_tail_start = 1;
    agent.set_context_state(Some(ContextState {
        messages: vec![],
        estimate_context_chars: tail_chars,
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

    current_tail_guard::maybe_reduce_before_next_llm(&mut agent, &mut messages)
        .await
        .unwrap();

    let text = messages[1].text_content().unwrap_or("");
    assert_eq!(messages[1].kind, MessageKind::CompactionSummary);
    assert!(
        !text.starts_with("<control_state>"),
        "child agent should not inherit parent plan snapshot: {text}"
    );
    assert!(!text.contains("plan_file_state:"));
}

fn assistant_with_tool_calls(calls: &[(&str, &str)]) -> ChatMessage {
    let tool_calls: Vec<_> = calls
        .iter()
        .map(|(id, name)| {
            serde_json::json!({
                "id": id,
                "type": "function",
                "function": {"name": name, "arguments": "{}"},
            })
        })
        .collect();
    ChatMessage::assistant_with_tool_calls(Some("tools"), tool_calls)
}

fn tool_message(id: &str, tool_call_id: &str, text: &str) -> ChatMessage {
    let mut msg = ChatMessage::tool(tool_call_id, text);
    msg.msg_id = Some(id.to_string());
    msg
}

fn append_transcript_message(path: &Path, msg: &ChatMessage) {
    let mut payload = serde_json::json!({
        "role": match msg.role {
            crate::core::llm::ChatMessageRole::System => "system",
            crate::core::llm::ChatMessageRole::User => "user",
            crate::core::llm::ChatMessageRole::Assistant => "assistant",
            crate::core::llm::ChatMessageRole::Tool => "tool",
        },
        "content": msg.text_content().unwrap_or(""),
    });
    if let Some(tool_calls) = &msg.tool_calls {
        payload["tool_calls"] = serde_json::Value::Array(tool_calls.clone());
    }
    if let Some(tool_call_id) = &msg.tool_call_id {
        payload["tool_call_id"] = serde_json::json!(tool_call_id);
    }
    append_entry(
        path,
        &TranscriptEntry::Message(MessageEntry {
            id: msg.msg_id.clone(),
            parent_id: None,
            timestamp: "2026-05-30T16:00:00Z".to_string(),
            message: payload,
        }),
    )
    .unwrap();
}

fn unique_plan_id(prefix: &str) -> String {
    format!(
        "{prefix}_{}_{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis()
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
            session_key: Some("sess-test".to_string()),
            session_id: Some("sid-test".to_string()),
            created_at: "2026-05-31T00:00:00Z".to_string(),
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
