//! PLANNER / EXECUTOR `<system_reminder>` 常量（plan-runtime.md §5.2）。
//!
//! 由 `PlanRuntime::decorate_messages` 在装配阶段把对应 reminder 注入到 system message 尾部
//! （不进 transcript，每轮重新拼，避免历史污染）。

/// PLAN 模式系统提醒：聚焦"先想清再说"，鼓励 `create_plan` + `ask_question`，禁止直接执行代码。
pub const PLANNER_REMINDER: &str = "\n<system_reminder mode=\"planning\">\n你当前处于 PLAN 模式。先用 create_plan 把目标拆为 milestones + todos（落到 ~/.tomcat/plans/<plan_id>.plan.md），\n再用 ask_question 在关键决策点向用户单选/多选确认；不要直接调用 write/edit/bash/checkpoint 实施。\n用户输入 /plan exit 退回 Chat；用户输入 /plan build <plan_id> 后才进入 EXEC（你**不**能自行 build）。\n</system_reminder>\n";

/// EXEC 模式系统提醒：按 plan.todos 推进，单 in_progress，里程碑完成可记 checkpoint。
pub const EXECUTOR_REMINDER_FMT: &str = "\n<system_reminder mode=\"executing\" plan_id=\"{plan_id}\">\n你当前处于 EXEC 模式，正在推进 ~/.tomcat/plans/{plan_id}.plan.md。每完成一个 todo 用 update_plan 把它标记为 completed；\n单次只能有一个 todo 处于 in_progress。里程碑全 completed 时不必显式提示（runtime 自动打 checkpoint）。\n全部 todo 完成后 runtime 会派生 mode=completed 并复位提示。期间允许 read/write/edit/bash/search_files；create_plan / ask_question 在 EXEC 不可见。\n</system_reminder>\n";

/// 把 EXECUTOR reminder 模板的 `{plan_id}` 替换为实际值。
pub fn render_executor_reminder(plan_id: &str) -> String {
    EXECUTOR_REMINDER_FMT.replace("{plan_id}", plan_id)
}
