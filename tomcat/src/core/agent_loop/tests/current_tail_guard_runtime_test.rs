use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serial_test::serial;
use tokio_util::sync::CancellationToken;

use super::super::current_tail_guard;
use super::super::{AgentLoop, AgentLoopConfig};
use super::mocks::MockPrimitiveExecutor;
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
use crate::core::session::transcript::{append_entry, MessageEntry, TranscriptEntry};
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
async fn mid_turn_guard_reduced_tail_survives_reload() {
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
        Arc::new(ChatOnlyMockLlm {
            summary_text: "unused".to_string(),
        }),
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
        preheat: Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    }));

    current_tail_guard::maybe_reduce_before_next_llm(&mut agent, &mut messages)
        .await
        .unwrap();

    let reloaded = init_context_state(&mgr, &config, "sys").unwrap();
    let texts: Vec<_> = reloaded
        .messages
        .iter()
        .filter_map(|msg| msg.text_content())
        .collect();
    assert!(texts
        .iter()
        .any(|text| text.starts_with("[Tool result persisted:")));
    assert!(texts.contains(&TOOL_RESULT_PLACEHOLDER));
    assert!(
        !texts.iter().any(|text| text == &"x".repeat(12_000)),
        "reload should keep the rewritten preview instead of reviving the original tail"
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
            },
            TodoItem {
                id: "t2".to_string(),
                content: "step active".to_string(),
                status: TodoStatus::InProgress,
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
        },
    )
    .unwrap();

    let plan_runtime = PlanRuntime::new("sess-plan-exec");
    plan_runtime.set_executing_for_test(plan_id.clone());

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
        Arc::new(ChatOnlyMockLlm {
            summary_text: "continue with execution".to_string(),
        }),
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
    assert!(text.contains("- mode: executing"));
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
            },
            TodoItem {
                id: "t2".to_string(),
                content: "second pending".to_string(),
                status: TodoStatus::Pending,
            },
        ],
    );

    let plan_runtime = PlanRuntime::new("sess-plan-pending");
    plan_runtime.set_executing_for_test(plan_id.clone());
    plan_runtime.set_mode_pending(plan_id.clone());

    let system = ChatMessage::system("sys");
    let user = ChatMessage::user("u".repeat(4_000));
    let assistant = ChatMessage::assistant("a".repeat(4_000));
    let mut messages = vec![system, user, assistant];
    let tail_chars: usize = messages.iter().skip(1).map(estimate_msg_chars).sum();

    let mut agent = AgentLoop::new(
        Arc::new(ChatOnlyMockLlm {
            summary_text: "continue with pending work".to_string(),
        }),
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
        preheat: Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    }));

    current_tail_guard::maybe_reduce_before_next_llm(&mut agent, &mut messages)
        .await
        .unwrap();

    let text = messages[1].text_content().unwrap_or("");
    assert!(text.contains("- mode: pending"));
    assert!(text.contains("current_step: first pending"));

    cleanup_plan_file(&plan_path);
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
