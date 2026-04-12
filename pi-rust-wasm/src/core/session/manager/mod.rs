//! SessionManager：会话 CRUD、transcript 追加与只读、上下文组装、会话级配置隔离。
//!
//! 通过 Mutex 序列化 sessions.json 的写入，保证并发安全（不锁文件）。

mod context;
mod session_impl;
#[cfg(test)]
mod tests;
mod types;

pub use context::{build_context_from_state, init_context_state};
#[allow(unused_imports)]
pub use session_impl::generate_entry_id;
pub use session_impl::SessionManager;
pub use types::{
    compound_turn_id, estimate_turn_chars, estimated_tokens_from_chars, ApiUsage, CompactionResult,
    ContextLiveMetrics, ContextState, TurnEntry,
};

const BRANCH_MAX_ENTRIES: usize = 2000;

#[cfg(test)]
use crate::core::agent_loop::AgentMessage;
#[cfg(test)]
use crate::core::session::transcript::{CompactionEntry, MessageEntry, TranscriptEntry};
#[cfg(test)]
use crate::infra::config::ContextConfig;
#[cfg(test)]
use context::{compute_fold_start, filter_turns_by_day, is_user_message, parse_date};
