use super::common::*;

#[tokio::test]
async fn verifier_spawned_on_all_completed() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    let verifier = std::sync::Arc::new(MockVerifierDispatcher::new(vec![ok_verify_pass()]));
    rt.attach_verifier(verifier.clone());
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

    assert_eq!(verifier.call_count.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert_eq!(out["verify"]["verdict"], "pass");
    assert_eq!(out["plan_mode_after"], "completed");
    cleanup_home(&home);
}

#[tokio::test]
async fn code_review_pass_runs_verifier_in_same_turn() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    rt.attach_reviewer(std::sync::Arc::new(MockReviewerDispatcher::new(vec![
        pass_code_review(),
    ])));
    let verifier = std::sync::Arc::new(MockVerifierDispatcher::new(vec![ok_verify_pass()]));
    rt.attach_verifier(verifier.clone());
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");
    rt.set_max_code_review_rounds(1);

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

    assert_eq!(out["code_review"]["verdict"], "pass");
    assert_eq!(out["verify"]["verdict"], "pass");
    assert_eq!(out["plan_mode_after"], "completed");
    assert_eq!(rt.code_review_rounds(&plan_id), 1);
    assert_eq!(verifier.call_count.load(std::sync::atomic::Ordering::Relaxed), 1);
    cleanup_home(&home);
}

#[tokio::test]
async fn code_review_non_pass_returns_to_main_and_rounds_exhaustion_skips_review() {
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
    let reviewer = std::sync::Arc::new(MockReviewerDispatcher::new(vec![fail_code_review()]));
    let verifier = std::sync::Arc::new(MockVerifierDispatcher::new(vec![ok_verify_pass()]));
    rt.attach_reviewer(reviewer.clone());
    rt.attach_verifier(verifier.clone());
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");
    rt.set_max_code_review_rounds(1);

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
    assert_eq!(first["code_review"]["verdict"], "fail");
    assert!(first["verify"].is_null());
    assert_eq!(first["plan_mode_after"], "executing");
    assert_eq!(reviewer.call_count.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert_eq!(verifier.call_count.load(std::sync::atomic::Ordering::Relaxed), 0);

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
    assert!(second["code_review"].is_null());
    assert_eq!(second["verify"]["verdict"], "pass");
    assert_eq!(second["plan_mode_after"], "completed");
    assert_eq!(rt.code_review_rounds(&plan_id), 1);
    assert_eq!(reviewer.call_count.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert_eq!(verifier.call_count.load(std::sync::atomic::Ordering::Relaxed), 1);

    let events = captured.lock();
    let warning = events
        .iter()
        .find(|v| v["event"] == "plan.code_review.warning")
        .expect("缺少 plan.code_review.warning");
    assert_eq!(warning["reason"], "rounds_exhausted");
    cleanup_home(&home);
}

#[tokio::test]
async fn code_review_transcript_matches_tool_result_after_normalization() {
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
    rt.attach_reviewer(std::sync::Arc::new(MockReviewerDispatcher::new(vec![
        ReviewSummary {
            kind: ReviewKind::Code,
            aborted: false,
            verdict: None,
            summary: "review finished without verdict".into(),
            changes_summary: "none".into(),
            applied_changes: false,
            ..Default::default()
        },
    ])));
    rt.attach_verifier(std::sync::Arc::new(MockVerifierDispatcher::new(vec![
        ok_verify_pass(),
    ])));
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");
    rt.set_max_code_review_rounds(1);

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

    assert_eq!(out["code_review"]["kind"], "code");
    assert_eq!(out["code_review"]["verdict"], "partial");
    let events = captured.lock();
    let code_review_event = events
        .iter()
        .find(|v| v["event"] == "plan.code_review")
        .expect("缺少 plan.code_review");
    assert_eq!(code_review_event["verdict"], out["code_review"]["verdict"]);
    assert_eq!(code_review_event["summary"], out["code_review"]["summary"]);
    cleanup_home(&home);
}
