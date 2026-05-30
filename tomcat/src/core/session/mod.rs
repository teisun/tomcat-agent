//! 会话管理：元数据 store（sessions.json）与 transcript（pi-mono 相容 JSONL）的 CRUD、上下文组装。

mod append_message_chain;
pub mod context_metrics;
pub(crate) mod manager;
pub(crate) mod store;
pub mod transcript;

pub use context_metrics::{ContextLiveMetrics, ContextMetrics};
pub use manager::{
    build_context_from_state, compound_turn_id, estimate_msg_chars, init_context_state, ApiUsage,
    CompactionResult, ContextState, MessageAppendSink, SessionManager,
};
pub use store::{load_store, save_store, SessionEntry, SessionStore, DEFAULT_SESSION_KEY};
pub use transcript::{
    append_entry, append_line, insert_entry_after_message_id,
    mark_message_entries_after_anchor_superseded, read_entries_tail, read_header,
    remove_branch_summary_entry_by_id, rewrite_message_text_entries_by_id,
    set_branch_summary_entry_is_boundary_true, write_header, BranchSummaryEntry, MessageEntry,
    MessageTextRewrite, SessionHeader, ThinkingTraceEntry, TranscriptEntry,
};

#[cfg(test)]
mod tests;
