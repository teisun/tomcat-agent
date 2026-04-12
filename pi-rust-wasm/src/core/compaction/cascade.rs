//! Layer 3: 强制删除最旧 turn（仅 API Context Overflow 后触发）。

use crate::core::session::manager::{estimate_turn_chars, ContextState};

/// Layer 3：强制删除最旧 turn 直到 ratio < 0.50。返回 `(删轮数, 删除字符数之和)`。
pub fn force_drop_oldest_to_target(state: &mut ContextState) -> (usize, usize) {
    // 必须先与「当前 turns + estimate_context_chars」对齐：若仍保留上一轮
    // `last_api_usage`，`estimated_token_count()` 会沿用巨大的 `prompt_tokens`，
    // 与正在 `remove(0)` 的 `user_turns_list` 脱节，`usage_ratio()` 长期 ≥ 0.5，
    // 会把 **全部** turn 删空；随后 `build_context_from_state` 为空，且 overflow
    // 重试路径用其覆盖 `messages`，还会丢掉本轮的 `System` 与末尾 `User`（见 chat 组装顺序）。
    state.invalidate_api_usage();
    let mut turns_removed = 0usize;
    let mut chars_removed = 0usize;
    while state.usage_ratio() >= 0.50 && !state.user_turns_list.is_empty() {
        let removed = state.user_turns_list.remove(0);
        let chars = estimate_turn_chars(&removed);
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
