//! 上下文 Compaction：异步预热 + Boundary 延迟应用。
//!
//! - Layer 0: 超大 tool result 落盘 + preview 占位符（保全信息）
//! - Layer 1: 异步 LLM 摘要预热（后台 tokio task）
//! - Layer 2: 延迟应用（时机 ⑤ 非阻塞 + 时机 ② async 检查）
//! - Layer 3: 强制删除最旧 turn（仅 API Context Overflow 后触发）

pub mod apply;
mod cascade;
pub mod preheat;
mod truncation;

#[cfg(test)]
mod tests;

// ---------------------------------------------------------------------------
// Public re-exports
// ---------------------------------------------------------------------------

pub use truncation::{
    compact_tool_results, layer0_persist_large_results, run_layer0_cleanup, Layer0CleanupOutcome,
    PersistedResult,
};

pub use cascade::{force_drop_oldest_to_target, is_context_overflow_error};
