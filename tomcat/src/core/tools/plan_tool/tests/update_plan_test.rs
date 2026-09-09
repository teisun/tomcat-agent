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
        evidence: Vec::new(),
        kind: Default::default(),
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
    rt.bind_plan_file_for_test(path.clone());

    let out = update_plan::execute(
        &rt,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id.clone()),
            path: None,
            replace: false,
            dispute_findings: Vec::new(),
            green_build_pass: None,
            green_build_evidence: Vec::new(),
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
    assert_eq!(items.len(), 4);
    assert_eq!(items[0]["status"], "in_progress");
    assert_eq!(items[1]["status"], "pending");
    assert!(out.get("path").is_some());
    assert!(out.get("panel_snapshot_id").is_some());
    assert_eq!(out["active_in_progress"], "t1");
    cleanup_home(&home);
}

#[tokio::test]
async fn work_todo_content_is_frozen_while_executing() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");

    let err = update_plan::execute(
        &rt,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id),
            path: None,
            replace: false,
            dispute_findings: Vec::new(),
            green_build_pass: None,
            green_build_evidence: Vec::new(),
            ops: vec![update_plan::UpdateOp::Upsert {
                id: "t1".into(),
                content: Some("silently broaden the approved task".into()),
                status: None,
            }],
        },
    )
    .await
    .expect_err("executing work content must be immutable");

    assert!(
        err.to_string().contains("content 已冻结") && err.to_string().contains("evidence"),
        "unexpected error: {err}"
    );
    cleanup_home(&home);
}

#[tokio::test]
async fn set_status_completed_accepts_evidence_and_persists_to_frontmatter() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");
    let evidence = vec!["cargo test -p tomcat --lib update_plan_test passed".to_string()];

    let out = update_plan::execute(
        &rt,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id.clone()),
            path: None,
            replace: false,
            dispute_findings: Vec::new(),
            green_build_pass: None,
            green_build_evidence: Vec::new(),
            ops: vec![update_plan::UpdateOp::SetStatus {
                id: "t1".into(),
                content: Some(evidence.clone().into()),
                status: TodoStatus::Completed,
            }],
        },
    )
    .await
    .unwrap();

    assert_eq!(out["items"][0]["evidence"], serde_json::json!(evidence));
    let plan = read_plan(&plan_path_for_id(&plan_id).unwrap()).unwrap();
    assert_eq!(plan.frontmatter.todos[0].evidence, evidence);
    assert!(!plan.body.contains("  - evidence:"));
    cleanup_home(&home);
}

#[tokio::test]
async fn planning_state_still_allows_content_rewrite() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    let plan_id = fresh_planning_plan(&rt);

    let out = update_plan::execute(
        &rt,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id),
            path: None,
            replace: false,
            dispute_findings: Vec::new(),
            green_build_pass: None,
            green_build_evidence: Vec::new(),
            ops: vec![update_plan::UpdateOp::Upsert {
                id: "t1".into(),
                content: Some("refined planning description".into()),
                status: None,
            }],
        },
    )
    .await
    .unwrap();

    assert_eq!(out["items"][0]["content"], "refined planning description");
    cleanup_home(&home);
}

#[tokio::test]
async fn completed_work_todo_without_evidence_produces_warning() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");

    let out = update_plan::execute(
        &rt,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id),
            path: None,
            replace: false,
            dispute_findings: Vec::new(),
            green_build_pass: None,
            green_build_evidence: Vec::new(),
            ops: vec![update_plan::UpdateOp::SetStatus {
                id: "t1".into(),
                content: None,
                status: TodoStatus::Completed,
            }],
        },
    )
    .await
    .unwrap();

    assert!(out["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning
            .as_str()
            .is_some_and(|text| text.contains("未记录 evidence"))));
    cleanup_home(&home);
}

#[tokio::test]
async fn update_plan_allows_two_independent_in_progress_todos() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    let plan_id = fresh_planning_plan(&rt);
    let path = plan_path_for_id(&plan_id).unwrap();
    let mut plan = read_plan(&path).unwrap();
    plan.frontmatter.state = PlanFileState::Executing;
    plan.frontmatter.session_key = Some("session-a".into());
    write_plan(&path, &plan, 2000).unwrap();
    rt.bind_plan_file_for_test(path.clone());

    let out = update_plan::execute(
        &rt,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id),
            path: None,
            replace: false,
            dispute_findings: Vec::new(),
            green_build_pass: None,
            green_build_evidence: Vec::new(),
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
    .expect("两个独立 todo 可并行推进");
    assert_eq!(out["items"][0]["status"], "in_progress");
    assert_eq!(out["items"][1]["status"], "in_progress");
    cleanup_home(&home);
}

#[tokio::test]
async fn update_plan_cross_session_allowed_for_planning_pending() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt_a = PlanRuntime::new("session-a");
    let plan_id = fresh_planning_plan(&rt_a);
    let rt_b = PlanRuntime::new("session-b");
    rt_b.enter_plan().unwrap();
    let out = update_plan::execute(
        &rt_b,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id),
            path: None,
            replace: false,
            dispute_findings: Vec::new(),
            green_build_pass: None,
            green_build_evidence: Vec::new(),
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
            dispute_findings: Vec::new(),
            green_build_pass: None,
            green_build_evidence: Vec::new(),
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
            todos: vec![
                TodoItem {
                    id: "t1".into(),
                    content: "step 1".into(),
                    status: TodoStatus::Pending,
                    evidence: Vec::new(),
                    kind: Default::default(),
                },
                TodoItem {
                    id: GATE_CODE_REVIEW_TODO_ID.into(),
                    content: GATE_CODE_REVIEW_TODO_CONTENT.into(),
                    status: TodoStatus::Pending,
                    evidence: Vec::new(),
                    kind: TodoKind::GateCodeReview,
                },
                TodoItem {
                    id: GATE_ACCEPTANCE_TODO_ID.into(),
                    content: GATE_ACCEPTANCE_TODO_CONTENT.into(),
                    status: TodoStatus::Pending,
                    evidence: Vec::new(),
                    kind: TodoKind::GateAcceptance,
                },
            ],
            green_build_pass: false,
            green_build_evidence: Vec::new(),
            code_review_pass: false,
            code_review_pass_at_ms: None,
            code_review_residual_findings: Vec::new(),
            completion_gate_cycles: 0,
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
            dispute_findings: Vec::new(),
            green_build_pass: None,
            green_build_evidence: Vec::new(),
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
    rt.bind_plan_file_for_test(path.clone());

    let out = update_plan::execute(
        &rt,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id.clone()),
            path: None,
            replace: false,
            dispute_findings: Vec::new(),
            green_build_pass: None,
            green_build_evidence: Vec::new(),
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
    assert_eq!(out["plan_state_after"], "executing");
    assert_eq!(out["next_step"]["phase"], "start_review");
    assert_eq!(rt.mode(), AgentMode::Plan);
    assert_eq!(rt.active_plan_path(), Some(path));
    cleanup_home(&home);
}

#[tokio::test]
async fn update_plan_reopen_completed_to_pending_and_emits_plan_pending() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    let events = std::sync::Arc::new(parking_lot::Mutex::new(Vec::<serde_json::Value>::new()));
    {
        let events = events.clone();
        rt.attach_transcript_appender(std::sync::Arc::new(move |extra| {
            events.lock().push(extra);
            Ok(())
        }));
    }
    let plan_id = fresh_planning_plan(&rt);
    let path = plan_path_for_id(&plan_id).unwrap();
    let mut plan = read_plan(&path).unwrap();
    plan.frontmatter.state = PlanFileState::Completed;
    plan.frontmatter.session_key = Some("session-a".into());
    plan.frontmatter.session_id = Some("sid-a".into());
    for todo in &mut plan.frontmatter.todos {
        todo.status = TodoStatus::Completed;
    }
    write_plan(&path, &plan, 2000).unwrap();
    rt.bind_plan_file_for_test(path.clone());

    let out = update_plan::execute(
        &rt,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id.clone()),
            path: None,
            replace: false,
            dispute_findings: Vec::new(),
            green_build_pass: None,
            green_build_evidence: Vec::new(),
            ops: vec![update_plan::UpdateOp::SetStatus {
                id: "t1".into(),
                content: None,
                status: TodoStatus::Pending,
            }],
        },
    )
    .await
    .unwrap();

    assert_eq!(out["plan_state_before"], "completed");
    assert_eq!(out["plan_state_after"], "pending");
    assert_eq!(rt.mode(), AgentMode::Plan);
    assert_eq!(rt.active_plan().unwrap().id, plan_id);
    assert_eq!(rt.active_plan_path(), Some(path.clone()));
    let persisted = read_plan(&path).unwrap();
    assert_eq!(persisted.frontmatter.state, PlanFileState::Pending);
    let event = events
        .lock()
        .iter()
        .find(|v| v["event"] == crate::infra::wire::WIRE_PLAN_PENDING)
        .cloned()
        .expect("缺少 plan.pending 事件");
    assert_eq!(event["plan_id"], plan_id);
    assert_eq!(event["state"], "pending");
    cleanup_home(&home);
}
