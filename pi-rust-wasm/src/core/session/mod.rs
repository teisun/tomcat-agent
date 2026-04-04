//! 会话管理：元数据 store（sessions.json）与 transcript（pi 系 JSONL）的 CRUD、上下文组装。

mod append_message_chain;
pub(crate) mod manager;
pub(crate) mod store;
pub mod transcript;

pub use manager::{
    build_context_from_state, estimate_turn_chars, init_context_state, ApiUsage, ContextState,
    SessionManager, TurnEntry,
};
pub use store::{load_store, save_store, SessionEntry, SessionStore, DEFAULT_SESSION_KEY};
pub use transcript::{
    append_entry, append_line, read_entries_tail, read_header, write_header, CompactionEntry,
    MessageEntry, SessionHeader, TranscriptEntry,
};
