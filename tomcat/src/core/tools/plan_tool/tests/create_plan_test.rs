use super::common::*;

#[test]
fn create_plan_rejected_outside_planning_returns_actionable_error() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    let args = create_plan::CreatePlanArgs {
        goal: "g".into(),
        draft: "d".into(),
        acceptance_commands: Vec::new(),
        todos: vec![create_plan::TodoArg {
            id: "t1".into(),
            content: "x".into(),
            status: TodoStatus::Pending,
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
        guidance: "Enter Plan mode before creating a plan.",
    };
    let rendered = error.to_string().to_ascii_lowercase();
    assert!(rendered.contains("不允许") || rendered.contains("not allowed"));
    assert!(
        !rendered.contains("invisible") && !rendered.contains("不可见"),
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
        acceptance_commands: Vec::new(),
        todos: vec![create_plan::TodoArg {
            id: "t1".into(),
            content: "first".into(),
            status: TodoStatus::Pending,
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
            acceptance_commands: Vec::new(),
            todos: vec![create_plan::TodoArg {
                id: "t1".into(),
                content: "first".into(),
                status: TodoStatus::Pending,
            }],
        },
    )
    .unwrap();
    let second = create_plan::execute(
        &rt,
        create_plan::CreatePlanArgs {
            goal: "second plan".into(),
            draft: "second draft".into(),
            acceptance_commands: Vec::new(),
            todos: vec![create_plan::TodoArg {
                id: "t1".into(),
                content: "second".into(),
                status: TodoStatus::Pending,
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
        acceptance_commands: Vec::new(),
        todos: vec![create_plan::TodoArg {
            id: "t1".into(),
            content: "first".into(),
            status: TodoStatus::Pending,
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

#[test]
fn create_plan_rejects_empty_goal() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    rt.enter_plan().unwrap();
    let args = create_plan::CreatePlanArgs {
        goal: "".into(),
        draft: "d".into(),
        acceptance_commands: Vec::new(),
        todos: vec![create_plan::TodoArg {
            id: "t1".into(),
            content: "x".into(),
            status: TodoStatus::Pending,
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
            acceptance_commands: Vec::new(),
            todos: vec![create_plan::TodoArg {
                id: "t1".into(),
                content: "x".into(),
                status: TodoStatus::Pending,
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
            acceptance_commands: Vec::new(),
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
fn create_plan_persists_normalized_acceptance_commands_and_tolerates_omission() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    rt.enter_plan().unwrap();

    let args = create_plan::CreatePlanArgs::from_json(&serde_json::json!({
        "goal": "declare acceptance",
        "draft": "body",
        "todos": [{ "id": "t1", "content": "first" }],
        "acceptance_commands": [
            "cd tomcat &&   cargo test --lib plan_tool",
            "   ",
            "cd tomcat && cargo test --lib plan_tool",
            "cd tomcat-vscode-ext && npm test"
        ],
    }))
    .expect("acceptance_commands is an accepted create_plan field");
    let out = create_plan::execute(&rt, args).expect("create_plan OK");
    let plan_id = out["plan_id"].as_str().unwrap().to_string();
    let plan = read_plan(&plan_path_for_id(&plan_id).unwrap()).unwrap();
    assert_eq!(
        plan.frontmatter.acceptance_commands,
        vec![
            "cd tomcat && cargo test --lib plan_tool".to_string(),
            "cd tomcat-vscode-ext && npm test".to_string(),
        ],
        "blank entries are dropped, whitespace collapsed, duplicates removed, order kept"
    );

    // Omitting the field keeps older callers working and leaves the list undeclared.
    let args = create_plan::CreatePlanArgs::from_json(&serde_json::json!({
        "goal": "no declaration",
        "draft": "body",
        "todos": [{ "id": "t1", "content": "first" }],
    }))
    .expect("acceptance_commands is optional");
    let out = create_plan::execute(&rt, args).expect("create_plan OK");
    let plan_id = out["plan_id"].as_str().unwrap().to_string();
    let plan = read_plan(&plan_path_for_id(&plan_id).unwrap()).unwrap();
    assert!(plan.frontmatter.acceptance_commands.is_empty());
    cleanup_home(&home);
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
