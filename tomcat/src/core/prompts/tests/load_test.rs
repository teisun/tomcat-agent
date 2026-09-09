use crate::core::prompts::{load, render, PromptKey};

#[test]
fn planner_prompt_mentions_create_plan_and_ask_question() {
    let s = load(PromptKey::PlannerReminder);
    assert!(s.contains("create_plan"));
    assert!(s.contains("ask_question"));
    assert!(s.contains("PLAN mode"));
    assert!(s.contains("requested plan/proposal creation or revision"));
    assert!(s.contains("New plan: use `create_plan`"));
    assert!(s.contains("Existing plan body or `## Goal`: call `read`, then use `edit`"));
    assert!(s.contains("Existing `frontmatter.todos`: use `update_plan`"));
    assert!(s.contains("Do NOT emit a plan/proposal as prose"));
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
    assert!(rendered.contains("content` is frozen"));
    assert!(rendered.contains("evidence"));
}

#[test]
fn executor_prompt_describes_visible_close_out_gates() {
    let rendered = load(PromptKey::ExecutorReminderFmt);
    assert!(rendered.contains("[gate] review"));
    assert!(rendered.contains("[gate] Acceptance"));
    assert!(rendered.contains("set `[gate] review` to in_progress"));
    assert!(rendered.contains("load_skill(verify)"));
    assert!(rendered.contains("next_step"));
    assert!(!rendered.contains("when ALL todos in the PlanFile flip to `completed`"));
}

#[test]
fn verification_prompt_requires_separating_regressions_from_pre_existing_failures() {
    let s = load(PromptKey::SystemVerification);
    assert!(s.contains("separate new regressions caused by your change"));
    assert!(s.contains("pre-existing failures and environment failures"));
    assert!(s.contains("never let one block verification of your own change"));
}

#[test]
fn verification_prompt_points_ui_changes_to_verify_skill() {
    let s = load(PromptKey::SystemVerification);
    assert!(s.contains("frontend, or VS Code webview changes"));
    assert!(s.contains("verify\nskill's UI acceptance guidance"));
    assert!(s.contains("PNG plus ARIA snapshot"));
    assert!(s.contains("green-build evidence"));
}

#[test]
fn verification_and_planner_prompts_keep_focused_and_acceptance_roles_separate() {
    let verification = load(PromptKey::SystemVerification);
    let planner = load(PromptKey::PlannerReminder);

    assert!(
        verification.contains("Mid-work (including EXEC before final close-out): focused checks")
    );
    assert!(verification.contains("Final acceptance: broader project checks once"));
    assert!(!verification.contains("green_build_evidence"));

    assert!(planner.contains("create_plan appends two gate todos"));
    assert!(planner.contains("Keep any milestone verification todo focused"));
    assert!(planner.contains("verification section in the plan body"));
}

#[test]
fn planner_prompt_carries_a_generic_plan_structure_section() {
    let s = load(PromptKey::PlannerReminder);
    let normalized = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let section = s
        .split_once("## Plan structure\n\n")
        .expect("planner should have a Plan structure section")
        .1
        .split_once("\n\n## Design and explanation standards")
        .expect("Plan structure should end before Design standards")
        .0;

    assert!(s.contains("## Plan structure"));
    assert!(normalized.contains("one self-contained section per substantive problem"));
    assert!(normalized.contains("separate subheadings in this order"));
    assert!(normalized.contains(
        "problem or requirement background with concrete evidence -> root cause (or governing constraint for new work) -> solution -> verification"
    ));
    assert!(normalized.contains("Never merge root cause and solution into the same subsection"));
    assert!(
        normalized.contains("plain language that a reader without the code context can understand")
    );
    assert!(normalized.contains("Do not use jargon, terminology dumps, or abstract slogans"));
    assert!(normalized
        .contains("when a technical term is necessary, explain it immediately in everyday words"));
    assert!(normalized
        .contains("Under solution, include an ASCII diagram that makes the design or flow clear"));
    assert!(normalized.contains("then a Key decisions checklist"));
    assert!(normalized.contains("exact files, symbols, or contracts to change"));
    assert!(normalized.contains("behavior before and after; boundaries; and explicit non-goals"));
    assert!(
        normalized.contains("Restating the goal or assigning responsibilities is not a solution")
    );
    assert!(normalized.contains("do not list every problem first and every solution afterwards"));

    assert!(section.contains("same subsection.\n\nExplain the background"));
    assert!(section.contains("everyday words.\n\nUnder solution"));
    assert!(section.contains("not a solution.\n\nKeep each problem"));
    assert!(
        section.lines().all(|line| line.len() <= 100),
        "Plan structure prose should stay reviewable instead of collapsing into long lines"
    );

    assert!(!s.contains(
        "Reason from first principles: an existing design may be overturned when it does not"
    ));
    assert!(s.contains("Reason from first principles: when planning or coding"));
}

#[test]
fn plan_review_prompt_challenges_the_design_without_becoming_a_gate() {
    let s = load(PromptKey::ReviewerPlan);
    assert!(s.contains("challenge the design itself from first principles"));
    assert!(s.contains("as simple and elegant as the"));
    assert!(s.contains("does the stated verification really prove the claim"));
    assert!(s.contains("Report gaps as `concern`"));
    // advisory-only 定位不变：不引入 verdict / 门禁。
    assert!(!s.contains("emit a verdict"));
}

#[test]
fn paged_reading_separates_persisted_results_from_fresh_files() {
    let s = load(PromptKey::SystemPagedReading);
    assert!(s.contains("Do NOT re-read the whole persisted result"));
    // 原文那句「不要整读」本意只针对已落盘结果，被当成通用读文件准则后
    // 直接催生了几百次 20-60 行的窄读。
    assert!(!s.contains("Do NOT re-read the entire file"));
    assert!(s.contains("one read costs one round-trip no matter how"));
    assert!(s.contains("300-1200"));
    assert!(s.contains("Repeatedly reading 20-60 line windows"));
}

#[test]
fn parallel_tools_gives_a_number_and_a_counter_example() {
    let s = load(PromptKey::SystemParallelTools);
    assert!(s.contains("4-8 independent read-only calls in one response"));
    assert!(
        s.contains("Five consecutive turns that each contain a single `read` is a failure mode")
    );
}

#[test]
fn planner_and_executor_gate_dispatch_agent_on_its_catalog_criteria() {
    let planner = load(PromptKey::PlannerReminder);
    let executor = load(PromptKey::ExecutorReminderFmt);

    assert!(planner
        .contains("use it to survey several modules in parallel without filling your own context"));
    assert!(
        planner.contains("but use it only when the criteria in its tool description are satisfied")
    );
    assert!(planner.contains("NEVER mutate the codebase in PLAN"));
    assert!(planner.contains("File editing tools are restricted to"));

    assert!(executor.contains(
        "survey several modules in parallel and return findings instead of raw file contents"
    ));
    assert!(executor.contains(
        "but use `dispatch_agent` only when the criteria in its tool description are satisfied"
    ));
    assert!(!executor.contains(". but Use `dispatch_agent`"));
}

#[test]
fn dispatch_agent_is_described_rather_than_listed_bare() {
    let planner = load(PromptKey::PlannerReminder);
    let executor = load(PromptKey::ExecutorReminderFmt);
    for (label, text) in [("planner", planner), ("executor", executor)] {
        assert!(
            text.contains("read-only explorer subagents"),
            "{label} 应说明 dispatch_agent 是什么，而不是把它塞进一串工具名里"
        );
        assert!(
            !text.contains("`dispatch_agent`and"),
            "{label} 里的缺空格笔误应已修掉"
        );
    }
}

/// 幽灵工具守卫：模板里以反引号标出的工具名必须真的存在于 `BUILTIN_TOOL_CATALOG`。
///
/// 上一轮 `dispatch_agent` 就是这么混进 planner 提示词的 —— 提示词里写着，运行时
/// 根本没有，模型每次照做都会撞上一个不存在的工具。
#[test]
fn every_tool_named_in_a_template_exists_in_the_catalog() {
    /// 反引号里的非工具标识符：字段名、枚举值、外部命令。
    const NON_TOOL_IDENTIFIERS: &[&str] = &[
        "aborted",
        "applied_changes",
        "block",
        "cancelled",
        "changes_summary",
        "code_review",
        "code_review_pass",
        "completed",
        "concern",
        "content",
        "cwd",
        "evidence",
        "exit_code",
        "fail",
        "false",
        "finished",
        "goal",
        "green_build_pass",
        "in_progress",
        "log_path",
        "next_offset",
        "next_step",
        "none",
        "partial",
        "pass",
        "plan_id",
        "plan_ref",
        "pytest",
        "rg",
        "since",
        "task_id",
        "verdict",
        "wait_ms",
        "workspace_roots",
    ];

    fn collect_txt(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("templates dir readable") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                collect_txt(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "txt") {
                out.push(path);
            }
        }
    }

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/core/prompts/templates");
    let mut files = Vec::new();
    collect_txt(&root, &mut files);
    assert!(!files.is_empty(), "should have found template files");

    let mut checked: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for file in files {
        let text = std::fs::read_to_string(&file).expect("template readable");
        for token in text.split('`').skip(1).step_by(2) {
            let is_bare_identifier = !token.is_empty()
                && token.starts_with(|c: char| c.is_ascii_lowercase())
                && token
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
            if !is_bare_identifier || NON_TOOL_IDENTIFIERS.contains(&token) {
                continue;
            }
            assert!(
                crate::core::tools::contract::catalog::builtin_tool_by_name(token).is_some(),
                "{}: `{token}` 看起来像工具名，但 BUILTIN_TOOL_CATALOG 里没有它。\n                 要么这个工具不存在（幽灵工具，删掉或去实现它），\n                 要么它其实是字段名/枚举值，请加进 NON_TOOL_IDENTIFIERS。",
                file.display()
            );
            checked.insert(token.to_string());
        }
    }

    // 扫描器自己也要被扫描：抽不出 token 的实现同样会「全部通过」。
    for expected in ["read", "edit", "update_plan", "dispatch_agent"] {
        assert!(
            checked.contains(expected),
            "扫描器没有从模板里抽出 `{expected}`，说明它没在真正工作"
        );
    }
}

#[test]
fn background_shell_prompt_mentions_finished_tag() {
    let s = load(PromptKey::SystemBackgroundShellMonitor);
    assert!(s.contains("<background-task-finished"));
    assert!(s.contains("task_output"));
    assert!(s.contains("wakeReason"));
    assert!(s.contains("5 seconds to 10 minutes"));
    assert!(s.contains("Read `content`, `finished`, `exit_code`, and `wakeReason` together"));
    assert!(s.contains("Do not mindlessly loop forever"));
    assert!(
        !s.contains("Call `task_output(block=true)` again with the same `since` to keep waiting.")
    );
}

#[test]
fn workspace_context_template_starts_with_workspace_and_has_no_time_placeholder() {
    let s = load(PromptKey::SystemWorkspaceContext);
    assert!(s.starts_with("Agent workspace directory (agent_workspace_dir):"));
    assert!(!s.contains("{now}"));
    assert!(!s.contains("Current date and time"));
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
    assert!(s.contains("![mockup](docs/mockup.png)"));
    assert!(s.contains("use `![alt](path)` only"));
    assert!(s.contains("src/app.ts:42"));
    assert!(s.contains("src/app.ts:42:7"));
    assert!(s.contains("src/app.ts:59-103"));
    assert!(s.contains("src/app.ts#L9"));
    assert!(s.contains("workspace-relative path"));
    assert!(s.contains("Button.tsx"));
    assert!(s.contains("ChatMarkdown.tsx:172"));
    assert!(s.contains("http:"));
    assert!(s.contains("https:"));
    assert!(s.contains("data:"));
    assert!(s.contains("blob:"));
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
fn verification_template_contains_shared_discovery_and_reporting_rules() {
    let s = load(PromptKey::SystemVerification);
    assert!(s.contains("real completion"));
    assert!(s.contains("Never fabricate"));
    assert!(s.contains("ordinary CHAT changes"));
    assert!(s.contains("focused quick check proportional"));
    assert!(s.contains("In PLAN or EXEC"));
    assert!(s.contains("mode reminder and active plan"));
    assert!(s.contains("do not rerun the same command unchanged"));
    assert!(s.contains("`task_output`"));
    assert!(s.contains("`task_stop`"));
    assert!(s.contains("start as specific as possible"));
    for phase in ["P0:", "P1:", "P2:", "P3:", "P4:", "P5:"] {
        assert!(s.contains(phase), "verification should contain {phase}");
    }
    assert!(s.contains("separate new regressions caused by your change"));
    assert!(!s.contains("Mini verification"));
    assert!(s.contains("Cargo.toml"));
    assert!(s.contains("whole-workspace `npm test`, `cargo test`, or `pytest`"));
}

#[test]
fn negative_assertion_prompt_present() {
    let s = load(PromptKey::SystemVerification);
    assert!(s.contains("unfounded assertions are strictly prohibited."));
    assert!(s.contains("Rationalizing is a trigger"));
    assert!(s.contains("Cheapest evidence first"));
    assert!(s.contains("Negative claims need evidence"));
    assert!(s.contains("UNVERIFIED: could not find X"));
}

#[test]
fn planner_prompt_uses_precise_decomposition_and_multi_perspective_tests() {
    let s = load(PromptKey::PlannerReminder);
    const TODO_POLICY: &str = concat!(
        "6. Decompose thoroughly. For non-trivial work, break the work into one or more milestones, \n",
        "then precisely break each milestone down into detailed todos so nothing is missed and over-decomposition is avoided. \n",
        "However, for a simple, single-surface change, a flat linear todo list is sufficient."
    );

    assert!(s.contains(TODO_POLICY));
    assert!(!s.contains("Avoid over-decomposition"));
    assert!(!s.contains("Err on the side"));
    assert!(!s.contains("of more, smaller items"));
    // engineering-standards #9：按独特风险分层的多视角测试。
    assert!(s.contains("unit, integration, and\n  E2E"));
    assert!(s.contains("only where it catches a distinct failure type"));
    assert!(s.contains("verification batches as shared\n  build/test boundaries"));
    assert!(s.contains("share a build target"));
    assert!(s.contains("milestone-level verification"));
    assert!(s.contains("user, project rules, manifest, or documentation"));
    // engineering-standards #6-8 在 planner 的重申锚点。
    assert!(s.contains("first principles"));
    assert!(s.contains("ASCII diagram"));
    assert!(s.contains("Put user experience first"));
}

/// S6/S8 are shared by every actor, while S7 needs an edit-capable planning
/// surface and is deliberately absent from the read-only code reviewer.
#[test]
fn standards_6_7_8_are_scoped_and_byte_identical() {
    const S6: &str = "Reason from first principles: when planning or coding, work out the architecture and implementation from first principles, follow best practices, pursue the most elegant solution, and dare to overturn a flawed technical design rather than patch around it.";
    const S7: &str = "Explain in plain, jargon-free language, assuming the reader knows nothing about the problem or the code. Do not let jargon, terminology dumps, or abstract slogans replace an explanation; when a technical term is necessary, explain it immediately in everyday words. When explaining a design, a solution, or a root cause, include one overall ASCII diagram of the whole picture by default and add an ASCII diagram for each complex section; when you are creating or updating a development plan, write your full explanation into the plan itself rather than only replying in the chat.";
    const S8: &str = "Put user experience first: when a task involves UI, design it from the user's experience and above all follow the existing UI design conventions of the user's project.";

    let identity = load(PromptKey::SystemCoreIdentity);
    let planner = load(PromptKey::PlannerReminder);
    let reviewer_plan = load(PromptKey::ReviewerPlan);
    let reviewer_code = load(PromptKey::ReviewerCode);
    for (label, sentence) in [("S6", S6), ("S8", S8)] {
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
    for (label, text) in [
        ("core_identity", identity),
        ("planner", planner),
        ("reviewer_plan", reviewer_plan),
    ] {
        assert!(text.contains(S7), "S7 应逐字出现在 {label}");
    }
    assert!(
        !reviewer_code.contains(S7),
        "read-only code reviewer must not promise plan-file ASCII explanations"
    );
}

#[test]
fn verification_prompt_uses_p0_p5_evidence_without_ecosystem_specific_defaults() {
    let s = load(PromptKey::SystemVerification);
    for phase in ["P0:", "P1:", "P2:", "P3:", "P4:", "P5:"] {
        assert!(s.contains(phase), "executor should contain {phase}");
    }
    assert!(!s.contains("P6:"));
    assert!(s.contains("active plan or user request"));
    assert!(s.contains("nearest manifest"));
    assert!(s.contains("README, CONTRIBUTING, CI workflows"));
    assert!(s.contains("existing tests near the changed code"));
    assert!(s.contains("Never default to"));
    assert!(s.contains("whole-workspace `npm test`, `cargo test`, or `pytest`"));

    // Cargo.toml is one manifest example and cargo test appears only in the
    // cross-ecosystem prohibition; neither is a Rust-specific injected policy.
    assert!(s.contains("package.json, Cargo.toml, pyproject.toml"));
    assert!(!s.contains("Rust project"));
    assert!(!s.contains("Cargo workspace must"));
    assert!(!s.contains("run `cargo test` for Rust"));
}

#[test]
fn long_command_wait_expiry_keeps_same_task_without_unchanged_rerun_contract() {
    let verification = load(PromptKey::SystemVerification);
    let monitor = load(PromptKey::SystemBackgroundShellMonitor);

    assert!(verification.contains("foreground timeout or wait expires"));
    assert!(verification.contains("do not rerun the same command unchanged"));
    assert!(verification.contains("Inspect it with `task_output`"));
    assert!(monitor.contains("`task_id` + `log_path` immediately"));
    assert!(monitor.contains("task_output(task_id, since=..., block=true, wait_ms=...)"));
    assert!(monitor.contains("a progress check, **not** a failure"));
    assert!(monitor.contains("returned `next_offset`"));
    assert!(
        monitor.contains("use `task_stop` when stopping it is appropriate")
            || verification.contains("use `task_stop` when stopping it is appropriate")
    );
}

#[test]
fn planner_and_system_verification_share_one_scoped_batch_per_target_contract() {
    let planner = load(PromptKey::PlannerReminder);
    let verification = load(PromptKey::SystemVerification);

    assert!(planner.contains("verification batches as shared\n  build/test boundaries"));
    assert!(
        planner.contains("Group todos that share a build target into named verification batches")
    );
    assert!(planner.contains("milestone-level verification as the default granularity"));
    assert!(verification
        .contains("Do not schedule the same test family once per todo and again at the end"));
    assert!(verification
        .contains("Finish all related production and test edits before running that batch"));
    assert!(verification
        .contains("P0: commands and verification batches in the active plan or user request"));
    assert!(verification.contains("Prefer the project's own focused command and command order"));
}

#[test]
fn dynamic_time_regression_uses_plan_project_evidence_and_bounded_verification_contract() {
    let workspace = load(PromptKey::SystemWorkspaceContext);
    let verification = load(PromptKey::SystemVerification);
    let executor = load(PromptKey::ExecutorReminderFmt);

    assert!(!workspace.contains("{now}"));
    assert!(!workspace.contains("Current date and time"));
    assert!(verification
        .contains("P0: commands and verification batches in the active plan or user request"));
    assert!(verification.contains("P1: injected project rules"));
    assert!(verification.contains("P2: the nearest manifest"));
    assert!(verification.contains("Never default to"));
    assert!(verification.contains("do not rerun the same command unchanged"));

    let cargo_test_mentions = executor.matches("cargo test").count()
        + verification.matches("cargo test").count()
        + workspace.matches("cargo test").count();
    assert_eq!(
        cargo_test_mentions, 1,
        "Cargo test should appear only in the bounded anti-default example"
    );
}
