use crate::core::plan_runtime::{review::Finding, NextAction};
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
fn executor_prompt_points_final_acceptance_to_verify_skill() {
    let rendered = load(PromptKey::ExecutorReminderFmt);
    assert!(rendered.contains("load_skill(verify)"));
    assert!(rendered.contains("按影响范围复核 diff 并验证"));
    assert!(!rendered.contains("[gate]"));
    assert!(!rendered.contains("green_build"));
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
    assert!(!s.contains("green-build evidence"));
}

#[test]
fn verification_and_planner_prompts_keep_focused_and_acceptance_roles_separate() {
    let verification = load(PromptKey::SystemVerification);
    let planner = load(PromptKey::PlannerReminder);

    assert!(
        verification.contains("Mid-work (including EXEC before final close-out): focused checks")
    );
    assert!(verification.contains("Final acceptance: once, when finishing the change or delivery"));
    assert!(verification.contains("Load the verify skill\n  (`load_skill(verify)`) and follow it"));
    assert!(!verification.contains("green_build_evidence"));

    assert!(planner.contains("one final todo with `kind=acceptance`"));
    assert!(planner.contains("intermediate verification todos use `kind=work`"));
    assert!(planner.contains("human-readable \"Acceptance\" section"));
}

/// Acceptance scope policy is written in the verify skill. Other prompts only
/// tell the executor to load it for final acceptance.
#[test]
fn acceptance_scope_policy_lives_only_in_the_verify_skill() {
    const VERIFY_SKILL: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/assets/skills/verify/SKILL.md"
    ));
    let verification = load(PromptKey::SystemVerification);
    let planner = load(PromptKey::PlannerReminder);
    let executor = load(PromptKey::ExecutorReminderFmt);
    let plan_review_brief = load(PromptKey::ReviewerPlanBrief);
    let code_review_brief = load(PromptKey::ReviewerCodeBrief);

    let forbidden = [
        "complete check set",
        "scope proportional",
        "Scale the checks",
        "every project check",
        "full acceptance suite",
    ];
    let navigating_surfaces = [
        ("verification", verification),
        ("planner", planner),
        ("executor", executor),
        ("plan review brief", plan_review_brief),
    ];
    for (name, text) in navigating_surfaces.iter().copied().chain([
        ("verify skill", VERIFY_SKILL),
        ("code review brief", code_review_brief),
    ]) {
        for phrase in forbidden {
            assert!(
                !text.contains(phrase),
                "{name} must not restate a superseded acceptance scope rule: {phrase:?}"
            );
        }
    }
    for (name, text) in navigating_surfaces {
        assert!(
            !text.contains("acceptance_commands"),
            "{name} must not resurrect the removed acceptance command contract"
        );
    }
    assert!(
        !code_review_brief.contains("acceptance_commands"),
        "code review brief must not mention removed acceptance-list duties"
    );

    let ladder_markers = ["Ladder:", "Ratchet:", "L0 ", "L3 "];
    for marker in ladder_markers {
        assert!(
            VERIFY_SKILL.contains(marker),
            "verify skill must define {marker:?}"
        );
        for (name, text) in navigating_surfaces {
            assert!(
                !text.contains(marker),
                "{name} must navigate to the verify skill instead of restating {marker:?}"
            );
        }
    }
    assert!(verification.contains("review the diff against the plan"));

    let next_actions = [
        NextAction::Done,
        NextAction::HandOff {
            reason: "test",
            open_findings: vec![Finding::new("P1".into(), "test".into(), "test".into())],
        },
        NextAction::RunVerify,
        NextAction::ContinueWork {
            remaining_work: vec!["- work (pending)".into()],
        },
    ];
    for action in next_actions {
        let instruction = action.instruction();
        for phrase in forbidden {
            assert!(
                !instruction.contains(phrase),
                "NextAction instruction must not restate a superseded acceptance scope rule: {phrase:?}"
            );
        }
    }
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

    assert!(normalized
        .contains("Give each problem or feature requirement its own self-contained section"));
    assert!(normalized.contains("with separate subsections in the following order"));
    let problem = section
        .lines()
        .find(|line| line.trim_start().starts_with("- For a problem:"))
        .expect("problems need their own subsection sequence")
        .trim();
    assert_eq!(
        problem,
        "- For a problem: \"Problem symptoms\", \"Root cause and evidence\", \"Solution\", and \"Verification\"."
    );
    let feature = section
        .lines()
        .find(|line| {
            line.trim_start()
                .starts_with("- For a feature requirement:")
        })
        .expect("feature requirements need their own subsection sequence")
        .trim();
    assert_eq!(
        feature,
        "- For a feature requirement: \"Requirement background\", \"Solution\", and \"Verification\"."
    );
    assert!(
        section.contains("Every Solution must contain a plain-language key decisions checklist")
    );
    assert!(normalized
        .contains("Do not list all problems first and then present all solutions together"));
    assert!(normalized.contains(
        "When creating or updating a development plan, write the complete explanation into the plan itself rather than only replying in the chat."
    ));
    assert!(normalized
        .contains("The key decisions checklist must name concrete implementation choices"));
    assert!(normalized.contains("exact files, symbols, or contracts to change"));
    assert!(normalized.contains("behavior before and after; and scope boundaries"));

    let checklist = section
        .split_once("\n\nThe key decisions checklist")
        .expect("planner must retain concrete implementation requirements")
        .1;
    assert!(
        checklist.lines().all(|line| line.len() <= 100),
        "Plan-specific prose should stay reviewable instead of collapsing into long lines"
    );
    assert!(!normalized.contains("root cause (or governing constraint for new work)"));

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
        "acceptance",
        "acceptance_commands",
        "applied_changes",
        "block",
        "cancelled",
        "cd",
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
        "todo_kind",
        "upsert",
        "verdict",
        "wait_ms",
        "work",
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
    assert!(s.contains("immediately follow it with a plain-language explanation"));
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
    assert!(s.contains("unit, integration, and E2E coverage"));
    assert!(s.contains("where each layer catches a\n  distinct failure type"));
    assert!(s.contains("verification batches as shared\n  build/test boundaries"));
    assert!(s.contains("Treat milestones as delivery boundaries"));
    assert!(s.contains("Source commands from the user, project rules, manifests"));
    // engineering-standards #6-8 在 planner 的重申锚点。
    assert!(s.contains("first principles"));
    assert!(s.contains("ASCII diagram"));
    assert!(s.contains("Put user experience first"));
}

/// S6/S8 remain shared. Core identity keeps general explanation rules, while
/// planner adds plan structure; the reviewer templates are unchanged in this update.
#[test]
fn standards_6_7_8_follow_shared_and_role_specific_contracts() {
    const S6: &str = "Reason from first principles: when planning or coding, work out the architecture and implementation from first principles, follow best practices, pursue the most elegant solution, and dare to overturn a flawed technical design rather than patch around it.";
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
    let common_explanation_rules = [
        "Explain problems and technical solutions in a way that is easy to read and understand:",
        "When explaining a problem, solution, or plan, provide an overall ASCII diagram by default to show the big picture.",
        "Do not explain problems or solutions using opaque jargon or strings of technical terms. When a technical term is necessary, immediately follow it with a plain-language explanation.",
        "Assume the reader knows nothing about the problems or code involved, and clearly explain all relevant background.",
    ];
    for (label, text) in [("core_identity", identity), ("planner", planner)] {
        for rule in common_explanation_rules {
            assert_eq!(
                text.matches(rule).count(),
                1,
                "{label} must preserve each shared explanation rule exactly once: {rule}"
            );
        }
        assert!(
            !text.contains("Explain in plain, jargon-free language"),
            "{label} must not retain the old explanation rule"
        );
    }
    assert_eq!(
        identity
            .matches(
                "All writing must use plain language—say it plainly, say it plainly, say it plainly."
            )
            .count(),
        1,
        "core_identity must retain the plain-language rule exactly once"
    );
    assert_eq!(
        identity
            .matches(
                "Include ASCII diagrams in complex sections to aid understanding. In the accompanying explanations, prioritize clear and complete communication while avoiding unnecessarily long-winded exposition."
            )
            .count(),
        1,
        "core_identity must retain the updated concise-explanation rule exactly once"
    );
    assert_eq!(
        planner
            .matches(
                "Include ASCII diagrams in complex sections to aid understanding. In the accompanying explanations, use concise wording that conveys all essential points clearly and fully, and avoid long-winded exposition."
            )
            .count(),
        1,
        "planner must retain its role-specific concise-explanation rule exactly once"
    );
    for plan_only in [
        "Give each problem or feature requirement its own self-contained section",
        "When creating or updating a development plan, write the complete explanation into the plan itself rather than only replying in the chat.",
    ] {
        assert!(planner.contains(plan_only));
        assert!(!identity.contains(plan_only));
    }
    assert!(reviewer_plan.contains("say it plainly"));
    assert!(
        !reviewer_code.contains("Explain in plain, jargon-free language"),
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
    assert!(planner.contains("Treat milestones as delivery boundaries"));
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
