//! Layer 0: 超大 tool result 落盘 + preview 占位符 & Layer 1: 占位符替换。

use std::path::Path;

use crate::core::session::manager::{ContextState, TurnEntry};
use crate::infra::config::ContextConfig;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub(super) const TRUNCATION_SUFFIX: &str = "\n\n[truncated — original content exceeded limit]";

pub(super) const TOOL_RESULT_PLACEHOLDER: &str =
    "[Previous tool result replaced to save context space]";

const LAYER1_TOOL_RESULT_THRESHOLD: usize = 20_000;

const LAYER0_PREVIEW_CHARS: usize = 500;

// ---------------------------------------------------------------------------
// Layer 0: Single tool result truncation (legacy fallback)
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct TruncationInfo {
    pub original_chars: usize,
    pub truncated_chars: usize,
}

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

/// Layer 0 fallback：若 `content` 超过 `max_chars` 则就地截断。
pub fn truncate_tool_result_if_needed(
    content: &mut String,
    max_chars: usize,
) -> Option<TruncationInfo> {
    if content.len() <= max_chars {
        return None;
    }
    let original_len = content.len();
    let safe_max = floor_char_boundary(content, max_chars);
    let safe_zone_start = floor_char_boundary(content, max_chars * 70 / 100);
    let cut_pos = content[safe_zone_start..safe_max]
        .rfind('\n')
        .map(|i| safe_zone_start + i)
        .unwrap_or(safe_max);
    content.truncate(cut_pos);
    content.push_str(TRUNCATION_SUFFIX);
    Some(TruncationInfo {
        original_chars: original_len,
        truncated_chars: content.len(),
    })
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

/// Layer 0 V2：超大 tool result 落盘 + preview 占位符。
/// 遍历最后一个 UserTurn 的 messages，满足条件 A（单条 >= threshold）
/// 或条件 B（单 turn 合计 >= aggregate threshold）时落盘。
pub fn layer0_persist_large_results(
    state: &mut ContextState,
    config: &ContextConfig,
    work_dir: &Path,
    session_id: &str,
) -> Vec<PersistedResult> {
    let mut results = Vec::new();
    let single_max = config.layer0_single_result_max_chars;
    let agg_max = config.layer0_turn_aggregate_max_chars;

    let last_turn = match state.user_turns_list.last_mut() {
        Some(TurnEntry::UserTurn { messages, .. }) => messages,
        _ => return results,
    };

    let total_tool_chars: usize = last_turn
        .iter()
        .filter_map(|m| {
            if let crate::core::agent_loop::AgentMessage::ToolResult { content, .. } = m {
                Some(content.len())
            } else {
                None
            }
        })
        .sum();

    let needs_aggregate = total_tool_chars >= agg_max;

    for msg in last_turn.iter_mut() {
        if let crate::core::agent_loop::AgentMessage::ToolResult {
            tool_call_id,
            content,
            ..
        } = msg
        {
            let should_persist = content.len() >= single_max
                || (needs_aggregate && content.len() > LAYER0_PREVIEW_CHARS);

            if !should_persist {
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
            state.estimate_context_chars = state.estimate_context_chars.saturating_sub(freed);

            results.push(PersistedResult {
                tool_call_id: tool_call_id.clone(),
                tool_name: String::new(),
                original_chars: original_len,
                persisted_path: path_str,
            });
        }
    }
    results
}

// ---------------------------------------------------------------------------
// Layer 1: Tool result placeholder replacement
// ---------------------------------------------------------------------------

/// Layer 1：从 compactable zone（排除最近 `m` 个 turns）中，
/// 将 > LAYER1_TOOL_RESULT_THRESHOLD 的 tool result 替换为占位符。
pub fn compact_tool_results(state: &mut ContextState, m: usize) -> usize {
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
                    if content.len() <= LAYER1_TOOL_RESULT_THRESHOLD {
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
