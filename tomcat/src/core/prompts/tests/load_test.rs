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
    assert!(
        !s.contains("Call `task_output(block=true)` again with the same `since` to keep waiting.")
    );
}

#[test]
fn workspace_context_template_contains_placeholders() {
    let s = load(PromptKey::SystemWorkspaceContext);
    assert!(s.contains("{now}"));
    assert!(s.contains("{agent_workspace_dir}"));
    assert!(s.contains("{agent_trail_dir}"));
}

#[test]
fn tool_instructions_template_uses_guidelines_placeholder_not_inline_rules() {
    let s = load(PromptKey::SystemToolInstructions);
    // 跨工具规则已下沉到 catalog.prompt_guidelines，模板只留框架句 + 占位符。
    assert!(
        s.contains("{tool_guidelines}"),
        "tool_instructions 应保留 {{tool_guidelines}} 占位符"
    );
    // 防双份复活：逐工具规则不得再内联在模板里。
    assert!(!s.contains("read(hashline=true) -> hashline_edit"));
    assert!(!s.contains("grep/find/ls -R"));
    assert!(!s.contains("Only claim you can access"));
}

#[test]
fn output_conventions_template_mentions_clickable_paths_and_forbidden_uris() {
    let s = load(PromptKey::SystemOutputConventions);
    assert!(s.contains("inline code"));
    assert!(s.contains("clickable file link"));
    assert!(s.contains("src/app.ts:42"));
    assert!(s.contains("file://"));
    assert!(s.contains("vscode://"));
    assert!(s.contains("【F:path†L1-L2】"));
}

#[test]
fn core_identity_has_operating_principles_and_tool_lines_placeholder() {
    let s = load(PromptKey::SystemCoreIdentity);
    assert!(s.contains("coding assistant"));
    assert!(s.contains("{tool_lines}"));
    assert!(s.contains("Operating principles:"));
    assert!(s.contains("Evidence first"));
    assert!(s.contains("Act, don't over-ask"));
    assert!(s.contains("no fabrication"));
    assert!(s.contains("first principles"));
    // #7 人话/ASCII 与 #8 UI 现常驻 core_identity。
    assert!(s.contains("plain, jargon-free language"));
    assert!(s.contains("ASCII diagram"));
    assert!(s.contains("Put user experience first"));
}

#[test]
fn parallel_tools_template_guides_batching() {
    let s = load(PromptKey::SystemParallelTools);
    assert!(s.contains("Parallel tool calls"));
    assert!(s.contains("single response"));
    assert!(s.contains("depends on"));
}

#[test]
fn verification_template_references_mini_verification_and_forbids_default_test() {
    let s = load(PromptKey::SystemVerification);
    assert!(s.contains("fabricate"));
    assert!(s.contains("Mini verification"));
    assert!(s.contains("P0-P6"));
    assert!(s.contains("npm test"));
}

#[test]
fn planner_prompt_prefers_thorough_decomposition_and_multi_perspective_tests() {
    let s = load(PromptKey::PlannerReminder);
    // engineering-standards #10：宁多勿少，取代原"避免过度拆解"。
    assert!(!s.contains("Avoid over-decomposition"));
    assert!(s.contains("err on the side of more"));
    // engineering-standards #9：多视角测试。
    assert!(s.contains("unit, integration, and E2E"));
    // engineering-standards #6-8 在 planner 的重申锚点。
    assert!(s.contains("first principles"));
    assert!(s.contains("ASCII diagram"));
    assert!(s.contains("Put user experience first"));
}

/// engineering-standards #6/#7/#8 必须在 core_identity、planner、两类 reviewer
/// 模板中一字不差出现，防止不同角色提示词逐渐漂移。
#[test]
fn standards_6_7_8_are_byte_identical_in_core_identity_planner_and_reviewers() {
    const S6: &str = "Reason from first principles: when planning or coding, work out the architecture and the implementation from first principles, pursue the most elegant solution, and dare to overturn a flawed technical design rather than patch around it.";
    const S7: &str = "Explain in plain, jargon-free language, assuming the reader knows nothing about the problem or the code; when explaining a design, a solution, or a root cause, include one overall ASCII diagram of the whole picture by default and add an ASCII diagram for each complex section; when you are creating or updating a development plan, write your full explanation into the plan itself rather than only replying in the chat.";
    const S8: &str = "Put user experience first: when a task involves UI, design it from the user's experience and above all follow the existing UI design conventions of the user's project.";

    let identity = load(PromptKey::SystemCoreIdentity);
    let planner = load(PromptKey::PlannerReminder);
    let reviewer_plan = load(PromptKey::ReviewerPlan);
    let reviewer_code = load(PromptKey::ReviewerCode);
    for (label, sentence) in [("S6", S6), ("S7", S7), ("S8", S8)] {
        assert!(
            identity.contains(sentence),
            "{label} 应逐字出现在 core_identity"
        );
        assert!(planner.contains(sentence), "{label} 应逐字出现在 planner");
        assert!(
            reviewer_plan.contains(sentence),
            "{label} 应逐字出现在 reviewer_plan"
        );
        assert!(
            reviewer_code.contains(sentence),
            "{label} 应逐字出现在 reviewer_code"
        );
    }
}
