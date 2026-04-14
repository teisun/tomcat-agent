//! Layer 3: 强制删除最旧 turn（仅 API Context Overflow 后触发）。

use crate::core::llm::{ChatMessageRole, MessageKind};
use crate::core::session::manager::{estimate_msg_chars, ContextState};

/// Layer 3：强制删除最旧 turn 直到 ratio < 0.50。返回 `(删轮数, 删除字符数之和)`。
pub fn force_drop_oldest_to_target(state: &mut ContextState) -> (usize, usize) {
    // 必须先与「当前 messages + estimate_context_chars」对齐：若仍保留上一轮
    // `last_api_usage`，`estimated_token_count()` 会沿用巨大的 `prompt_tokens`，
    // 与正在 drain 的 `messages` 脱节，`usage_ratio()` 长期 ≥ 0.5，
    // 会把 **全部** messages 删空。
    state.invalidate_api_usage();
    let mut turns_removed = 0usize;
    let mut chars_removed = 0usize;

    while state.usage_ratio() >= 0.50 && !state.messages.is_empty() {
        // Find the end of the oldest turn: everything from the start up to (but not including)
        // the next turn-start boundary. A turn starts at a user/compaction message.
        let turn_end = state
            .messages
            .iter()
            .enumerate()
            .skip(1) // first message marks the start of the oldest turn
            .find(|(_, m)| {
                (m.role == ChatMessageRole::User && m.kind != MessageKind::Steering)
                    || m.kind == MessageKind::CompactionSummary
            })
            .map(|(i, _)| i)
            .unwrap_or(state.messages.len());

        let dropped: Vec<_> = state.messages.drain(..turn_end).collect();
        let chars: usize = dropped.iter().map(estimate_msg_chars).sum();
        chars_removed += chars;
        turns_removed += 1;
        state.estimate_context_chars = state.estimate_context_chars.saturating_sub(chars);
    }
    (turns_removed, chars_removed)
}

/// 检测 LLM 错误消息是否表示 context overflow。
pub fn is_context_overflow_error(err: &str) -> bool {
    let lower = err.to_lowercase();
    lower.contains("context")
        && (lower.contains("length") || lower.contains("token") || lower.contains("limit"))
}
