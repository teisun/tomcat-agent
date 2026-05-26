use super::common::*;
use crate::core::tools::plan_tool::update_plan::rewrite_todos_board;

#[test]
fn rewrite_todos_board_replaces_between_markers() {
    let mut body =
        "## Todos Board\n\n<!-- todos-board:auto:begin -->\nOLD CONTENT\n<!-- todos-board:auto:end -->\n"
            .to_string();
    let todos = vec![TodoItem {
        id: "t1".into(),
        content: "step".into(),
        status: TodoStatus::InProgress,
    }];
    rewrite_todos_board(&mut body, &todos);
    assert!(!body.contains("OLD CONTENT"));
    assert!(body.contains("- [~] t1: step"));
    assert!(body.contains("todos-board:auto:begin"));
    assert!(body.contains("todos-board:auto:end"));
}

#[test]
fn rewrite_todos_board_noop_without_markers() {
    let original = "## Todos Board\n\nno markers here\n".to_string();
    let mut body = original.clone();
    rewrite_todos_board(&mut body, &[]);
    assert_eq!(body, original);
}

#[tokio::test]
async fn update_plan_set_status_returns_full_items_snapshot() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    rt.set_max_code_review_rounds(0);
    let plan_id = fresh_planning_plan(&rt);
    let path = plan_path_for_id(&plan_id).unwrap();
    let mut plan = read_plan(&path).unwrap();
    plan.frontmatter.state = PlanFileState::Executing;
    plan.frontmatter.session_key = Some("session-a".into());
    plan.frontmatter.session_id = Some("sid-a".into());
    write_plan(&path, &plan, 2000).unwrap();
    rt.set_executing_for_test(plan_id.clone());

    let out = update_plan::execute(
        &rt,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id.clone()),
            path: None,
            replace: false,
            ops: vec![update_plan::UpdateOp::SetStatus {
                id: "t1".into(),
                content: None,
                status: TodoStatus::InProgress,
            }],
        },
    )
    .await
    .unwrap();
    assert_eq!(out["plan_id"], plan_id);
    let items = out["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["status"], "in_progress");
    assert_eq!(items[1]["status"], "pending");
    assert!(out.get("path").is_some());
    assert!(out.get("panel_snapshot_id").is_some());
    assert_eq!(out["active_in_progress"], "t1");
    cleanup_home(&home);
}

#[tokio::test]
async fn update_plan_reuses_todos_op_engine_single_in_progress_violation() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    let plan_id = fresh_planning_plan(&rt);
    let path = plan_path_for_id(&plan_id).unwrap();
    let mut plan = read_plan(&path).unwrap();
    plan.frontmatter.state = PlanFileState::Executing;
    plan.frontmatter.session_key = Some("session-a".into());
    write_plan(&path, &plan, 2000).unwrap();
    rt.set_executing_for_test(plan_id.clone());

    let err = update_plan::execute(
        &rt,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id),
            path: None,
            replace: false,
            ops: vec![
                update_plan::UpdateOp::SetStatus {
                    id: "t1".into(),
                    content: None,
                    status: TodoStatus::InProgress,
                },
                update_plan::UpdateOp::SetStatus {
                    id: "t2".into(),
                    content: None,
                    status: TodoStatus::InProgress,
                },
            ],
        },
    )
    .await
    .expect_err("两个 in_progress 应被 ops 引擎拒");
    matches!(err, ToolError::Op(_));
    cleanup_home(&home);
}

#[tokio::test]
async fn update_plan_cross_session_allowed_for_planning_pending() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt_a = PlanRuntime::new("session-a");
    let plan_id = fresh_planning_plan(&rt_a);
    let rt_b = PlanRuntime::new("session-b");
    rt_b.enter_planning().unwrap();
    let out = update_plan::execute(
        &rt_b,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id),
            path: None,
            replace: false,
            ops: vec![update_plan::UpdateOp::Upsert {
                id: "t1".into(),
                content: Some("edited by b".into()),
                status: None,
            }],
        },
    )
    .await
    .unwrap();
    let items = out["items"].as_array().unwrap();
    assert_eq!(items[0]["content"], "edited by b");
    cleanup_home(&home);
}

#[tokio::test]
async fn update_plan_cross_session_rejected_for_executing() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt_a = PlanRuntime::new("session-a");
    let plan_id = fresh_planning_plan(&rt_a);
    let path = plan_path_for_id(&plan_id).unwrap();
    let mut plan = read_plan(&path).unwrap();
    plan.frontmatter.state = PlanFileState::Executing;
    plan.frontmatter.session_key = Some("session-a".into());
    plan.frontmatter.session_id = Some("sid-a".into());
    write_plan(&path, &plan, 2000).unwrap();

    let rt_b = PlanRuntime::new("session-b");
    let err = update_plan::execute(
        &rt_b,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id),
            path: None,
            replace: false,
            ops: vec![update_plan::UpdateOp::Upsert {
                id: "t1".into(),
                content: Some("intruder".into()),
                status: None,
            }],
        },
    )
    .await
    .expect_err("session-b 不应能写入 session-a 的 executing plan");
    matches!(err, ToolError::CrossSessionDenied(_));
    cleanup_home(&home);
}

#[tokio::test]
async fn update_plan_plan_id_prefers_active_external_path() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let workspace = tempfile::tempdir().unwrap();
    let external_path = workspace.path().join("external.plan.md");
    let plan = PlanFile {
        frontmatter: PlanFileFrontmatter {
            plan_id: "external_plan".into(),
            goal: "g".into(),
            state: PlanFileState::Planning,
            session_key: Some("session-a".into()),
            session_id: Some("sid-a".into()),
            created_at: "2026-05-24T00:00:00Z".into(),
            schema_version: 1,
            todos: vec![TodoItem {
                id: "t1".into(),
                content: "step 1".into(),
                status: TodoStatus::Pending,
            }],
            unknown: Default::default(),
        },
        body: "## Goal\nexternal\n".into(),
    };
    write_plan(&external_path, &plan, 2000).unwrap();

    let rt = PlanRuntime::new("session-a");
    rt.build_plan(&external_path.to_string_lossy(), Some("sid-a".into()))
        .unwrap();

    let out = update_plan::execute(
        &rt,
        update_plan::UpdatePlanArgs {
            plan_id: Some("external_plan".into()),
            path: None,
            replace: false,
            ops: vec![update_plan::UpdateOp::SetStatus {
                id: "t1".into(),
                content: None,
                status: TodoStatus::InProgress,
            }],
        },
    )
    .await
    .unwrap();

    let normalized_external_path =
        crate::infra::platform::normalize_path(external_path.to_string_lossy().as_ref()).unwrap();
    assert_eq!(
        out["path"],
        crate::infra::platform::format_home_path(&normalized_external_path)
    );
    let parsed = read_plan(&external_path).unwrap();
    assert_eq!(parsed.frontmatter.todos[0].status, TodoStatus::InProgress);
    cleanup_home(&home);
}

#[tokio::test]
async fn update_plan_in_exec_promotes_completed() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    let plan_id = fresh_planning_plan(&rt);
    let path = plan_path_for_id(&plan_id).unwrap();
    let mut plan = read_plan(&path).unwrap();
    plan.frontmatter.state = PlanFileState::Executing;
    plan.frontmatter.session_key = Some("session-a".into());
    plan.frontmatter.session_id = Some("sid-a".into());
    write_plan(&path, &plan, 2000).unwrap();
    rt.set_executing_for_test(plan_id.clone());

    let out = update_plan::execute(
        &rt,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id.clone()),
            path: None,
            replace: false,
            ops: vec![
                update_plan::UpdateOp::SetStatus {
                    id: "t1".into(),
                    content: None,
                    status: TodoStatus::Completed,
                },
                update_plan::UpdateOp::SetStatus {
                    id: "t2".into(),
                    content: None,
                    status: TodoStatus::Completed,
                },
            ],
        },
    )
    .await
    .unwrap();
    assert_eq!(out["plan_state_before"], "executing");
    assert_eq!(out["plan_state_after"], "completed");
    match rt.mode() {
        PlanMode::Completed { plan_id: cur } => assert_eq!(cur, plan_id),
        other => panic!("expected Completed, got {other:?}"),
    }
    cleanup_home(&home);
}
