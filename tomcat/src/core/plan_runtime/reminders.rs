//! PLANNER / EXECUTOR `<system_reminder>` 常量。
//!
//! 文案统一来自 `core/prompts/templates/plan/*.txt`，通过 `include_str!` 编译期嵌入，
//! 运行期不再支持 env override。

use std::sync::LazyLock;

use crate::core::prompts::{load as load_prompt, render as render_prompt, PromptKey};

/// PLAN 模式系统提醒。
pub static PLANNER_REMINDER: LazyLock<&'static str> =
    LazyLock::new(|| load_prompt(PromptKey::PlannerReminder));

/// 把 EXECUTOR reminder 模板的 `{plan_id}` 替换为实际值。
pub fn render_executor_reminder(plan_id: &str) -> String {
    render_prompt(PromptKey::ExecutorReminderFmt, &[("plan_id", plan_id)])
}
