//! SessionManager：会话 CRUD、transcript 追加与只读、上下文组装、会话级配置隔离。
//!
//! 通过 Mutex 序列化 sessions.json 的写入，保证并发安全（不锁文件）。

mod context;
mod session_impl;
#[cfg(test)]
mod tests;
mod types;

pub(crate) use context::INTERRUPTED_TOOL_RESULT_TEXT;
pub use context::{build_context_from_state, init_context_state};
#[allow(unused_imports)]
pub use session_impl::generate_entry_id;
pub use session_impl::{derive_title_from_user_message, is_rule_derived_title};
pub use session_impl::SessionManager;
pub use types::{
    compound_turn_id, estimate_msg_chars, estimated_tokens_from_chars, ApiUsage, CompactionResult,
    ContextLiveMetrics, ContextState, PlanEventKind, PlanEventRef,
};

pub trait MessageAppendSink: Send + Sync {
    fn append_message(
        &self,
        value: serde_json::Value,
    ) -> Result<String, crate::infra::error::AppError>;
}

const BRANCH_MAX_ENTRIES: usize = 2000;

#[cfg(test)]
use crate::core::session::transcript::{BranchSummaryEntry, MessageEntry, TranscriptEntry};
#[cfg(test)]
use crate::infra::config::ContextConfig;
#[cfg(test)]
use context::{compute_fold_start, filter_turns_by_day, is_user_message, parse_date};
