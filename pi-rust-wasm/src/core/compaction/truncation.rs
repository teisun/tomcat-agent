//! Layer 0: 超大 tool result 落盘 + preview 占位符 & Layer 1: 占位符替换。

use std::path::Path;

use crate::core::session::manager::{ContextState, TurnEntry};
use crate::infra::config::ContextConfig;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub(super) const TOOL_RESULT_PLACEHOLDER: &str =
    "[Previous tool result replaced to save context space]";

const LAYER0_PREVIEW_CHARS: usize = 500;

const M_PROTECTED_TURNS: usize = 5;

pub(super) fn floor_char_boundary(s: &str, pos: usize) -> usize {
    if pos >= s.len() {
        return s.len();
    }
    let mut p = pos;
    while p > 0 && !s.is_char_boundary(p) {
        p -= 1;
    }
    p
}

// ---------------------------------------------------------------------------
// Layer 0 V2: Persist large tool results to disk
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct PersistedResult {
    pub tool_call_id: String,
    pub tool_name: String,
    pub original_chars: usize,
    pub persisted_path: String,
}

/// L0 步骤 A+B 的汇总（timing ⑤）。
#[derive(Debug, Clone, Default)]
pub struct Layer0CleanupOutcome {
    pub persisted: Vec<PersistedResult>,
    /// 落盘替换为 preview 后减少的字符数之和。
    pub persist_chars_freed: usize,
    /// compactable zone 占位符替换减少的字符数。
    pub placeholder_chars_freed: usize,
}

/// Layer 0 步骤 A：超大 tool result 落盘 + preview 占位符。
/// 仅扫描最后一个 UserTurn，单条 >= `layer0_single_result_max_chars` 时落盘。
pub fn layer0_persist_large_results(
    state: &mut ContextState,
    config: &ContextConfig,
    work_dir: &Path,
    session_id: &str,
) -> (Vec<PersistedResult>, usize) {
    let mut results = Vec::new();
    let mut persist_chars_freed = 0usize;
    let single_max = config.layer0_single_result_max_chars;

    let last_turn = match state.user_turns_list.last_mut() {
        Some(TurnEntry::UserTurn { messages, .. }) => messages,
        _ => return (results, persist_chars_freed),
    };

    for msg in last_turn.iter_mut() {
        if let crate::core::agent_loop::AgentMessage::ToolResult {
            tool_call_id,
            content,
            ..
        } = msg
        {
            if content.len() < single_max {
                continue;
            }
            if content.starts_with("[Tool result persisted:") {
                continue;
            }

            let persist_dir = work_dir
                .join("agents")
                .join(session_id)
                .join("tool-results");

            if std::fs::create_dir_all(&persist_dir).is_err() {
                continue;
            }

            let file_path = persist_dir.join(format!("{}.txt", tool_call_id));
            if std::fs::write(&file_path, content.as_bytes()).is_err() {
                continue;
            }

            let original_len = content.len();
            let path_str = file_path.to_string_lossy().to_string();

            let preview_end = floor_char_boundary(content, LAYER0_PREVIEW_CHARS);
            let preview = &content[..preview_end];
            let replacement = format!(
                "[Tool result persisted: {} ({} chars)]\nPreview: {}...",
                path_str, original_len, preview
            );

            let new_len = replacement.len();
            *content = replacement;

            let freed = original_len.saturating_sub(new_len);
            persist_chars_freed += freed;
            state.estimate_context_chars = state.estimate_context_chars.saturating_sub(freed);

            results.push(PersistedResult {
                tool_call_id: tool_call_id.clone(),
                tool_name: String::new(),
                original_chars: original_len,
                persisted_path: path_str,
            });
        }
    }
    (results, persist_chars_freed)
}

// ---------------------------------------------------------------------------
// Layer 1: Tool result placeholder replacement
// ---------------------------------------------------------------------------

/// Layer 1：从 compactable zone（排除最近 `m` 个 turns）中，
/// 将长度 **大于** `ContextConfig::layer0_placeholder_threshold_chars`（默认 10_000）的 tool result 替换为占位符。
pub fn compact_tool_results(state: &mut ContextState, config: &ContextConfig, m: usize) -> usize {
    let threshold = config.layer0_placeholder_threshold_chars;
    let len = state.user_turns_list.len();
    if len <= m {
        return 0;
    }
    let compactable_end = len - m;
    let mut total_reduced = 0usize;

    for turn in state.user_turns_list[..compactable_end].iter_mut() {
        if let TurnEntry::UserTurn { messages, .. } = turn {
            for msg in messages.iter_mut() {
                if let crate::core::agent_loop::AgentMessage::ToolResult { content, .. } = msg {
                    if content.len() <= threshold {
                        continue;
                    }
                    if content.starts_with("[Tool result persisted:")
                        || content == TOOL_RESULT_PLACEHOLDER
                    {
                        continue;
                    }
                    let old_len = content.len();
                    let reduced = old_len - TOOL_RESULT_PLACEHOLDER.len();
                    *content = TOOL_RESULT_PLACEHOLDER.to_string();
                    state.estimate_context_chars =
                        state.estimate_context_chars.saturating_sub(reduced);
                    total_reduced += reduced;
                }
            }
        }
    }
    total_reduced
}

// ---------------------------------------------------------------------------
// run_layer0_cleanup: Combined L0 persist + L1 placeholder (TASK-20)
// ---------------------------------------------------------------------------

/// TASK-20: L0 步骤 A（最后一 turn 落盘）+ 步骤 B（compactable zone 占位符替换）。
/// 在时机 ⑤（reasoning loop 最终 assistant 回复后）调用。
pub fn run_layer0_cleanup(
    state: &mut ContextState,
    config: &ContextConfig,
    work_dir: &Path,
    session_id: &str,
) -> Layer0CleanupOutcome {
    let (persisted, persist_chars_freed) =
        layer0_persist_large_results(state, config, work_dir, session_id);
    let placeholder_chars_freed = compact_tool_results(state, config, M_PROTECTED_TURNS);
    Layer0CleanupOutcome {
        persisted,
        persist_chars_freed,
        placeholder_chars_freed,
    }
}
