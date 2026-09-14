use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use super::super::current_tail_guard;
use super::super::{AgentLoop, AgentLoopConfig};
use super::mocks::{test_binding, MockPrimitiveExecutor};
use crate::core::compaction::preheat::Preheat;
use crate::core::compaction::TOOL_RESULT_PLACEHOLDER;
use crate::core::llm::{
    ChatMessage, ChatRequest, ChatResponse, ChatResponseChoice, LlmProvider, StreamEvent,
};
use crate::core::plan_runtime::file_store::{PlanFileState, TodoItem, TodoStatus};
use crate::core::plan_runtime::PlanRuntime;
use crate::core::session::manager::{
    estimate_msg_chars, ApiUsage, CompactionResult, ContextState, PlanEventKind, PlanEventRef,
};
use crate::core::session::transcript::{
    append_entry, read_entries_tail, write_header, MessageEntry, SessionHeader, TranscriptEntry,
};
use crate::core::session::user_message_sidecar::user_message_sidecar_path;

use crate::infra::config::ContextConfig;
use crate::infra::error::AppError;
use crate::infra::event_bus::{DefaultEventBus, EventBus};
use crate::infra::wire;

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

    let mut messages = vec![user, assistant];
    messages.append(&mut tools);
    let tail_chars: usize = messages.iter().map(estimate_msg_chars).sum();

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
        test_binding(llm, "gpt-4"),
        Arc::new(MockPrimitiveExecutor),
        Arc::new(DefaultEventBus::new()),
        config,
        CancellationToken::new(),
    );
    agent.start_idx = 0;
    agent.context_tail_start = 0;
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
    assert!(
        !transcript_tool_texts
            .iter()
            .any(|text| text == TOOL_RESULT_PLACEHOLDER),
        "placeholder waves must leave original JSONL message bodies intact"
    );
    assert!(
        !read_entries_tail(&transcript, 20)
            .unwrap()
            .iter()
            .any(|entry| matches!(entry, TranscriptEntry::ToolResultsCompacted(_))),
        "placeholder waves must remain runtime-only and never append a marker"
    );
}

#[tokio::test]
async fn collapse_to_branch_summary_keeps_planning_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("collapse.jsonl");
    write_session_header(&transcript);

    let plan_runtime = PlanRuntime::new("sess-plan");
    plan_runtime.enter_plan().unwrap();
    plan_runtime.seed_active_plan_for_test("plan_123".to_string(), PlanFileState::Planning);
    plan_runtime.replace_session_todos(vec![
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
    ]);

    let mut user = ChatMessage::user("u".repeat(4_000));
    user.msg_id = Some("u1".to_string());
    let mut tool_calls = (1..=25)
        .map(|index| {
            serde_json::json!({
                "id": format!("read-{index}"),
                "type": "function",
                "function": {
                    "name": "read",
                    "arguments": format!(r#"{{"path":"src/read-{index}.rs"}}"#),
                },
            })
        })
        .collect::<Vec<_>>();
    tool_calls.extend([
        serde_json::json!({
            "id": "edit-1",
            "type": "function",
            "function": {"name": "edit", "arguments": r#"{"path":"src/edited.rs"}"#},
        }),
        serde_json::json!({
            "id": "write-1",
            "type": "function",
            "function": {"name": "write", "arguments": r#"{"path":"src/written.rs"}"#},
        }),
    ]);
    let mut assistant =
        ChatMessage::assistant_with_tool_calls(Some(&"a".repeat(4_000)), tool_calls);
    assistant.msg_id = Some("a1".to_string());
    append_transcript_message(&transcript, &user);
    append_transcript_message(&transcript, &assistant);
    let transcript_prefix_before_collapse = std::fs::read_to_string(&transcript).unwrap();

    let mut messages = vec![user, assistant];
    let tail_chars: usize = messages.iter().map(estimate_msg_chars).sum();

    let config = AgentLoopConfig {
        session_id: "sess-collapse".to_string(),
        plan_runtime: Some(plan_runtime),
        ..Default::default()
    };
    let llm = Arc::new(ChatOnlyMockLlm {
        summary_text: "continue with plan execution".to_string(),
    });
    let mut agent = AgentLoop::new(
        test_binding(llm, "gpt-4"),
        Arc::new(MockPrimitiveExecutor),
        Arc::new(DefaultEventBus::new()),
        config,
        CancellationToken::new(),
    );
    agent.start_idx = 0;
    agent.context_tail_start = 0;
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
        resume_control: Default::default(),
        preheat: Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    }));

    current_tail_guard::maybe_reduce_before_next_llm(&mut agent, &mut messages)
        .await
        .unwrap();

    assert_eq!(messages.len(), 1, "collapsed summary");
    let summary = &messages[0];
    assert_eq!(
        summary.kind,
        crate::core::llm::MessageKind::CompactionSummary
    );
    let text = summary.text_content().unwrap_or("");
    assert!(text.starts_with("<control_state>"));
    assert!(text.contains("mode: plan"));
    assert!(text.contains("<verbatim_user_messages>"));
    assert!(text.contains("<recent_files>"));
    assert!(
        !text.contains("src/read-5.rs"),
        "only the latest 20 unique read paths should be retained"
    );
    assert!(text.contains("src/read-6.rs"));
    assert!(text.contains("src/read-25.rs"));
    assert!(text.contains("src/edited.rs"));
    assert!(text.contains("src/written.rs"));
    assert!(text.contains("git status --short"));
    let sidecar_path = user_message_sidecar_path(&transcript);
    assert!(
        sidecar_path.is_file(),
        "collapse must materialize the user-message sidecar"
    );
    assert!(
        text.contains(&sidecar_path.display().to_string()),
        "collapse summary must point to its readable sidecar"
    );
    assert!(
        std::fs::read_to_string(&sidecar_path)
            .unwrap()
            .contains("\"id\":\"u1\""),
        "sidecar must preserve the Normal user message"
    );

    assert!(text.contains("## Progress"));
    assert!(text.contains("Rendered from the session todo scratchpad"));
    assert!(text.contains("t2: step active"));

    let entries = read_entries_tail(&transcript, 10).unwrap();
    let last = entries.last().unwrap();
    assert!(
        std::fs::read_to_string(&transcript)
            .unwrap()
            .starts_with(&transcript_prefix_before_collapse),
        "collapse must preserve every existing transcript byte and append its boundary at the tail"
    );
    match last {
        TranscriptEntry::BranchSummary(entry) => {
            assert_eq!(entry.is_boundary, Some(true));
            assert!(entry
                .summary
                .as_deref()
                .unwrap_or("")
                .contains("<control_state>"));
        }
        other => panic!("expected collapse branch summary, got {other:?}"),
    }
}

#[tokio::test]
async fn preheat_starts_at_tool_round_when_ratio_reaches_half() {
    let mut user = ChatMessage::user("u".repeat(100));
    user.msg_id = Some("u1".to_string());
    let mut messages = vec![user];
    let config = AgentLoopConfig {
        session_id: "sess-midturn-preheat".to_string(),
        ..Default::default()
    };
    let mut agent = AgentLoop::new(
        test_binding(
            Arc::new(ChatOnlyMockLlm {
                summary_text: "summary".to_string(),
            }),
            "gpt-4",
        ),
        Arc::new(MockPrimitiveExecutor),
        Arc::new(DefaultEventBus::new()),
        config,
        CancellationToken::new(),
    );
    agent.start_idx = 0;
    agent.context_tail_start = 0;
    agent.set_context_state(Some(ContextState {
        messages: vec![],
        estimate_context_chars: 100,
        context_budget_chars: 120,
        context_budget_tokens: 30,
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

    let preheat = &agent.context_state.as_ref().unwrap().preheat;
    assert!(
        preheat.is_running() || preheat.is_finished(),
        "the guard's Fits path must start a preheat once the build reaches 50%"
    );
    agent.context_state.as_mut().unwrap().preheat.abort();
}

#[tokio::test]
async fn midturn_preheat_anchor_in_tail_applies_and_keeps_surviving_tail_raw() {
    let mut first = ChatMessage::user("a".repeat(400));
    first.msg_id = Some("u1".to_string());
    let mut covered_end = ChatMessage::assistant("b".repeat(400));
    covered_end.msg_id = Some("a1".to_string());
    let raw_tail = "tail must remain raw ".repeat(4_000);
    let mut tail = tool_message("t1", "call-1", &raw_tail);
    tail.timestamp = Some("2026-09-08T00:00:00Z".to_string());
    let mut messages = vec![first, covered_end, tail];
    let mut preheat = Preheat::new();
    preheat.restore_completed(CompactionResult {
        summary_text: "preheated prefix summary".to_string(),
        covered_start_id: "u1".to_string(),
        covered_end_id: "a1".to_string(),
        covered_count: 2,
        transcript_compaction_entry_id: None,
        estimated_covered_tokens_before: Some(200),
        estimated_summary_tokens: Some(8),
        estimated_tokens_saved: Some(192),
        preheat_elapsed_ms: 1,
    });
    let config = AgentLoopConfig {
        session_id: "sess-ready-midturn-preheat".to_string(),
        context_config: ContextConfig {
            keep_recent_turns: 0,
            ..Default::default()
        },
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
        config,
        CancellationToken::new(),
    );
    agent.start_idx = 0;
    agent.context_tail_start = 0;
    agent.set_context_state(Some(ContextState {
        messages: vec![],
        estimate_context_chars: 900,
        context_budget_chars: 400,
        context_budget_tokens: 100,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        latest_plan_event: None,
        resume_control: Default::default(),
        preheat,
        session_obs: Default::default(),
        live: Default::default(),
    }));

    current_tail_guard::maybe_reduce_before_next_llm(&mut agent, &mut messages)
        .await
        .unwrap();

    assert_eq!(messages.len(), 2, "prefix summary + raw tail");
    assert_eq!(
        messages[0].kind,
        crate::core::llm::MessageKind::CompactionSummary
    );
    assert_eq!(
        messages[1].text_content(),
        Some(raw_tail.as_str()),
        "history_end must protect a surviving 80K tail even when no recent turns are retained"
    );
    assert_eq!(
        agent.start_idx, 1,
        "the surviving tool result is still the active-tail start after the summary"
    );
    assert_eq!(
        messages
            .iter()
            .filter_map(|message| message.msg_id.as_deref())
            .collect::<std::collections::HashSet<_>>()
            .len(),
        messages
            .iter()
            .filter(|message| message.msg_id.is_some())
            .count(),
        "the fold/apply/unfold handoff must not duplicate persisted message ids"
    );
}

#[tokio::test]
async fn incident_replay_from_085_to_098_applies_tail_anchor_without_stale() {
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

    let mut user = ChatMessage::user("u".repeat(400));
    user.msg_id = Some("u1".to_string());
    let mut first_assistant = ChatMessage::assistant("a".repeat(400));
    first_assistant.msg_id = Some("a1".to_string());
    let mut messages = vec![user, first_assistant];
    let mut agent = AgentLoop::new(
        test_binding(
            Arc::new(ChatOnlyMockLlm {
                summary_text: "incident summary".to_string(),
            }),
            "gpt-4",
        ),
        Arc::new(MockPrimitiveExecutor),
        event_bus,
        AgentLoopConfig {
            session_id: "incident-085-098".to_string(),
            ..Default::default()
        },
        CancellationToken::new(),
    );
    agent.start_idx = 0;
    agent.context_tail_start = 0;
    agent.set_context_state(Some(ContextState {
        messages: vec![],
        estimate_context_chars: 3_400,
        context_budget_chars: 4_000,
        context_budget_tokens: 1_000,
        last_api_usage: Some(ApiUsage {
            prompt_tokens: 850,
            completion_tokens: 0,
        }),
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        latest_plan_event: None,
        resume_control: Default::default(),
        preheat: Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    }));

    // Incident step 1: at 85%, the mid-turn Fits path starts a snapshot whose anchor is in the
    // still-uncommitted tail.
    current_tail_guard::maybe_reduce_before_next_llm(&mut agent, &mut messages)
        .await
        .unwrap();
    let result = match agent
        .context_state
        .as_mut()
        .unwrap()
        .preheat
        .await_result(std::time::Duration::from_secs(1))
        .await
    {
        crate::core::compaction::preheat::PreheatOutcome::Completed(result) => result,
        outcome => panic!("85% preheat must complete for replay, got {outcome:?}"),
    };

    // Tool work arrives after the snapshot. At 98%, applying that finished preheat must fold the
    // new tail into the same coordinate system instead of reporting ApplyBoundaryStale.
    let mut later_tool = tool_message("t1", "call-1", "later tool result must remain raw");
    later_tool.timestamp = Some("2026-09-14T00:00:02Z".to_string());
    messages.push(later_tool);
    let state = agent.context_state.as_mut().unwrap();
    state.update_api_usage(980, 0);
    state.preheat.restore_completed(result);

    assert!(
        current_tail_guard::apply_ready_preheat(&mut agent, &mut messages, Some(0.85)).unwrap(),
        "the completed preheat must apply through the merged view"
    );
    assert_eq!(switched.load(Ordering::SeqCst), 1);
    assert_eq!(errors.load(Ordering::SeqCst), 0);
    assert_eq!(messages.len(), 2, "summary + surviving later tool");
    assert_eq!(
        messages[0].kind,
        crate::core::llm::MessageKind::CompactionSummary
    );
    assert_eq!(
        messages[agent.start_idx].text_content(),
        Some("later tool result must remain raw"),
        "the request payload retains post-snapshot tail work unchanged"
    );
}

#[tokio::test]
async fn midturn_summary_boundary_lands_on_round_boundary() {
    let mut user = ChatMessage::user("u".repeat(400));
    user.msg_id = Some("u1".to_string());
    let mut first_assistant = ChatMessage::assistant_with_tool_calls(
        Some("first tool round"),
        vec![serde_json::json!({
            "id": "tc1",
            "type": "function",
            "function": {"name": "read", "arguments": r#"{"path":"one.txt"}"#},
        })],
    );
    first_assistant.msg_id = Some("a1".to_string());
    let first_tool = tool_message("t1", "tc1", &"one\n".repeat(100));
    let mut second_assistant = ChatMessage::assistant_with_tool_calls(
        Some("second tool round"),
        vec![serde_json::json!({
            "id": "tc2",
            "type": "function",
            "function": {"name": "read", "arguments": r#"{"path":"two.txt"}"#},
        })],
    );
    second_assistant.msg_id = Some("a2".to_string());
    let second_tool = tool_message("t2", "tc2", &"two\n".repeat(25));
    let mut messages = vec![
        user,
        first_assistant,
        first_tool,
        second_assistant,
        second_tool,
    ];
    let mut preheat = Preheat::new();
    preheat.restore_completed(CompactionResult {
        summary_text: "preheated first round".to_string(),
        covered_start_id: "u1".to_string(),
        covered_end_id: "t1".to_string(),
        covered_count: 3,
        transcript_compaction_entry_id: None,
        estimated_covered_tokens_before: None,
        estimated_summary_tokens: None,
        estimated_tokens_saved: None,
        preheat_elapsed_ms: 0,
    });
    let chars: usize = messages.iter().map(estimate_msg_chars).sum();
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
            session_id: "midturn-boundary".to_string(),
            ..Default::default()
        },
        CancellationToken::new(),
    );
    agent.start_idx = 0;
    agent.context_tail_start = 0;
    agent.set_context_state(Some(ContextState {
        messages: Vec::new(),
        estimate_context_chars: chars + 800,
        context_budget_chars: 1_200,
        context_budget_tokens: 300,
        last_api_usage: None,
        post_usage_appended_chars: chars,
        transcript_path: PathBuf::new(),
        latest_plan_event: None,
        resume_control: Default::default(),
        preheat,
        session_obs: Default::default(),
        live: Default::default(),
    }));
    let tail_before = messages[3..].to_vec();

    current_tail_guard::maybe_reduce_before_next_llm(&mut agent, &mut messages)
        .await
        .unwrap();

    assert_eq!(
        messages[0].kind,
        crate::core::llm::MessageKind::CompactionSummary
    );
    assert_eq!(
        messages[1].role,
        crate::core::llm::ChatMessageRole::Assistant,
        "the first raw message after a preheat summary must begin the next complete tool round"
    );
    assert_eq!(
        serde_json::to_vec(&messages[agent.start_idx..]).unwrap(),
        serde_json::to_vec(&tail_before).unwrap(),
        "a preheat anchored in earlier history must preserve every later tail message in order"
    );
    assert!(
        !crate::core::session::has_dangling_tool_calls_in_messages(&messages),
        "the summary splice must preserve paired assistant/tool messages"
    );
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
            project_root: None,
        },
    )
    .unwrap();
}
