//! 上下文 Compaction 四层防护算法 V2。
//!
//! - Layer 0: 超大 tool result 落盘 + preview 占位符（保全信息）
//! - Layer 1: compactable zone 内 tool result > 20K 占位符替换（零 LLM 开销）
//! - Layer 2: LLM 一次性摘要 compactable zone（按 m 值保护最近 turns）
//! - Layer 3: 强制删除最旧 turn 到 ratio < 0.50 兜底
//!
//! 由 ratio 水位线驱动级联降压：每层执行后重算 ratio，降压成功即停。

mod cascade;
mod summary;
mod truncation;

#[cfg(test)]
mod tests;

// ---------------------------------------------------------------------------
// Public re-exports (preserves original module API surface)
// ---------------------------------------------------------------------------

pub use truncation::{
    compact_tool_results, layer0_persist_large_results, truncate_tool_result_if_needed,
    PersistedResult, TruncationInfo,
};

pub use summary::{
    run_compaction, run_compaction_loop, SUMMARIZATION_PROMPT, UPDATE_SUMMARIZATION_PROMPT,
};

pub use cascade::{
    determine_cascade_params, force_drop_oldest, force_drop_oldest_to_target,
    is_context_overflow_error, run_compaction_cascade, run_compaction_cascade_v2, CascadeParams,
    CascadeResult,
};
