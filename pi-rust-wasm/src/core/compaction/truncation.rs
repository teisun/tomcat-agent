//! Layer 0: 超大 tool result 落盘 + preview 占位符 & Layer 1: 占位符替换。

use std::path::Path;

use crate::core::llm::{ChatMessageContent, ChatMessageRole, MessageKind};
use crate::core::session::manager::ContextState;
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
/// 仅扫描最后一个 UserTurn 内的 tool 消息，单条 >= `layer0_single_result_max_chars` 时落盘。
pub fn layer0_persist_large_results(
    state: &mut ContextState,
    config: &ContextConfig,
    work_dir: &Path,
    session_id: &str,
) -> (Vec<PersistedResult>, usize) {
    let mut results = Vec::new();
    let mut persist_chars_freed = 0usize;
    let single_max = config.layer0_single_result_max_chars;

    // Find the start of the last turn (last user/compaction boundary).
    let last_turn_start = state
        .messages
        .iter()
        .enumerate()
        .rev()
        .find(|(_, m)| {
            (m.role == ChatMessageRole::User && m.kind != MessageKind::Steering)
                || m.kind == MessageKind::CompactionSummary
        })
        .map(|(i, _)| i)
        .unwrap_or(state.messages.len());

    for msg in state.messages[last_turn_start..].iter_mut() {
        if msg.role != ChatMessageRole::Tool {
            continue;
        }

        let content = match &mut msg.content {
            Some(ChatMessageContent::Text(s)) => s,
            _ => continue,
        };

        if content.len() < single_max {
            continue;
        }
        if content.starts_with("[Tool result persisted:") {
            continue;
        }

        let tool_call_id = msg.tool_call_id.clone().unwrap_or_default();

        let persist_dir = work_dir
            .join("workspace")
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
            tool_call_id,
            tool_name: String::new(),
            original_chars: original_len,
            persisted_path: path_str,
        });
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

    // Find the start of the last M turns.
    let protected_start = find_protected_turn_start(&state.messages, m);
    if protected_start == 0 {
        return 0;
    }

    let mut total_reduced = 0usize;

    for msg in state.messages[..protected_start].iter_mut() {
        if msg.role != ChatMessageRole::Tool {
            continue;
        }

        let content = match &mut msg.content {
            Some(ChatMessageContent::Text(s)) => s,
            _ => continue,
        };

        if content.len() <= threshold {
            continue;
        }
        if content.starts_with("[Tool result persisted:") || content == TOOL_RESULT_PLACEHOLDER {
            continue;
        }

        let old_len = content.len();
        let reduced = old_len - TOOL_RESULT_PLACEHOLDER.len();
        *content = TOOL_RESULT_PLACEHOLDER.to_string();
        state.estimate_context_chars = state.estimate_context_chars.saturating_sub(reduced);
        total_reduced += reduced;
    }
    total_reduced
}

/// 返回「最后 m 个 turns」的起始消息索引（即第 (total_turns - m) 个 turn-start 的位置）。
/// 若 turns <= m，返回 0（整个 messages 列表均受保护）。
fn find_protected_turn_start(messages: &[crate::core::llm::ChatMessage], m: usize) -> usize {
    // Collect all turn-start indices in order.
    let turn_starts: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter(|(_, msg)| {
            (msg.role == ChatMessageRole::User && msg.kind != MessageKind::Steering)
                || msg.kind == MessageKind::CompactionSummary
        })
        .map(|(i, _)| i)
        .collect();

    let total_turns = turn_starts.len();
    if total_turns <= m {
        return 0;
    }

    turn_starts[total_turns - m]
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
