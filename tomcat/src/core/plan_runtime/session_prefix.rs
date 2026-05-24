//! User-message 模式前缀（plan-runtime.md §4.1 R11、§5.3）。
//!
//! `[mode: PLAN]` / `[mode: EXEC plan_id=…]` 由 chat_loop 在装配 messages 阶段
//! **只**给发往 LLM 的那份副本贴前缀；**不**改 transcript 原文（避免 hydrate 后再次贴前缀）。
//! 见 §8 D5 防御 + 单测 `transcript_user_text_has_no_mode_prefix`。

use std::path::Path;

use super::mode::PlanMode;

/// 当前 mode 对应的 user-message 前缀（空字符串表示无前缀，CHAT 模式）。
///
/// 形如：
/// - Chat → `""`
/// - Planning → `"[mode: PLAN plan_path=~/.tomcat/plans/foo.plan.md]\n"`（无 active plan 时退化为 `[mode: PLAN]`）
/// - Executing { plan_id } → `"[mode: EXEC plan_id=<plan_id> plan_path=~/.tomcat/plans/foo.plan.md]\n"`
/// - Pending { plan_id } → `""`（pending 期不进 LLM 转，由 `/plan build` 后切 EXEC 才贴）
/// - Completed { .. } → `""`（自动收口后回 CHAT 语义，提示已通过 reminder swap 移除）
pub fn user_prefix_for_mode(mode: &PlanMode, plan_path: Option<&Path>) -> String {
    match mode {
        PlanMode::Planning => plan_path
            .map(|path| {
                format!(
                    "[mode: PLAN plan_path={}]\n",
                    crate::infra::platform::format_home_path(path)
                )
            })
            .unwrap_or_else(|| "[mode: PLAN]\n".to_string()),
        PlanMode::Executing { plan_id } => plan_path
            .map(|path| {
                format!(
                    "[mode: EXEC plan_id={plan_id} plan_path={}]\n",
                    crate::infra::platform::format_home_path(path)
                )
            })
            .unwrap_or_else(|| format!("[mode: EXEC plan_id={plan_id}]\n")),
        PlanMode::Chat | PlanMode::Pending { .. } | PlanMode::Completed { .. } => String::new(),
    }
}

/// 当 `text` 以一行 `[mode: PLAN]` / `[mode: EXEC plan_id=...]` 开头时，剥去该行（含末尾换行）。
///
/// 用于在 transcript 落盘前 sanitize：plan-runtime.md §5.3 D5 不变式要求 transcript 中
/// user 文本**不含** mode prefix；否则 hydrate 后再次贴前缀会出现 `[mode: PLAN][mode: PLAN]…`
/// 套娃（详见 §8 D5 防御）。
///
/// 实现谨慎：只剥首行符合 `^\[mode:` 且以 `]` 结束 + 一个换行的串；其它内容（含 `[mode]` 字符
/// 出现在正文别处）保持不动。
pub fn strip_user_prefix(text: &str) -> &str {
    if !text.starts_with("[mode:") {
        return text;
    }
    if let Some(eol) = text.find('\n') {
        let head = &text[..eol];
        if head.ends_with(']') {
            return &text[eol + 1..];
        }
    }
    text
}
