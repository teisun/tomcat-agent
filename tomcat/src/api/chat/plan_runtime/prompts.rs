//! PLANNER / EXECUTOR `<system_reminder>` 常量（plan-runtime.md §5.2 / F1 / F2 2026-05）。
//!
//! 由 `PlanRuntime::decorate_messages` 在装配阶段把对应 reminder 注入到 system message 尾部
//! （不进 transcript，每轮重新拼，避免历史污染）。
//!
//! 文案存在外置 `prompts/{planner,executor}.txt` 中，便于运维快速 review/diff；
//! `TOMCAT_PLANNER_REMINDER_OVERRIDE_PATH` / `TOMCAT_EXECUTOR_REMINDER_OVERRIDE_PATH`
//! 可在不重编译的前提下临时替换文案。
//!
//! `ask_question` 在 CHAT 模式也可用——本模块未提供 CHAT 专属 reminder（CHAT 不注入 reminder），
//! `ask_question` 的可用性靠 catalog + 工具自描述。

const PLANNER_FROZEN: &str = include_str!("prompts/planner.txt");
const EXECUTOR_FROZEN: &str = include_str!("prompts/executor.txt");

use std::sync::LazyLock;

/// PLAN 模式系统提醒。
pub static PLANNER_REMINDER: LazyLock<String> = LazyLock::new(|| {
    if let Ok(path) = std::env::var("TOMCAT_PLANNER_REMINDER_OVERRIDE_PATH") {
        if let Ok(s) = std::fs::read_to_string(&path) {
            return s;
        }
        eprintln!(
            "[tomcat plan_runtime] WARN: TOMCAT_PLANNER_REMINDER_OVERRIDE_PATH={path} 不可读，回落到内置 planner reminder"
        );
    }
    PLANNER_FROZEN.to_string()
});

/// EXEC 模式系统提醒模板（包含 `{plan_id}` 占位符）。
pub static EXECUTOR_REMINDER_FMT: LazyLock<String> = LazyLock::new(|| {
    if let Ok(path) = std::env::var("TOMCAT_EXECUTOR_REMINDER_OVERRIDE_PATH") {
        if let Ok(s) = std::fs::read_to_string(&path) {
            return s;
        }
        eprintln!(
            "[tomcat plan_runtime] WARN: TOMCAT_EXECUTOR_REMINDER_OVERRIDE_PATH={path} 不可读，回落到内置 executor reminder"
        );
    }
    EXECUTOR_FROZEN.to_string()
});

/// 把 EXECUTOR reminder 模板的 `{plan_id}` 替换为实际值。
pub fn render_executor_reminder(plan_id: &str) -> String {
    EXECUTOR_REMINDER_FMT.replace("{plan_id}", plan_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planner_reminder_mentions_create_plan_and_ask_question() {
        let s: &String = &PLANNER_REMINDER;
        assert!(s.contains("create_plan"));
        assert!(s.contains("ask_question"));
        assert!(s.contains("PLAN mode"));
    }

    #[test]
    fn executor_reminder_emphasizes_update_plan_and_forbids_raw_writes() {
        let rendered = render_executor_reminder("plan_demo_aaaa1111");
        assert!(rendered.contains("plan_demo_aaaa1111"));
        assert!(rendered.contains("update_plan"));
        assert!(rendered.contains("off-limits"));
    }
}
