use crate::core::prompts::{load, render, PromptKey};

#[test]
fn planner_prompt_mentions_create_plan_and_ask_question() {
    let s = load(PromptKey::PlannerReminder);
    assert!(s.contains("create_plan"));
    assert!(s.contains("ask_question"));
    assert!(s.contains("PLAN mode"));
}

#[test]
fn executor_prompt_renders_plan_id() {
    let rendered = render(
        PromptKey::ExecutorReminderFmt,
        &[("plan_id", "plan_demo_aaaa1111")],
    );
    assert!(rendered.contains("plan_demo_aaaa1111"));
    assert!(rendered.contains("update_plan"));
    assert!(rendered.contains("off-limits"));
}

#[test]
fn background_shell_prompt_mentions_finished_tag() {
    let s = load(PromptKey::SystemBackgroundShellMonitor);
    assert!(s.contains("<background-task-finished"));
    assert!(s.contains("task_output"));
    assert!(s.contains("wakeReason"));
    assert!(s.contains("Read `content`, `finished`, `exit_code`, and `wakeReason` together"));
    assert!(s.contains("Do not mindlessly loop forever"));
    assert!(!s.contains("Call `task_output(block=true)` again with the same `since` to keep waiting."));
}

#[test]
fn workspace_context_template_contains_placeholders() {
    let s = load(PromptKey::SystemWorkspaceContext);
    assert!(s.contains("{now}"));
    assert!(s.contains("{agent_workspace_dir}"));
    assert!(s.contains("{agent_trail_dir}"));
}
