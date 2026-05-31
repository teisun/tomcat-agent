use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use super::super::current_tail_guard;
use super::super::{AgentLoop, AgentLoopConfig};
use super::mocks::MockPrimitiveExecutor;
use crate::core::compaction::preheat::Preheat;
use crate::core::compaction::TOOL_RESULT_PLACEHOLDER;
use crate::core::llm::{
    ChatMessage, ChatRequest, ChatResponse, ChatResponseChoice, LlmProvider, StreamEvent,
};
use crate::core::plan_runtime::file_store::{TodoItem, TodoStatus};
use crate::core::plan_runtime::PlanRuntime;
use crate::core::session::manager::{
    estimate_msg_chars, ApiUsage, ContextState, PlanEventKind, PlanEventRef,
};
use crate::core::session::transcript::{
    append_entry, read_entries_tail, write_header, MessageEntry, SessionHeader, TranscriptEntry,
};
use crate::infra::config::ContextConfig;
use crate::infra::error::AppError;
use crate::infra::event_bus::DefaultEventBus;

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
async fn mid_turn_guard_rewrites_tail_and_transcript() {
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("session.jsonl");
    write_session_header(&transcript);

    let system = ChatMessage::system("sys");
    let mut user = ChatMessage::user("read everything");
    user.msg_id = Some("u1".to_string());
    let calls: Vec<_> = (1..=5)
        .map(|i| {
            serde_json::json!({
                "id": format!("tc{i}"),
                "type": "function",
                "function": {"name": "read", "arguments": format!("{{\"path\":\"file{i}\"}}")}
            })
        })
        .collect();
    let mut assistant = ChatMessage::assistant_with_tool_calls(Some("tools"), calls);
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

    let config = AgentLoopConfig {
        session_id: "sess-mid-turn".to_string(),
        agent_trail_dir: dir.path().to_string_lossy().to_string(),
        context_config: ContextConfig {
            current_tail_compactable_min_chars: 1,
            current_tail_single_result_max_chars: 10_000,
            ..Default::default()
        },
        ..Default::default()
    };
    let llm = Arc::new(ChatOnlyMockLlm {
        summary_text: "unused".to_string(),
    });
    let mut agent = AgentLoop::new(
        llm,
        Arc::new(MockPrimitiveExecutor),
        Arc::new(DefaultEventBus::new()),
        config,
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

    let tool_texts: Vec<String> = messages
        .iter()
        .filter(|msg| msg.role == crate::core::llm::ChatMessageRole::Tool)
        .map(|msg| msg.text_content().unwrap_or("").to_string())
        .collect();
    assert!(
        tool_texts
            .iter()
            .any(|text| text.starts_with("[Tool result persisted:")),
        "one large result should persist to preview"
    );
    assert!(
        tool_texts
            .iter()
            .any(|text| text == TOOL_RESULT_PLACEHOLDER),
        "older compactable results should be placeholdered"
    );

    let state = agent.context_state.as_ref().unwrap();
    assert!(
        state.post_usage_appended_chars < tail_chars,
        "rewriting current tail should shrink appended chars"
    );
    assert!(
        !state.is_over_budget(),
        "guard should bring the request back under budget"
    );

    let transcript_entries = read_entries_tail(&transcript, 20).unwrap();
    let transcript_tool_texts: Vec<String> = transcript_entries
        .into_iter()
        .filter_map(|entry| match entry {
            TranscriptEntry::Message(me) => me
                .message
                .get("content")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            _ => None,
        })
        .collect();
    assert!(transcript_tool_texts
        .iter()
        .any(|text| text.starts_with("[Tool result persisted:")));
    assert!(transcript_tool_texts
        .iter()
        .any(|text| text == TOOL_RESULT_PLACEHOLDER));
}

#[tokio::test]
async fn collapse_to_branch_summary_keeps_planning_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("collapse.jsonl");
    write_session_header(&transcript);

    let plan_runtime = PlanRuntime::new("sess-plan");
    plan_runtime.enter_planning().unwrap();
    plan_runtime.set_active_planning_plan(
        "plan_123".to_string(),
        PathBuf::from("/tmp/ignored.plan.md"),
    );
    plan_runtime.replace_session_todos(vec![
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
    ]);

    let system = ChatMessage::system("sys");
    let mut user = ChatMessage::user("u".repeat(4_000));
    user.msg_id = Some("u1".to_string());
    let mut assistant = ChatMessage::assistant("a".repeat(4_000));
    assistant.msg_id = Some("a1".to_string());
    append_transcript_message(&transcript, &user);
    append_transcript_message(&transcript, &assistant);

    let mut messages = vec![system, user, assistant];
    let tail_chars: usize = messages.iter().skip(1).map(estimate_msg_chars).sum();

    let config = AgentLoopConfig {
        session_id: "sess-collapse".to_string(),
        plan_runtime: Some(plan_runtime),
        ..Default::default()
    };
    let llm = Arc::new(ChatOnlyMockLlm {
        summary_text: "continue with plan execution".to_string(),
    });
    let mut agent = AgentLoop::new(
        llm,
        Arc::new(MockPrimitiveExecutor),
        Arc::new(DefaultEventBus::new()),
        config,
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
            plan_id: "plan_123".to_string(),
            path: PathBuf::from("/tmp/demo.plan.md"),
        }),
        preheat: Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    }));

    current_tail_guard::maybe_reduce_before_next_llm(&mut agent, &mut messages)
        .await
        .unwrap();

    assert_eq!(messages.len(), 2, "system + collapsed summary");
    let summary = &messages[1];
    assert_eq!(
        summary.kind,
        crate::core::llm::MessageKind::CompactionSummary
    );
    let text = summary.text_content().unwrap_or("");
    assert!(text.contains("## Execution Keepalive"));
    assert!(text.contains("- mode: planning"));
    assert!(text.contains("step active"));
    assert!(text.contains("step pending"));
    assert!(text.contains("build:plan_123:/tmp/demo.plan.md"));

    let state = agent.context_state.as_ref().unwrap();
    assert_eq!(state.messages.len(), 1);
    assert_eq!(
        state.messages[0].kind,
        crate::core::llm::MessageKind::CompactionSummary
    );

    let entries = read_entries_tail(&transcript, 10).unwrap();
    let last = entries.last().unwrap();
    match last {
        TranscriptEntry::BranchSummary(entry) => {
            assert_eq!(entry.is_boundary, Some(true));
            assert!(entry
                .summary
                .as_deref()
                .unwrap_or("")
                .contains("Execution Keepalive"));
        }
        other => panic!("expected collapse branch summary, got {other:?}"),
    }
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

fn write_session_header(path: &Path) {
    write_header(
        path,
        &SessionHeader {
            r#type: "session".to_string(),
            version: Some(1),
            id: "sid".to_string(),
            timestamp: "2026-05-30T16:00:00Z".to_string(),
            cwd: None,
        },
    )
    .unwrap();
}
