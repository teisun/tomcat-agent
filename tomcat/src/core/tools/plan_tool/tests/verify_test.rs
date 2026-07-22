use std::sync::atomic::Ordering;

use super::common::*;
use crate::core::plan_runtime::verify::{normalize_for_gate, VerifyCheck, VerifySummary};

#[tokio::test]
async fn update_plan_does_not_dispatch_dormant_verifier_even_when_attached() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    rt.set_max_code_review_rounds(0);
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

    assert!(out.get("verify").is_none(), "update_plan 不应再返回 verify 字段");
    assert_eq!(out["plan_state_after"], "completed");
    assert!(matches!(rt.mode(), PlanState::Chat));
    assert_eq!(verifier.call_count.load(Ordering::Relaxed), 0);
    cleanup_home(&home);
}

#[tokio::test]
async fn dispatch_verifier_without_dispatcher_returns_placeholder_summary() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");

    let summary = rt.dispatch_verifier(&plan_id).await;

    assert_eq!(summary.verdict, "aborted");
    assert_eq!(summary.verifier_stop_reason, "not_dispatched");
    assert!(summary.summary.contains("未注入"));
    cleanup_home(&home);
}

#[tokio::test]
async fn dispatch_verifier_uses_attached_dispatcher_when_called_explicitly() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    let verifier = std::sync::Arc::new(MockVerifierDispatcher::new(vec![fail_verify()]));
    rt.attach_verifier(verifier.clone());
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");

    let summary = rt.dispatch_verifier(&plan_id).await;

    assert_eq!(summary.verdict, "fail");
    assert_eq!(summary.summary, "unit verification failed");
    assert_eq!(verifier.call_count.load(Ordering::Relaxed), 1);
    cleanup_home(&home);
}

#[tokio::test]
async fn write_verify_transcript_keeps_plan_verify_event_available_for_future_restart() {
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
    let plan_id = fresh_planning_plan(&rt);
    let mut summary = VerifySummary {
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
    };

    let warnings = normalize_for_gate(&mut summary);
    rt.write_verify_transcript(&plan_id, &summary);

    let events = captured.lock();
    let verify_event = events
        .iter()
        .find(|v| v["event"] == "plan.verify")
        .expect("缺少 plan.verify 事件");
    assert!(warnings.iter().any(|w| w.contains("降级为 skip")));
    assert_eq!(summary.verdict, "partial");
    assert_eq!(summary.checks[0].result, "skip");
    assert_eq!(verify_event["plan_id"], plan_id);
    assert_eq!(verify_event["verdict"], "partial");
    assert_eq!(verify_event["checks"][0]["result"], "skip");
    cleanup_home(&home);
}

#[test]
fn normalize_for_gate_downgrades_empty_pass_command_and_marks_partial() {
    let mut summary = VerifySummary {
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
    };

    let warnings = normalize_for_gate(&mut summary);

    assert_eq!(summary.checks[0].result, "skip");
    assert_eq!(summary.verdict, "partial");
    assert!(warnings.iter().any(|w| w.contains("command 为空")));
    assert!(warnings
        .iter()
        .any(|w| w.contains("verdict 已降级为 partial")));
}

#[test]
fn normalize_for_gate_preserves_fail_verdict() {
    let mut summary = fail_verify();

    let warnings = normalize_for_gate(&mut summary);

    assert_eq!(summary.verdict, "fail");
    assert_eq!(summary.checks[0].result, "fail");
    assert!(warnings.is_empty());
}
