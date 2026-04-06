//! Layer 3: 强制删除最旧 turn（仅 API Context Overflow 后触发）。

use crate::core::session::manager::{estimate_turn_chars, ContextState};

/// Layer 3：强制删除最旧 turn 直到 ratio < 0.50。
pub fn force_drop_oldest_to_target(state: &mut ContextState) {
    while state.usage_ratio() >= 0.50 && !state.user_turns_list.is_empty() {
        let removed = state.user_turns_list.remove(0);
        let chars = estimate_turn_chars(&removed);
        state.estimate_context_chars = state.estimate_context_chars.saturating_sub(chars);
    }
    state.invalidate_api_usage();
}

/// 检测 LLM 错误消息是否表示 context overflow。
pub fn is_context_overflow_error(err: &str) -> bool {
    let lower = err.to_lowercase();
    lower.contains("context")
        && (lower.contains("length") || lower.contains("token") || lower.contains("limit"))
}
