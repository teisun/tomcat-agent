//! Layer 3: 强制删除最旧 turn & Cascade 级联压缩。

use std::path::Path;

use crate::core::llm::LlmProvider;
use crate::core::session::manager::{estimate_turn_chars, ContextState};
use crate::infra::config::ContextConfig;

use super::summary::{run_compaction, run_compaction_loop};
use super::truncation::{compact_tool_results, layer0_persist_large_results, PersistedResult};

// ---------------------------------------------------------------------------
// Layer 3: Force drop oldest to target ratio
// ---------------------------------------------------------------------------

/// Layer 3 V2：强制删除最旧 turn 直到 ratio < 0.50。
pub fn force_drop_oldest_to_target(state: &mut ContextState) {
    while state.usage_ratio() >= 0.50 && !state.user_turns_list.is_empty() {
        let removed = state.user_turns_list.remove(0);
        let chars = estimate_turn_chars(&removed);
        state.estimate_context_chars = state.estimate_context_chars.saturating_sub(chars);
    }
    state.invalidate_api_usage();
}

/// Layer 3 legacy：强制删除最旧 turn 直到回预算。
pub fn force_drop_oldest(state: &mut ContextState) {
    while state.is_over_budget() && !state.user_turns_list.is_empty() {
        let removed = state.user_turns_list.remove(0);
        let chars = estimate_turn_chars(&removed);
        state.estimate_context_chars = state.estimate_context_chars.saturating_sub(chars);
    }
}

// ---------------------------------------------------------------------------
// Cascade params: ratio watermark logic
// ---------------------------------------------------------------------------

/// Cascade 参数：由 ratio 水位线决定。
#[derive(Debug, Clone)]
pub struct CascadeParams {
    pub should_cascade: bool,
    pub m: usize,
    pub block_tool_calls: bool,
    pub target_layer3: bool,
}

/// 根据当前 ratio 和 buffer 安全网决定 cascade 参数。
pub fn determine_cascade_params(state: &ContextState, config: &ContextConfig) -> CascadeParams {
    let ratio = state.usage_ratio();
    let input_budget = config
        .context_window
        .saturating_sub(config.max_output_tokens);
    let remaining = input_budget.saturating_sub(state.estimated_token_count());

    let buffer_cap = |val: usize| val.min(input_budget * 3 / 10);
    let autocompact_buf = buffer_cap(config.autocompact_buffer_tokens);
    let warning_buf = buffer_cap(config.warning_buffer_tokens);

    if ratio >= 1.0 {
        CascadeParams {
            should_cascade: true,
            m: 1,
            block_tool_calls: true,
            target_layer3: true,
        }
    } else if ratio >= 0.98 {
        CascadeParams {
            should_cascade: true,
            m: 1,
            block_tool_calls: true,
            target_layer3: false,
        }
    } else if ratio >= 0.92 || remaining < autocompact_buf {
        CascadeParams {
            should_cascade: true,
            m: 2,
            block_tool_calls: false,
            target_layer3: false,
        }
    } else if ratio >= 0.85 || remaining < warning_buf {
        CascadeParams {
            should_cascade: true,
            m: 3,
            block_tool_calls: false,
            target_layer3: false,
        }
    } else if ratio >= 0.70 {
        CascadeParams {
            should_cascade: true,
            m: 5,
            block_tool_calls: false,
            target_layer3: false,
        }
    } else {
        CascadeParams {
            should_cascade: false,
            m: 5,
            block_tool_calls: false,
            target_layer3: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Cascade result
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct CascadeResult {
    pub layers_executed: Vec<u8>,
    pub ratio_before: f64,
    pub ratio_after: f64,
    pub block_tool_calls: bool,
    pub persisted_results: Vec<PersistedResult>,
}

// ---------------------------------------------------------------------------
// Compaction cascade V2: L0 → L1 → L2 → L3
// ---------------------------------------------------------------------------

/// V2 级联压缩：ratio 水位线驱动，逐层执行、每层后重算 ratio、降压成功即停。
pub async fn run_compaction_cascade_v2(
    state: &mut ContextState,
    llm: &dyn LlmProvider,
    config: &ContextConfig,
    transcript_path: &Path,
    work_dir: &Path,
    session_id: &str,
) -> CascadeResult {
    let ratio_before = state.usage_ratio();
    let mut layers_executed = Vec::new();

    let persisted_results = layer0_persist_large_results(state, config, work_dir, session_id);
    if !persisted_results.is_empty() {
        layers_executed.push(0);
    }

    let mut params = determine_cascade_params(state, config);
    if !params.should_cascade {
        return CascadeResult {
            layers_executed,
            ratio_before,
            ratio_after: state.usage_ratio(),
            block_tool_calls: params.block_tool_calls,
            persisted_results,
        };
    }

    let reduced = compact_tool_results(state, params.m);
    if reduced > 0 {
        layers_executed.push(1);
    }
    params = determine_cascade_params(state, config);
    if !params.should_cascade {
        return CascadeResult {
            layers_executed,
            ratio_before,
            ratio_after: state.usage_ratio(),
            block_tool_calls: params.block_tool_calls,
            persisted_results,
        };
    }

    if state.compaction_consecutive_failures < 3 {
        let _ = run_compaction(state, llm, config, transcript_path, params.m).await;
        layers_executed.push(2);
        params = determine_cascade_params(state, config);
        if !params.should_cascade {
            return CascadeResult {
                layers_executed,
                ratio_before,
                ratio_after: state.usage_ratio(),
                block_tool_calls: params.block_tool_calls,
                persisted_results,
            };
        }
    }

    if params.target_layer3 || state.compaction_consecutive_failures >= 3 {
        force_drop_oldest_to_target(state);
        layers_executed.push(3);
    }

    CascadeResult {
        layers_executed,
        ratio_before,
        ratio_after: state.usage_ratio(),
        block_tool_calls: params.block_tool_calls,
        persisted_results,
    }
}

/// Legacy 三层级联压缩（向后兼容，不使用 ratio 水位线）。
pub async fn run_compaction_cascade(
    state: &mut ContextState,
    llm: &dyn LlmProvider,
    config: &ContextConfig,
    transcript_path: &Path,
) {
    if state.is_over_budget() {
        compact_tool_results(state, config.keep_recent_turns);
    }
    if state.is_over_budget() {
        let _ = run_compaction_loop(state, llm, config, transcript_path).await;
    }
    if state.is_over_budget() {
        force_drop_oldest(state);
    }
}

// ---------------------------------------------------------------------------
// Helper: context overflow detection
// ---------------------------------------------------------------------------

/// 检测 LLM 错误消息是否表示 context overflow。
pub fn is_context_overflow_error(err: &str) -> bool {
    let lower = err.to_lowercase();
    lower.contains("context")
        && (lower.contains("length") || lower.contains("token") || lower.contains("limit"))
}
