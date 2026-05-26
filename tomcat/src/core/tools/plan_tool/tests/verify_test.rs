use super::common::*;

#[tokio::test]
async fn verify_event_in_transcript() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    let captured: std::sync::Arc<parking_lot::Mutex<Vec<serde_json::Value>>> =
        std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
    {
        let sink = std::sync::Arc::clone(&captured);
        rt.attach_transcript_appender(std::sync::Arc::new(move |extra| {
            sink.lock().push(extra);
            Ok(())
        }));
    }
    rt.attach_verifier(std::sync::Arc::new(MockVerifierDispatcher::new(vec![
        ok_verify_pass(),
    ])));
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");

    let _ = update_plan::execute(
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

    let events = captured.lock();
    let verify_event = events
        .iter()
        .find(|v| v["event"] == "plan.verify")
        .expect("缺少 plan.verify 事件");
    assert_eq!(verify_event["plan_id"], plan_id);
    assert_eq!(verify_event["verdict"], "pass");
    cleanup_home(&home);
}

#[tokio::test]
async fn verify_event_matches_tool_result_after_normalization() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    let captured: std::sync::Arc<parking_lot::Mutex<Vec<serde_json::Value>>> =
        std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
    {
        let sink = std::sync::Arc::clone(&captured);
        rt.attach_transcript_appender(std::sync::Arc::new(move |extra| {
            sink.lock().push(extra);
            Ok(())
        }));
    }
    rt.attach_verifier(std::sync::Arc::new(MockVerifierDispatcher::new(vec![
        VerifySummary {
            checks: vec![VerifyCheck {
                name: "unit test".into(),
                command: String::new(),
                result: "pass".into(),
                output_excerpt: "claimed ok".into(),
            }],
            verdict: "pass".into(),
            summary: "claimed success".into(),
            verifier_turns_limit: 64,
            verifier_stop_reason: "completed".into(),
            ..Default::default()
        },
    ])));
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");

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

    let events = captured.lock();
    let verify_event = events
        .iter()
        .find(|v| v["event"] == "plan.verify")
        .expect("缺少 plan.verify");
    assert_eq!(out["verify"]["verdict"], "partial");
    assert_eq!(out["verify"]["checks"][0]["result"], "skip");
    assert_eq!(verify_event["verdict"], out["verify"]["verdict"]);
    assert_eq!(verify_event["checks"], out["verify"]["checks"]);
    assert_eq!(verify_event["summary"], out["verify"]["summary"]);
    cleanup_home(&home);
}

#[tokio::test]
async fn update_plan_tool_result_has_verify_field() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    rt.set_max_code_review_rounds(0);
    rt.attach_verifier(std::sync::Arc::new(MockVerifierDispatcher::new(vec![
        ok_verify_pass(),
    ])));
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");

    let out = update_plan::execute(
        &rt,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id),
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

    assert!(out.get("verify").is_some());
    assert_eq!(out["verify"]["summary"], "verification passed");
    cleanup_home(&home);
}

#[tokio::test]
async fn verify_gate_soft_does_not_block() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    rt.set_verify_gate_mode("soft");
    rt.attach_verifier(std::sync::Arc::new(MockVerifierDispatcher::new(vec![
        fail_verify(),
    ])));
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");

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

    assert_eq!(out["verify"]["verdict"], "fail");
    assert_eq!(out["plan_state_after"], "completed");
    match rt.mode() {
        PlanMode::Completed { plan_id: cur } => assert_eq!(cur, plan_id),
        other => panic!("expected Completed, got {other:?}"),
    }
    cleanup_home(&home);
}

#[tokio::test]
async fn verify_gate_allows_completed_on_partial() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    rt.set_verify_gate_mode("gate");
    rt.attach_verifier(std::sync::Arc::new(MockVerifierDispatcher::new(vec![
        partial_verify(),
    ])));
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");

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

    assert_eq!(out["verify"]["verdict"], "partial");
    assert_eq!(out["plan_state_after"], "completed");
    match rt.mode() {
        PlanMode::Completed { plan_id: cur } => assert_eq!(cur, plan_id),
        other => panic!("expected Completed, got {other:?}"),
    }
    cleanup_home(&home);
}

#[tokio::test]
async fn verify_gate_allows_completed_on_aborted() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    rt.set_verify_gate_mode("gate");
    rt.attach_verifier(std::sync::Arc::new(MockVerifierDispatcher::new(vec![
        aborted_verify("max_turns", "verifier hit turn budget"),
    ])));
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");

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

    assert_eq!(out["verify"]["verdict"], "aborted");
    assert_eq!(out["verify"]["verifier_stop_reason"], "max_turns");
    assert_eq!(out["plan_state_after"], "completed");
    cleanup_home(&home);
}

#[tokio::test]
async fn verify_gate_blocks_completed_on_fail() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    rt.set_verify_gate_mode("gate");
    rt.attach_verifier(std::sync::Arc::new(MockVerifierDispatcher::new(vec![
        fail_verify(),
    ])));
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");

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

    assert_eq!(out["plan_state_after"], "executing");
    let plan = read_plan(&plan_path_for_id(&plan_id).unwrap()).unwrap();
    assert!(matches!(plan.frontmatter.state, PlanFileState::Executing));
    cleanup_home(&home);
}

#[tokio::test]
async fn gate_fail_keeps_mode_executing_but_returns_verify() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    rt.set_verify_gate_mode("gate");
    rt.attach_verifier(std::sync::Arc::new(MockVerifierDispatcher::new(vec![
        fail_verify(),
    ])));
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");

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

    assert_eq!(out["verify"]["verdict"], "fail");
    assert_eq!(out["plan_state_after"], "executing");
    match rt.mode() {
        PlanMode::Executing { plan_id: cur } => assert_eq!(cur, plan_id),
        other => panic!("expected Executing, got {other:?}"),
    }
    cleanup_home(&home);
}

#[tokio::test]
async fn main_agent_can_reopen_todo_after_gate_fail() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    rt.set_verify_gate_mode("gate");
    rt.attach_verifier(std::sync::Arc::new(MockVerifierDispatcher::new(vec![
        fail_verify(),
    ])));
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");

    let _ = update_plan::execute(
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

    let reopen = update_plan::execute(
        &rt,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id),
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

    assert_eq!(reopen["plan_state_after"], "executing");
    assert_eq!(reopen["active_in_progress"], "t1");
    assert!(reopen["verify"].is_null());
    cleanup_home(&home);
}

#[tokio::test]
async fn gate_fail_then_recomplete_respawns_verifier() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    rt.set_verify_gate_mode("gate");
    let verifier = std::sync::Arc::new(MockVerifierDispatcher::new(vec![
        fail_verify(),
        ok_verify_pass(),
    ]));
    rt.attach_verifier(verifier.clone());
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");

    let first = update_plan::execute(
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
    assert_eq!(first["plan_state_after"], "executing");

    let reopen = update_plan::execute(
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
    assert!(reopen["verify"].is_null());

    let second = update_plan::execute(
        &rt,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id.clone()),
            path: None,
            replace: false,
            ops: vec![update_plan::UpdateOp::SetStatus {
                id: "t1".into(),
                content: None,
                status: TodoStatus::Completed,
            }],
        },
    )
    .await
    .unwrap();

    assert_eq!(
        verifier
            .call_count
            .load(std::sync::atomic::Ordering::Relaxed),
        2
    );
    assert_eq!(second["verify"]["verdict"], "pass");
    assert_eq!(second["plan_state_after"], "completed");
    cleanup_home(&home);
}
