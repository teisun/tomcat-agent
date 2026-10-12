use super::super::reminders;

#[test]
fn prompts_render_executor_reminder_substitutes_plan_id() {
    let s = reminders::render_executor_reminder("ship-001");
    assert!(s.contains("ship-001"));
    assert!(!s.contains("{plan_id}"));
}

#[test]
fn planner_reminder_is_ready_for_ephemeral_tail_injection() {
    let reminder: &str = *reminders::PLANNER_REMINDER;
    assert!(
        reminder.contains("<system_reminder") && reminder.contains("</system_reminder>"),
        "PLANNER_REMINDER 必须使用 <system_reminder ...> ... </system_reminder> 包裹，实际：\n{reminder}"
    );
    assert!(
        reminder.to_lowercase().contains("plan"),
        "PLANNER_REMINDER 必须显式提示当前在 PLAN/规划 模式，实际：\n{reminder}"
    );

    assert!(
        reminder.trim_start().starts_with("<system_reminder"),
        "the pre-wrapped reminder can be appended as a synthetic user message"
    );
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
fn executor_reminder_contains_no_legacy_mode_wording() {
    let reminder = reminders::render_executor_reminder("migration-plan");
    assert!(
        !reminder.contains("EXEC") && !reminder.contains("CHAT"),
        "executor reminder must describe the plan lifecycle, not retired session modes: {reminder}"
    );
}

#[test]
fn runtime_reminders_point_final_acceptance_to_the_verify_skill() {
    let planner: &str = *reminders::PLANNER_REMINDER;
    let executor = reminders::render_executor_reminder("batch-contract");
    let verification =
        crate::core::prompts::load(crate::core::prompts::PromptKey::SystemVerification);

    assert!(planner.contains("not\nto the number of todos"));
    assert!(planner.contains("verification batches as shared\n  build/test boundaries"));
    assert!(planner.contains("one final todo with `kind=acceptance`"));
    assert!(planner.contains("human-readable \"Acceptance\" section"));
    assert!(verification.contains(
        "For final acceptance, run only checks not covered by a still-valid earlier\nresult"
    ));
    assert!(verification
        .contains("Do not schedule the same test family once per todo and again at the end"));
    assert!(executor.contains("load_skill(verify)"));
}
