pub(super) use super::super::preheat::messages_to_text;
use crate::core::compaction::preheat::Preheat;
use crate::core::llm::ChatMessage;
use crate::core::session::manager::{CompactionResult, ContextState};

const TS: &str = "2026-04-04T12:00:00Z";

// ---------------------------------------------------------------------------
// Chat message helpers shared across multiple `tests/*` submodules
// (historically lived in a single `tests.rs` file).
// ---------------------------------------------------------------------------

pub(super) fn assistant_msg(text: &str) -> ChatMessage {
    let mut m = ChatMessage::assistant(text);
    m.timestamp = Some(TS.to_string());
    m
}

pub(super) fn steering_msg(text: &str) -> ChatMessage {
    let mut m = ChatMessage::steering(text);
    m.timestamp = Some(TS.to_string());
    m
}

pub(super) fn summary_msg(text: &str) -> ChatMessage {
    let mut m = ChatMessage::compaction_summary(text);
    m.msg_id = Some("summary_0".to_string());
    m.timestamp = Some(TS.to_string());
    m
}

// ---------------------------------------------------------------------------
// Helper factories
// ---------------------------------------------------------------------------

pub(super) fn user_msg_with_id(id: &str, text: &str) -> ChatMessage {
    let mut m = ChatMessage::user(text);
    m.msg_id = Some(id.to_string());
    m.timestamp = Some(TS.to_string());
    m
}

pub(super) fn tool_msg(tcid: &str, content: &str) -> ChatMessage {
    ChatMessage::tool(tcid, content)
}

pub(super) fn tool_msg_with_id(id: &str, tcid: &str, content: &str) -> ChatMessage {
    let mut m = ChatMessage::tool(tcid, content);
    m.msg_id = Some(id.to_string());
    m.timestamp = Some(TS.to_string());
    m
}

pub(super) fn user_msg(text: &str) -> ChatMessage {
    let mut m = ChatMessage::user(text);
    m.timestamp = Some(TS.to_string());
    m
}

pub(super) fn dummy_compaction_result() -> CompactionResult {
    CompactionResult {
        summary_text: "summary".into(),
        covered_start_id: "start".into(),
        covered_end_id: "end".into(),
        covered_count: 1,
        transcript_compaction_entry_id: None,
        estimated_covered_tokens_before: Some(10),
        estimated_summary_tokens: Some(2),
        estimated_tokens_saved: Some(8),
        preheat_elapsed_ms: 0,
    }
}

pub(super) fn make_state(chars: usize, budget_chars: usize, budget_tokens: usize) -> ContextState {
    ContextState {
        messages: vec![],
        estimate_context_chars: chars,
        context_budget_chars: budget_chars,
        context_budget_tokens: budget_tokens,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: std::path::PathBuf::new(),
        latest_plan_event: None,
        preheat: Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    }
}
