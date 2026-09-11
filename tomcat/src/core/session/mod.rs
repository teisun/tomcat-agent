//! 会话管理：元数据 store（sessions.json）与 transcript（pi-mono 相容 JSONL）的 CRUD、上下文组装。

mod append_message_chain;
pub mod attachments;
pub mod context_metrics;
pub(crate) mod housekeeping_ledger;
pub(crate) mod manager;
mod model_thinking;
pub(crate) mod preheat_cache;
pub(crate) mod resume_index;
pub mod scope;
pub(crate) mod store;
pub(crate) mod subagent_transcript;
pub(crate) mod tool_display_sidecar;
pub(crate) mod user_message_sidecar;

pub mod transcript;

#[cfg(test)]
pub(crate) use append_message_chain::assert_active_tool_result_integrity;
pub(crate) use append_message_chain::{
    classify_resumable_ask_question_result, collect_recent_chat_messages_from_tail,
    find_dangling_tail_tool_call_ids, find_dangling_tail_tool_calls,
    has_complete_tail_tool_results, has_dangling_tool_calls_in_messages, is_tool_call_pending,
    ResumableAskQuestionResult,
};
pub use context_metrics::{ContextLiveMetrics, ContextMetrics};
pub use manager::{
    build_context_from_state, compound_turn_id, estimate_msg_chars, init_context_state,
    init_context_state_with_limits, AgentMode, ApiUsage, CompactionResult, ContextState,
    MessageAppendSink, PlanEventKind, PlanEventRef, ResumeControlState, SessionManager,
    INTERRUPTED_TOOL_RESULT_TEXT, PENDING_TOOL_RESULT_TEXT, UNKNOWN_RESTART_TOOL_RESULT_TEXT,
};
pub use model_thinking::{ModelPrefs, ModelPrefsStore};
pub use resume_index::RESUME_INDEX_SCHEMA_VERSION;
pub use scope::{
    fnv1a_hex, project_root, resolve_session_mode, session_key_for, session_key_for_agent,
    SessionMode,
};
pub use store::{load_store, save_store, SessionEntry, SessionStore, DEFAULT_SESSION_KEY};
pub use transcript::{
    append_entry, append_line, insert_entry_after_message_id,
    mark_message_entries_after_anchor_superseded,
    mark_tool_result_entries_by_tool_call_id_superseded, mark_trailing_user_messages_superseded,
    read_entries_tail, read_header, rewrite_message_summary_titles_by_id,
    rewrite_message_text_entries_by_id, write_header, BranchSummaryEntry, BranchSummaryTextEntry,
    ErrorEntry, MessageEntry, MessageSummaryTitleRewrite, MessageTextRewrite, SessionHeader,
    ThinkingTraceEntry, ToolResultsCompactedEntry, TranscriptEntry,
};

#[cfg(test)]
mod tests;
