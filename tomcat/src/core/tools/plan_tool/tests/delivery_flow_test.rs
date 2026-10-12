use super::common::*;

fn create_args(todos: Vec<create_plan::TodoArg>) -> create_plan::CreatePlanArgs {
    create_plan::CreatePlanArgs {
        goal: "simplify close out".into(),
        draft: "Implement the requested delivery flow.".into(),
        todos,
    }
}

#[test]
fn create_plan_keeps_only_llm_authored_todos_and_enforces_one_acceptance() {
    let _guard = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let runtime = PlanRuntime::new("session");
    runtime.enter_plan().unwrap();

    let out = create_plan::execute(
        &runtime,
        create_args(vec![
            create_plan::TodoArg {
                id: "work".into(),
                content: "implement the change".into(),
                status: TodoStatus::Pending,
                kind: TodoKind::Work,
            },
            create_plan::TodoArg {
                id: "acceptance".into(),
                content: "验收：复核改动并回归验证".into(),
                status: TodoStatus::Pending,
                kind: TodoKind::Acceptance,
            },
        ]),
    )
    .unwrap();

    let plan = read_plan(&plan_path_for_id(out["plan_id"].as_str().unwrap()).unwrap()).unwrap();
    assert_eq!(plan.frontmatter.todos.len(), 2);
    assert_eq!(plan.frontmatter.todos[1].kind, TodoKind::Acceptance);

    let err = create_plan::execute(
        &runtime,
        create_args(vec![
            create_plan::TodoArg {
                id: "acceptance-a".into(),
                content: "验收 A".into(),
                status: TodoStatus::Pending,
                kind: TodoKind::Acceptance,
            },
            create_plan::TodoArg {
                id: "acceptance-b".into(),
                content: "验收 B".into(),
                status: TodoStatus::Pending,
                kind: TodoKind::Acceptance,
            },
        ]),
    )
    .expect_err("two acceptance todos must be rejected");
    assert!(err.to_string().contains(&crate::infra::i18n::tr_in(
        crate::infra::i18n::Locale::En,
        "planFile.acceptanceLimit",
        &[]
    )));
    cleanup_home(&home);
}

#[tokio::test]
async fn reviewer_feedback_can_add_one_acceptance_todo_and_trigger_verify_hint() {
    let _guard = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let runtime = PlanRuntime::new("session");
    runtime.enter_plan().unwrap();
    let out = create_plan::execute(
        &runtime,
        create_args(vec![create_plan::TodoArg {
            id: "work".into(),
            content: "implement the change".into(),
            status: TodoStatus::Pending,
            kind: TodoKind::Work,
        }]),
    )
    .unwrap();
    let plan_id = out["plan_id"].as_str().unwrap().to_string();
    mark_plan_executing(&runtime, &plan_id, "session");

    update_plan::execute(
        &runtime,
        update_plan::UpdatePlanArgs {
            plan_id: None,
            path: None,
            replace: false,
            ops: vec![update_plan::UpdateOp::Upsert {
                id: "acceptance".into(),
                content: Some("验收：复核改动并验证".into()),
                status: Some(TodoStatus::Pending),
                todo_kind: Some(TodoKind::Acceptance),
            }],
        },
    )
    .await
    .unwrap();

    let out = update_plan::execute(
        &runtime,
        update_plan::UpdatePlanArgs {
            plan_id: None,
            path: None,
            replace: false,
            ops: vec![update_plan::UpdateOp::SetStatus {
                id: "acceptance".into(),
                content: None,
                status: TodoStatus::InProgress,
            }],
        },
    )
    .await
    .unwrap();

    assert_eq!(out["next_step"]["phase"], "run_verify");
    assert!(out["next_step"]["hint"]
        .as_str()
        .unwrap()
        .contains("load_skill(verify)"));
    assert_eq!(
        read_plan(&plan_path_for_id(&plan_id).unwrap())
            .unwrap()
            .frontmatter
            .todos
            .iter()
            .find(|todo| todo.id == "acceptance")
            .unwrap()
            .kind,
        TodoKind::Acceptance
    );
    cleanup_home(&home);
}

#[tokio::test]
async fn update_plan_rejects_a_second_acceptance_and_completes_when_all_todos_are_terminal() {
    let _guard = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let runtime = PlanRuntime::new("session");
    let code_reviewer = std::sync::Arc::new(MockCodeReviewerDispatcher::new(Vec::new()));
    runtime.attach_code_reviewer(code_reviewer.clone());
    runtime.enter_plan().unwrap();
    let out = create_plan::execute(
        &runtime,
        create_args(vec![
            create_plan::TodoArg {
                id: "work".into(),
                content: "implement the change".into(),
                status: TodoStatus::Pending,
                kind: TodoKind::Work,
            },
            create_plan::TodoArg {
                id: "acceptance".into(),
                content: "验收：复核改动并验证".into(),
                status: TodoStatus::Pending,
                kind: TodoKind::Acceptance,
            },
        ]),
    )
    .unwrap();
    let plan_id = out["plan_id"].as_str().unwrap().to_string();
    mark_plan_executing(&runtime, &plan_id, "session");

    let err = update_plan::execute(
        &runtime,
        update_plan::UpdatePlanArgs {
            plan_id: None,
            path: None,
            replace: false,
            ops: vec![update_plan::UpdateOp::Upsert {
                id: "another-acceptance".into(),
                content: Some("验收：不应存在".into()),
                status: None,
                todo_kind: Some(TodoKind::Acceptance),
            }],
        },
    )
    .await
    .expect_err("a second acceptance todo must be rejected");
    assert!(err.to_string().contains(&crate::infra::i18n::tr_in(
        crate::infra::i18n::Locale::En,
        "planFile.acceptanceLimit",
        &[]
    )));

    update_plan::execute(
        &runtime,
        update_plan::UpdatePlanArgs {
            plan_id: None,
            path: None,
            replace: false,
            ops: vec![
                update_plan::UpdateOp::SetStatus {
                    id: "work".into(),
                    content: None,
                    status: TodoStatus::Completed,
                },
                update_plan::UpdateOp::SetStatus {
                    id: "acceptance".into(),
                    content: None,
                    status: TodoStatus::Completed,
                },
            ],
        },
    )
    .await
    .unwrap();

    assert_eq!(
        read_plan(&plan_path_for_id(&plan_id).unwrap())
            .unwrap()
            .frontmatter
            .state,
        PlanFileState::Completed
    );

    let reopened = update_plan::execute(
        &runtime,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id.clone()),
            path: None,
            replace: false,
            ops: vec![update_plan::UpdateOp::Upsert {
                id: "follow-up".into(),
                content: Some("address follow-up".into()),
                status: Some(TodoStatus::Pending),
                todo_kind: None,
            }],
        },
    )
    .await
    .unwrap();
    assert_eq!(reopened["plan_state_after"], "pending");
    assert!(reopened["warnings"][0]
        .as_str()
        .unwrap()
        .contains("reopened"));
    assert_eq!(
        code_reviewer
            .call_count
            .load(std::sync::atomic::Ordering::Relaxed),
        0,
        "todo completion must not dispatch the dormant code reviewer"
    );
    cleanup_home(&home);
}
