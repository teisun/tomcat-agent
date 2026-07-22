use super::common::*;

#[tokio::test]
async fn code_review_pass_completes_without_verifier() {
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
    rt.attach_code_reviewer(std::sync::Arc::new(MockCodeReviewerDispatcher::new(vec![
        CodeReviewSummary {
            aborted: false,
            verdict: Some("pass".into()),
            summary: "code review passed".into(),
            changes_summary: "none".into(),
            applied_changes: false,
            findings: vec![crate::core::plan_runtime::review::Finding {
                severity: "suggestion".into(),
                area: "tests".into(),
                note: "nice to have".into(),
            }],
            ..Default::default()
        },
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
    assert_eq!(out["code_review"]["findings"][0]["area"], "tests");
    assert_eq!(out["plan_state_after"], "completed");
    assert!(out.get("verify").is_none());
    assert_eq!(rt.code_review_rounds(&plan_id), 1);
    assert_eq!(
        verifier
            .call_count
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
    let persisted = read_plan(&plan_path_for_id(&plan_id).unwrap()).unwrap();
    assert_eq!(persisted.frontmatter.state, PlanFileState::Completed);
    assert!(matches!(rt.mode(), PlanState::Chat));
    let events = captured.lock();
    let code_review_event = events
        .iter()
        .find(|v| v["event"] == "plan.code_review")
        .expect("缺少 plan.code_review");
    assert_eq!(code_review_event["verdict"], "pass");
    assert_eq!(code_review_event["findings"][0]["area"], "tests");
    assert_eq!(code_review_event["rounds"], 1);
    cleanup_home(&home);
}

#[tokio::test]
async fn aborted_code_review_best_effort_completes_without_verifier() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    let reviewer = std::sync::Arc::new(MockCodeReviewerDispatcher::new(vec![aborted_code_review(
        "reviewer spawn failed",
    )]));
    let verifier = std::sync::Arc::new(MockVerifierDispatcher::new(vec![ok_verify_pass()]));
    rt.attach_code_reviewer(reviewer.clone());
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

    assert_eq!(out["code_review"]["verdict"], "aborted");
    assert_eq!(out["plan_state_after"], "completed");
    assert!(out.get("verify").is_none());
    assert_eq!(
        reviewer
            .call_count
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    assert_eq!(
        verifier
            .call_count
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
    let warnings = out["warnings"].as_array().expect("warnings array");
    assert!(
        warnings.iter().any(|warning| {
            warning
                .as_str()
                .is_some_and(|text| text.contains("best-effort"))
        }),
        "aborted code review 应明确记录 best-effort completed 的 warning"
    );
    cleanup_home(&home);
}

#[tokio::test]
async fn code_review_non_pass_returns_to_main_and_rounds_exhaustion_completes_without_second_review() {
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
    let reviewer = std::sync::Arc::new(MockCodeReviewerDispatcher::new(vec![CodeReviewSummary {
        aborted: false,
        verdict: Some("fail".into()),
        summary: "code review found a concrete issue".into(),
        changes_summary: "none".into(),
        applied_changes: false,
        findings: vec![crate::core::plan_runtime::review::Finding {
            severity: "concern".into(),
            area: "logic".into(),
            note: "missing guard".into(),
        }],
        ..Default::default()
    }]));
    let verifier = std::sync::Arc::new(MockVerifierDispatcher::new(vec![ok_verify_pass()]));
    rt.attach_code_reviewer(reviewer.clone());
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
    assert_eq!(first["plan_state_after"], "executing");
    assert!(first.get("verify").is_none());
    assert_eq!(first["code_review"]["findings"][0]["note"], "missing guard");
    assert_eq!(first["items"].as_array().unwrap().len(), 2);
    assert!(first["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|item| !item["id"].as_str().unwrap_or_default().starts_with("cr_fix_")));
    assert_eq!(
        reviewer
            .call_count
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    assert_eq!(
        verifier
            .call_count
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
    let warnings = first["warnings"].as_array().expect("warnings array");
    assert!(warnings.iter().any(|warning| warning
        .as_str()
        .is_some_and(|text| text.contains("重新打开一个已有 todo"))));
    assert!(warnings.iter().any(|warning| warning
        .as_str()
        .is_some_and(|text| text.contains("新增一个修复 todo"))));

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
    assert!(reopen.get("verify").is_none());

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
    assert!(second.get("verify").is_none());
    assert_eq!(second["plan_state_after"], "completed");
    assert_eq!(rt.code_review_rounds(&plan_id), 1);
    assert_eq!(
        reviewer
            .call_count
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    assert_eq!(
        verifier
            .call_count
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
    let second_warnings = second["warnings"].as_array().expect("warnings array");
    assert!(second_warnings.iter().any(|warning| warning
        .as_str()
        .is_some_and(|text| text.contains("rounds 已用尽"))));

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
    rt.attach_code_reviewer(std::sync::Arc::new(MockCodeReviewerDispatcher::new(vec![
        CodeReviewSummary {
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

    assert_eq!(out["code_review"]["verdict"], "partial");
    let events = captured.lock();
    let code_review_event = events
        .iter()
        .find(|v| v["event"] == "plan.code_review")
        .expect("缺少 plan.code_review");
    assert_eq!(code_review_event["verdict"], out["code_review"]["verdict"]);
    assert_eq!(code_review_event["summary"], out["code_review"]["summary"]);
    assert_eq!(code_review_event["rounds"], 1);
    cleanup_home(&home);
}
