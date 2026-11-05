use super::common::*;

#[test]
fn create_plan_rejected_outside_planning_returns_actionable_error() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    let args = create_plan::CreatePlanArgs {
        goal: "g".into(),
        draft: "d".into(),
        todos: vec![create_plan::TodoArg {
            id: "t1".into(),
            content: "x".into(),
            status: TodoStatus::Pending,
            kind: TodoKind::Work,
        }],
    };
    let err = create_plan::execute(&rt, args).expect_err("CHAT 模式应被拒");
    match err {
        ToolError::RejectedInMode {
            tool,
            mode,
            guidance,
        } => {
            assert_eq!(tool, "create_plan");
            assert_eq!(mode, "chat");
            assert!(guidance.contains("Plan"));
        }
        other => panic!("expected RejectedInMode, got {other:?}"),
    }
    cleanup_home(&home);
}

#[test]
fn rejection_message_does_not_claim_the_tool_is_invisible() {
    let error = ToolError::RejectedInMode {
        tool: "create_plan",
        mode: "chat".to_string(),
        guidance: "Enter Plan mode before creating a plan.".into(),
    };
    let rendered = error.to_string().to_ascii_lowercase();
    assert!(rendered.contains("not allowed"));
    assert!(
        !rendered.contains("invisible"),
        "the catalog is stable; rejection must describe policy, not visibility"
    );
}

#[test]
fn create_plan_in_planning_writes_disk_and_records_active_id() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    rt.enter_plan().unwrap();
    let args = create_plan::CreatePlanArgs {
        goal: "为 chat 补齐 plan 闭环".into(),
        draft: "step 1; step 2; step 3".into(),
        todos: vec![create_plan::TodoArg {
            id: "t1".into(),
            content: "first".into(),
            status: TodoStatus::Pending,
            kind: TodoKind::Work,
        }],
    };
    let out = create_plan::execute(&rt, args).expect("create_plan OK");
    let plan_id = out["plan_id"]
        .as_str()
        .expect("plan_id present")
        .to_string();
    assert!(plan_id.starts_with("plan_"));
    assert_eq!(out["state"], "planning");
    assert_eq!(out["review"]["aborted"], serde_json::Value::Bool(true));

    assert_eq!(
        rt.active_plan().as_ref().map(|plan| plan.id.as_str()),
        Some(plan_id.as_str())
    );
    assert_eq!(
        rt.active_plan().as_ref().map(|plan| plan.state),
        Some(PlanFileState::Planning)
    );
    let path = home
        .join(".tomcat")
        .join("plans")
        .join(format!("{plan_id}.plan.md"));
    assert!(path.exists(), "{path:?} 应该已写盘");
    let plan_text = std::fs::read_to_string(&path).expect("plan file readable");
    assert!(plan_text.contains("## Goal"));
    assert!(plan_text.contains("## Plan"));
    assert!(!plan_text.contains("## Draft"));
    assert!(!plan_text.contains("## Notes"));
    assert!(!plan_text.contains("## Review"));
    cleanup_home(&home);
}

#[test]
fn create_plan_multiple_times_overrides_the_active_plan() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    rt.enter_plan().unwrap();

    let first = create_plan::execute(
        &rt,
        create_plan::CreatePlanArgs {
            goal: "first plan".into(),
            draft: "first draft".into(),
            todos: vec![create_plan::TodoArg {
                id: "t1".into(),
                content: "first".into(),
                status: TodoStatus::Pending,
                kind: TodoKind::Work,
            }],
        },
    )
    .unwrap();
    let second = create_plan::execute(
        &rt,
        create_plan::CreatePlanArgs {
            goal: "second plan".into(),
            draft: "second draft".into(),
            todos: vec![create_plan::TodoArg {
                id: "t1".into(),
                content: "second".into(),
                status: TodoStatus::Pending,
                kind: TodoKind::Work,
            }],
        },
    )
    .unwrap();

    let first_id = first["plan_id"].as_str().unwrap();
    let second_id = second["plan_id"].as_str().unwrap();
    assert_ne!(first_id, second_id);
    assert_eq!(
        rt.active_plan().as_ref().map(|plan| plan.id.as_str()),
        Some(second_id)
    );
    assert_eq!(
        rt.active_plan_path(),
        Some(plan_path_for_id(second_id).unwrap()),
        "create_plan 应记录最新 planning plan 的真实 path"
    );
    assert!(plan_path_for_id(first_id).unwrap().is_file());
    assert!(plan_path_for_id(second_id).unwrap().is_file());
    cleanup_home(&home);
}

#[test]
fn create_plan_normalizes_legacy_heading_wrapped_draft() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    rt.enter_plan().unwrap();
    let args = create_plan::CreatePlanArgs {
        goal: "Draft a minimal internal plan with two clear next steps.".into(),
        draft: "## Goal\n\nDraft a minimal internal plan.\n\n## Notes\n\nKeep scope small and actionable.\n"
            .into(),
        todos: vec![create_plan::TodoArg {
            id: "t1".into(),
            content: "first".into(),
            status: TodoStatus::Pending,
            kind: TodoKind::Work,
        }],
    };
    let out = create_plan::execute(&rt, args).expect("create_plan OK");
    let plan_id = out["plan_id"]
        .as_str()
        .expect("plan_id present")
        .to_string();
    let path = home
        .join(".tomcat")
        .join("plans")
        .join(format!("{plan_id}.plan.md"));
    let plan_text = std::fs::read_to_string(&path).expect("plan file readable");
    assert_eq!(plan_text.matches("## Goal").count(), 1);
    assert_eq!(plan_text.matches("## Plan").count(), 1);
    assert!(!plan_text.contains("## Draft") && !plan_text.contains("## Notes"));
    assert!(plan_text.contains("Keep scope small and actionable."));
    cleanup_home(&home);
}

// Exercise the real save/read path, including host headings and generated board.
fn assert_saved_draft(rt: &PlanRuntime, draft: &str, expected: &str) {
    let out = create_plan::execute(
        rt,
        create_plan::CreatePlanArgs {
            goal: "roundtrip".into(),
            draft: draft.into(),
            todos: vec![create_plan::TodoArg {
                id: "t1".into(),
                content: "save the complete body".into(),
                status: TodoStatus::Pending,
                kind: TodoKind::Work,
            }],
        },
    )
    .unwrap();
    let path = plan_path_for_id(out["plan_id"].as_str().unwrap()).unwrap();
    let saved = read_plan(&path).unwrap();
    let body = saved
        .body
        .strip_prefix("## Goal\n\nroundtrip\n\n## Plan\n\n")
        .unwrap();
    let (actual, board) = body.rsplit_once("\n\n## Todos Board\n\n").unwrap();
    assert_eq!(actual, expected);
    assert!(board.contains("<!-- todos-board:auto:begin -->"));
    assert!(board.contains("<!-- todos-board:auto:end -->"));
    assert!(board.contains(&crate::infra::i18n::tr("planTool.autoBoard", &[])));
    assert_eq!(saved.frontmatter.todos.len(), 1);
    assert_eq!(saved.frontmatter.todos[0].content, "save the complete body");
}

#[test]
fn create_plan_preserves_complete_body_heading_levels_and_spacing() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("body-roundtrip");
    rt.enter_plan().unwrap();
    let draft = format!(
        "Overview\n\n{}\n\n## Solution Breakdown and Implementation\n\n### Issue 1: 保留输入\n\n#### Solution\n\n##### Solution Explanation\n\n```text\n提交 -> 保存\n     +-> 失败：保留输入\n```\n\n| Decision | Reason |\n|---|---|\n| Keep input | Retry |\n\nT1 | Save fails\n- Scenario: edited form\n- Steps: fail write\n- Expected result: input stays\n- Files involved: save_test.rs (proposed)\n\n## Empty chapter\n\n\n## Notes\n\nKeep this ordinary title.\n\n### Older level-three chapter\n\n## Acceptance\n\nInput, headings, whitespace and order survive.  \nEnd.",
        "完整方案，不应被摘要化。\n".repeat(160)
    );
    assert!(draft.chars().count() > 2000);
    assert_saved_draft(&rt, &format!(" \n{draft}\n\n"), &draft);
    assert_saved_draft(
        &rt,
        "## Notes\n\nStandalone notes.",
        "## Notes\n\nStandalone notes.",
    );
    cleanup_home(&home);
}

#[test]
fn create_plan_legacy_wrappers_preserve_chapters_and_fenced_examples() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("legacy-roundtrip");
    rt.enter_plan().unwrap();
    for (open, close) in [
        ("```text", "```"),
        ("~~~markdown", "~~~"),
        ("````markdown", "````"),
        ("~~~text", ""),
    ] {
        let example =
            format!("{open}\n## Goal\n## Plan\n## Notes\n```\n## Still in example\n{close}");
        let content = format!("Keep detail.\n\n## Empty\n\n## Feature\n\n{example}");
        let draft = format!("## Goal\n\nroundtrip\n\n## Draft\n\n## Notes\n\n{content}");
        // The short backtick fence closes only the first case. A different
        // marker, shorter fence or absent closer must keep example titles literal.
        assert_saved_draft(&rt, &draft, content.trim());
    }
    assert_saved_draft(&rt, "## Plan\n\n## Empty\n", "## Empty");
    assert_saved_draft(
        &rt,
        "## Goal\n\nDifferent goal.\n\n## Notes\n\nKeep notes.",
        "Different goal.\n\n\nKeep notes.",
    );
    cleanup_home(&home);
}

#[test]
fn create_plan_rejects_empty_goal() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    rt.enter_plan().unwrap();
    let args = create_plan::CreatePlanArgs {
        goal: "".into(),
        draft: "d".into(),
        todos: vec![create_plan::TodoArg {
            id: "t1".into(),
            content: "x".into(),
            status: TodoStatus::Pending,
            kind: TodoKind::Work,
        }],
    };
    let err = create_plan::execute(&rt, args).expect_err("空 goal 应被拒");
    matches!(err, ToolError::BadArgs(_));
    cleanup_home(&home);
}

#[test]
fn create_plan_rejects_empty_draft_or_todos() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    rt.enter_plan().unwrap();
    let err = create_plan::execute(
        &rt,
        create_plan::CreatePlanArgs {
            goal: "g".into(),
            draft: "   ".into(),
            todos: vec![create_plan::TodoArg {
                id: "t1".into(),
                content: "x".into(),
                status: TodoStatus::Pending,
                kind: TodoKind::Work,
            }],
        },
    )
    .expect_err("空 draft 应被拒");
    matches!(err, ToolError::BadArgs(_));
    let err = create_plan::execute(
        &rt,
        create_plan::CreatePlanArgs {
            goal: "g".into(),
            draft: "d".into(),
            todos: vec![],
        },
    )
    .expect_err("空 todos 应被拒");
    matches!(err, ToolError::BadArgs(_));
    cleanup_home(&home);
}

#[test]
fn create_plan_from_json_rejects_legacy_plan_id_and_body() {
    let err = create_plan::CreatePlanArgs::from_json(&serde_json::json!({
        "plan_id": "x",
        "goal": "g",
        "draft": "d",
        "todos": [],
    }))
    .expect_err("旧字段 plan_id 应被拒");
    matches!(err, ToolError::BadArgs(_));
    let err = create_plan::CreatePlanArgs::from_json(&serde_json::json!({
        "goal": "g",
        "body": "old",
        "todos": [],
    }))
    .expect_err("旧字段 body 应被拒");
    matches!(err, ToolError::BadArgs(_));
}

#[test]
fn create_plan_rejects_removed_acceptance_commands_argument() {
    let err = create_plan::CreatePlanArgs::from_json(&serde_json::json!({
        "goal": "declare acceptance",
        "draft": "body",
        "todos": [{ "id": "t1", "content": "first" }],
        "acceptance_commands": ["cargo test"],
    }))
    .expect_err("removed field must not silently recreate the old floor");
    assert!(err.to_string().contains("unknown field"));
}

#[test]
fn create_plan_derived_id_passes_safety_check() {
    let id = create_plan::derive_plan_id("@#$%^");
    crate::core::plan_runtime::safety::assert_plan_id_safe(&id).unwrap();
    let id = create_plan::derive_plan_id("");
    crate::core::plan_runtime::safety::assert_plan_id_safe(&id).unwrap();
}

#[test]
fn create_plan_derived_id_collapses_underscore_runs() {
    let id = create_plan::derive_plan_id("test stuff --- md !!! html");
    assert!(id.starts_with("plan_test_stuff_md_html_"), "实际 id: {id}");
    assert!(!id.contains("___"), "不应出现连续下划线: {id}");
}
