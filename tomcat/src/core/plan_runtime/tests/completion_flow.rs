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
