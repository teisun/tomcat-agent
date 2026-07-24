use super::super::reminders;

#[test]
fn prompts_render_executor_reminder_substitutes_plan_id() {
    let s = reminders::render_executor_reminder("ship-001");
    assert!(s.contains("ship-001"));
    assert!(!s.contains("{plan_id}"));
}

#[test]
fn plan_enter_injects_planner_reminder_into_system() {
    let reminder: &str = *reminders::PLANNER_REMINDER;
    assert!(
        reminder.contains("<system_reminder") && reminder.contains("</system_reminder>"),
        "PLANNER_REMINDER 必须使用 <system_reminder ...> ... </system_reminder> 包裹，实际：\n{reminder}"
    );
    assert!(
        reminder.to_lowercase().contains("plan"),
        "PLANNER_REMINDER 必须显式提示当前在 PLAN/规划 模式，实际：\n{reminder}"
    );

    let composed = format!("BASE_SYSTEM_PROMPT\n{reminder}");
    assert!(composed.starts_with("BASE_SYSTEM_PROMPT"));
    assert!(composed.contains("<system_reminder"));
}

#[test]
fn executor_reminder_format_uses_system_reminder_tags() {
    let plan_id = "demo-plan-1";
    let s = reminders::render_executor_reminder(plan_id);
    assert!(
        s.contains("<system_reminder") && s.contains("</system_reminder>"),
        "EXECUTOR reminder 必须使用 <system_reminder ...> ... </system_reminder> 包裹，实际：\n{s}"
    );
    assert!(s.contains(plan_id), "EXECUTOR reminder 必须包含 plan_id");
}

#[test]
fn runtime_reminders_batch_detailed_todos_without_repeating_final_acceptance() {
    let planner: &str = *reminders::PLANNER_REMINDER;
    let executor = reminders::render_executor_reminder("batch-contract");

    assert!(planner.contains("not\nto the number of todos"));
    assert!(planner.contains("verification batches as shared\n  build/test boundaries"));
    assert!(planner.contains("share a build target"));
    assert!(planner.contains("milestone-level verification"));
    assert!(planner.contains(
        "final acceptance, run only checks not covered by a still-valid earlier\n  result"
    ));
    assert!(
        planner.contains("Do not schedule the same test family once per todo and again at the end")
    );
    assert!(
        executor.contains("Follow the building plan's verification scope, timing, and batching")
    );
}
