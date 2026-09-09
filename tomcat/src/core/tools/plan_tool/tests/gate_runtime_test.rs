use super::common::*;
use crate::core::plan_runtime::file_store::{
    TodoKind, GATE_ACCEPTANCE_TODO_ID, GATE_CODE_REVIEW_TODO_ID,
};

fn git_workspace_with_uncommitted_code() -> std::path::PathBuf {
    let workspace = std::env::temp_dir().join(format!(
        "tomcat_gate_runtime_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(workspace.join("src")).unwrap();
    std::fs::write(workspace.join("src/lib.rs"), "pub fn changed() {}\n").unwrap();
    assert!(std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(&workspace)
        .status()
        .unwrap()
        .success());
    workspace
}

fn args(plan_id: &str, ops: Vec<update_plan::UpdateOp>) -> update_plan::UpdatePlanArgs {
    update_plan::UpdatePlanArgs {
        plan_id: Some(plan_id.into()),
        path: None,
        replace: false,
        dispute_findings: Vec::new(),
        green_build_pass: None,
        green_build_evidence: Vec::new(),
        ops,
    }
}

fn complete_work_ops() -> Vec<update_plan::UpdateOp> {
    vec![
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
    ]
}

#[test]
fn create_plan_appends_two_runtime_owned_gates_to_disk_and_result() {
    let _guard = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let runtime = PlanRuntime::new("session-a");
    runtime.enter_plan().unwrap();

    let result = create_plan::execute(&runtime, good_args_with_todo()).unwrap();
    let items = result["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    assert_eq!(items[1]["id"], GATE_CODE_REVIEW_TODO_ID);
    assert_eq!(items[1]["kind"], "gate_code_review");
    assert_eq!(items[2]["id"], GATE_ACCEPTANCE_TODO_ID);
    assert_eq!(items[2]["kind"], "gate_acceptance");

    let plan = read_plan(&plan_path_for_id(result["plan_id"].as_str().unwrap()).unwrap()).unwrap();
    assert_eq!(plan.frontmatter.todos[1].kind, TodoKind::GateCodeReview);
    assert_eq!(plan.frontmatter.todos[2].kind, TodoKind::GateAcceptance);
    cleanup_home(&home);
}

#[tokio::test]
async fn work_completion_returns_start_review_until_visible_gate_starts() {
    let _guard = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let runtime = PlanRuntime::new("session-a");
    let plan_id = fresh_planning_plan(&runtime);
    mark_plan_executing(&runtime, &plan_id, "session-a");

    let out = update_plan::execute(&runtime, args(&plan_id, complete_work_ops()))
        .await
        .unwrap();

    assert_eq!(out["plan_state_after"], "executing");
    assert_eq!(out["next_step"]["phase"], "start_review");
    assert_eq!(out["items"][2]["id"], GATE_CODE_REVIEW_TODO_ID);
    assert_eq!(out["items"][2]["status"], "pending");
    cleanup_home(&home);
}

#[tokio::test]
async fn review_gate_rejects_early_or_direct_terminal_transitions() {
    let _guard = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let runtime = PlanRuntime::new("session-a");
    let plan_id = fresh_planning_plan(&runtime);
    mark_plan_executing(&runtime, &plan_id, "session-a");

    let early = update_plan::execute(
        &runtime,
        args(
            &plan_id,
            vec![update_plan::UpdateOp::SetStatus {
                id: GATE_CODE_REVIEW_TODO_ID.into(),
                content: None,
                status: TodoStatus::InProgress,
            }],
        ),
    )
    .await
    .expect_err("work todo remains pending");
    assert!(early.to_string().contains("every work todo"));

    let terminal = update_plan::execute(
        &runtime,
        args(
            &plan_id,
            vec![update_plan::UpdateOp::SetStatus {
                id: GATE_CODE_REVIEW_TODO_ID.into(),
                content: None,
                status: TodoStatus::Completed,
            }],
        ),
    )
    .await
    .expect_err("runtime alone may complete gates");
    assert!(terminal.to_string().contains("runtime-managed"));
    cleanup_home(&home);
}

#[tokio::test]
async fn docs_only_review_start_skips_both_gates_and_completes() {
    let _guard = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let runtime = PlanRuntime::new("session-a");
    let plan_id = fresh_planning_plan(&runtime);
    mark_plan_executing(&runtime, &plan_id, "session-a");

    update_plan::execute(&runtime, args(&plan_id, complete_work_ops()))
        .await
        .unwrap();
    let out = update_plan::execute(
        &runtime,
        args(
            &plan_id,
            vec![update_plan::UpdateOp::SetStatus {
                id: GATE_CODE_REVIEW_TODO_ID.into(),
                content: None,
                status: TodoStatus::InProgress,
            }],
        ),
    )
    .await
    .unwrap();

    assert_eq!(out["plan_state_after"], "completed");
    assert_eq!(out["next_step"]["phase"], "done");
    assert_eq!(out["items"][2]["status"], "completed");
    assert_eq!(out["items"][3]["status"], "completed");
    cleanup_home(&home);
}

#[tokio::test]
async fn failed_review_returns_implement_focused_instead_of_immediately_restarting_review() {
    let _guard = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let workspace = git_workspace_with_uncommitted_code();
    let runtime = PlanRuntime::new("session-a");
    runtime.attach_workspace_root(workspace.clone());
    let plan_id = fresh_planning_plan(&runtime);
    mark_plan_executing(&runtime, &plan_id, "session-a");
    runtime.set_max_code_review_rounds(1);
    runtime.attach_code_reviewer(std::sync::Arc::new(MockCodeReviewerDispatcher::new(vec![
        CodeReviewSummary {
            verdict: Some("fail".into()),
            findings: vec![Finding::new(
                "P1".into(),
                "runtime".into(),
                "missing regression coverage".into(),
            )],
            ..Default::default()
        },
    ])));

    update_plan::execute(&runtime, args(&plan_id, complete_work_ops()))
        .await
        .unwrap();
    let out = update_plan::execute(
        &runtime,
        args(
            &plan_id,
            vec![update_plan::UpdateOp::SetStatus {
                id: GATE_CODE_REVIEW_TODO_ID.into(),
                content: None,
                status: TodoStatus::InProgress,
            }],
        ),
    )
    .await
    .unwrap();

    assert_eq!(out["next_step"]["phase"], "implement_focused");
    assert!(out["next_step"]["hint"]
        .as_str()
        .is_some_and(|hint| hint.contains("F01: missing regression coverage")));
    assert!(out["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .all(|warning| !warning
            .as_str()
            .is_some_and(|text| text.contains("green_build_evidence"))));

    let persisted = read_plan(&plan_path_for_id(&plan_id).unwrap()).unwrap();
    assert_eq!(
        persisted.frontmatter.todos[2].status,
        TodoStatus::Pending,
        "failed review must reopen the visible gate"
    );
    let _ = std::fs::remove_dir_all(workspace);
    cleanup_home(&home);
}

#[tokio::test]
async fn code_edit_during_acceptance_after_review_budget_keeps_review_and_requires_fresh_acceptance(
) {
    let _guard = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let workspace = git_workspace_with_uncommitted_code();
    let runtime = PlanRuntime::new("session-a");
    runtime.attach_workspace_root(workspace.clone());
    let plan_id = fresh_planning_plan(&runtime);
    mark_plan_executing(&runtime, &plan_id, "session-a");
    runtime.set_max_code_review_rounds(1);
    runtime.attach_code_reviewer(std::sync::Arc::new(MockCodeReviewerDispatcher::new(vec![
        CodeReviewSummary {
            verdict: Some("pass".into()),
            ..Default::default()
        },
    ])));

    update_plan::execute(&runtime, args(&plan_id, complete_work_ops()))
        .await
        .unwrap();
    let passed = update_plan::execute(
        &runtime,
        args(
            &plan_id,
            vec![update_plan::UpdateOp::SetStatus {
                id: GATE_CODE_REVIEW_TODO_ID.into(),
                content: None,
                status: TodoStatus::InProgress,
            }],
        ),
    )
    .await
    .unwrap();
    assert_eq!(passed["next_step"]["phase"], "run_acceptance");

    std::thread::sleep(std::time::Duration::from_millis(10));
    std::fs::write(workspace.join("src/lib.rs"), "pub fn changed_again() {}\n").unwrap();
    let reopened = update_plan::execute(&runtime, args(&plan_id, Vec::new()))
        .await
        .unwrap();

    assert_eq!(reopened["next_step"]["phase"], "run_acceptance");
    assert!(reopened["code_review_pass"].as_bool().unwrap());
    assert!(!reopened["green_build_pass"].as_bool().unwrap());
    assert_eq!(reopened["items"][2]["status"], "completed");
    assert_eq!(reopened["items"][3]["status"], "pending");
    let _ = std::fs::remove_dir_all(workspace);
    cleanup_home(&home);
}

#[tokio::test]
async fn replace_reinjects_runtime_owned_gates() {
    let _guard = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let runtime = PlanRuntime::new("session-a");
    let plan_id = fresh_planning_plan(&runtime);
    mark_plan_executing(&runtime, &plan_id, "session-a");

    let out = update_plan::execute(
        &runtime,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id),
            path: None,
            replace: true,
            dispute_findings: Vec::new(),
            green_build_pass: None,
            green_build_evidence: Vec::new(),
            ops: vec![update_plan::UpdateOp::Upsert {
                id: "replacement".into(),
                content: Some("new work".into()),
                status: Some(TodoStatus::Pending),
            }],
        },
    )
    .await
    .unwrap();

    assert_eq!(out["items"].as_array().unwrap().len(), 3);
    assert_eq!(out["items"][1]["id"], GATE_CODE_REVIEW_TODO_ID);
    assert_eq!(out["items"][2]["id"], GATE_ACCEPTANCE_TODO_ID);
    assert!(out["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning
            .as_str()
            .is_some_and(|text| text.contains("preserved"))));
    cleanup_home(&home);
}

#[tokio::test]
async fn acceptance_before_review_pass_is_rejected() {
    let _guard = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let runtime = PlanRuntime::new("session-a");
    let plan_id = fresh_planning_plan(&runtime);
    mark_plan_executing(&runtime, &plan_id, "session-a");

    update_plan::execute(&runtime, args(&plan_id, complete_work_ops()))
        .await
        .unwrap();
    let error = update_plan::execute(
        &runtime,
        args(
            &plan_id,
            vec![update_plan::UpdateOp::SetStatus {
                id: GATE_ACCEPTANCE_TODO_ID.into(),
                content: None,
                status: TodoStatus::InProgress,
            }],
        ),
    )
    .await
    .expect_err("acceptance requires a completed review gate");

    assert!(error.to_string().contains("may start only after"));
    cleanup_home(&home);
}

#[tokio::test]
async fn green_build_without_acceptance_in_progress_is_rejected() {
    let _guard = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let runtime = PlanRuntime::new("session-a");
    let plan_id = fresh_planning_plan(&runtime);
    mark_plan_executing(&runtime, &plan_id, "session-a");

    let error = update_plan::execute(
        &runtime,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id),
            path: None,
            replace: false,
            ops: Vec::new(),
            dispute_findings: Vec::new(),
            green_build_pass: Some(true),
            green_build_evidence: Vec::new(),
        },
    )
    .await
    .expect_err("green-build evidence has no active acceptance gate");

    assert!(error
        .to_string()
        .contains("只能在 `[gate] Acceptance` 为 in_progress 时提交"));
    cleanup_home(&home);
}

#[tokio::test]
async fn acceptance_gate_rejects_direct_terminal_transitions() {
    let _guard = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let runtime = PlanRuntime::new("session-a");
    let plan_id = fresh_planning_plan(&runtime);
    mark_plan_executing(&runtime, &plan_id, "session-a");

    for status in [TodoStatus::Completed, TodoStatus::Cancelled] {
        let error = update_plan::execute(
            &runtime,
            args(
                &plan_id,
                vec![update_plan::UpdateOp::SetStatus {
                    id: GATE_ACCEPTANCE_TODO_ID.into(),
                    content: None,
                    status,
                }],
            ),
        )
        .await
        .expect_err("runtime alone may complete or cancel acceptance");
        assert!(error.to_string().contains("runtime-managed"));
    }
    cleanup_home(&home);
}

#[tokio::test]
async fn replace_rejects_runtime_gate_ids() {
    let _guard = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let runtime = PlanRuntime::new("session-a");
    let plan_id = fresh_planning_plan(&runtime);
    mark_plan_executing(&runtime, &plan_id, "session-a");

    let error = update_plan::execute(
        &runtime,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id),
            path: None,
            replace: true,
            dispute_findings: Vec::new(),
            green_build_pass: None,
            green_build_evidence: Vec::new(),
            ops: vec![update_plan::UpdateOp::Upsert {
                id: GATE_CODE_REVIEW_TODO_ID.into(),
                content: Some("attempt to replace runtime gate".into()),
                status: Some(TodoStatus::Pending),
            }],
        },
    )
    .await
    .expect_err("replace may not mention runtime gates");

    assert!(error.to_string().contains("may contain only work todos"));
    cleanup_home(&home);
}

#[tokio::test]
async fn one_update_may_start_only_one_close_out_gate() {
    let _guard = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let runtime = PlanRuntime::new("session-a");
    let plan_id = fresh_planning_plan(&runtime);
    mark_plan_executing(&runtime, &plan_id, "session-a");

    let error = update_plan::execute(
        &runtime,
        args(
            &plan_id,
            vec![
                update_plan::UpdateOp::SetStatus {
                    id: GATE_CODE_REVIEW_TODO_ID.into(),
                    content: None,
                    status: TodoStatus::InProgress,
                },
                update_plan::UpdateOp::SetStatus {
                    id: GATE_ACCEPTANCE_TODO_ID.into(),
                    content: None,
                    status: TodoStatus::InProgress,
                },
            ],
        ),
    )
    .await
    .expect_err("one call cannot start both close-out gates");

    assert!(error.to_string().contains("start one close-out gate"));
    cleanup_home(&home);
}
