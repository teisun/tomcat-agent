use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;

use super::common::*;

#[tokio::test]
async fn create_plan_internally_dispatches_reviewer_with_real_summary() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    rt.attach_reviewer(std::sync::Arc::new(MockReviewerDispatcher::new(vec![
        ok_review(),
    ])));
    rt.enter_planning().unwrap();
    let out = create_plan::execute_with_reviewer(&rt, good_args_with_todo(), false)
        .await
        .unwrap();
    assert!(out["plan_id"].as_str().unwrap().starts_with("plan_"));
    assert_eq!(out["review"]["aborted"], serde_json::Value::Bool(false));
    assert_eq!(out["review"]["summary"], "looks ok");
    cleanup_home(&home);
}

#[tokio::test]
async fn create_plan_succeeds_even_when_reviewer_aborts() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    rt.attach_reviewer(std::sync::Arc::new(MockReviewerDispatcher::new(vec![
        ReviewSummary::aborted_with("simulated parse error"),
    ])));
    rt.enter_planning().unwrap();
    let out = create_plan::execute_with_reviewer(&rt, good_args_with_todo(), false)
        .await
        .unwrap();
    let plan_id = out["plan_id"].as_str().unwrap().to_string();
    assert_eq!(out["review"]["aborted"], serde_json::Value::Bool(true));
    assert!(out["review"]["summary"].as_str().unwrap().contains("parse error"));
    let plan_path = home
        .join(".tomcat")
        .join("plans")
        .join(format!("{plan_id}.plan.md"));
    assert!(plan_path.exists());
    cleanup_home(&home);
}

#[tokio::test]
async fn create_plan_without_reviewer_returns_placeholder() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    rt.enter_planning().unwrap();
    let out = create_plan::execute_with_reviewer(&rt, good_args_with_todo(), false)
        .await
        .unwrap();
    assert_eq!(out["review"]["aborted"], serde_json::Value::Bool(true));
    assert!(out["review"]["summary"].as_str().unwrap().contains("P4 接入"));
    cleanup_home(&home);
}

#[tokio::test]
async fn dispatch_reviewer_releases_plan_lock_before_spawn() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");

    struct LockAcquiringMock;
    #[async_trait]
    impl ReviewerDispatcher for LockAcquiringMock {
        async fn dispatch(
            &self,
            plan_id: &str,
            _plan_text: &str,
            _kind: ReviewKind,
            _allow_review_edit: bool,
            _abort: std::sync::Arc<AtomicBool>,
        ) -> ReviewSummary {
            use crate::core::plan_runtime::file_store::{plan_path_for_id, with_advisory_lock};
            let path = plan_path_for_id(plan_id).unwrap();
            let lock_path = path.with_file_name(format!(
                "{}.lock",
                path.file_name().unwrap().to_string_lossy()
            ));
            let r = with_advisory_lock(&lock_path, 150, || Ok::<_, crate::core::plan_runtime::file_store::PlanError>(()));
            match r {
                Ok(()) => ReviewSummary {
                    aborted: false,
                    summary: "lock acquired by reviewer (write_plan 已释放)".into(),
                    changes_summary: "none".into(),
                    applied_changes: false,
                    ..Default::default()
                },
                Err(e) => ReviewSummary::aborted_with(format!("LockBusy: {e}")),
            }
        }
    }

    rt.attach_reviewer(std::sync::Arc::new(LockAcquiringMock));
    rt.enter_planning().unwrap();
    let out = create_plan::execute_with_reviewer(&rt, good_args_with_todo(), false)
        .await
        .unwrap();
    assert!(
        !out["review"]["aborted"].as_bool().unwrap(),
        "dispatch_reviewer 应能拿到 lock（说明 write_plan 已释放），实际：{:?}",
        out["review"]
    );
    cleanup_home(&home);
}

#[test]
fn create_plan_writes_transcript_plan_create_event() {
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

    rt.enter_planning().unwrap();
    let out = create_plan::execute(&rt, good_args_with_todo()).expect("create_plan OK");
    let plan_id = out["plan_id"].as_str().unwrap().to_string();

    let events = captured.lock();
    let plan_create = events
        .iter()
        .find(|v| v["event"] == "plan.create")
        .expect("缺少 plan.create 事件");
    assert_eq!(plan_create["plan_id"], plan_id);
    assert_eq!(plan_create["mode"], "planning");
    assert!(plan_create["path"].as_str().unwrap().ends_with(".plan.md"));
    cleanup_home(&home);
}

#[tokio::test]
async fn reviewer_summary_lands_in_transcript_plan_review() {
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

    let summary = ReviewSummary {
        aborted: false,
        summary: "ok".into(),
        changes_summary: "none".into(),
        applied_changes: false,
        reviewer_turns_used: 2,
        reviewer_turns_limit: 64,
        reviewer_stop_reason: "completed".into(),
        child_session_id: "child-1".into(),
        ..Default::default()
    };
    rt.attach_reviewer(std::sync::Arc::new(MockReviewerDispatcher::new(vec![
        summary,
    ])));
    rt.enter_planning().unwrap();
    let _ = create_plan::execute_with_reviewer(&rt, good_args_with_todo(), true)
        .await
        .unwrap();

    let events = captured.lock();
    let plan_review = events
        .iter()
        .find(|v| v["event"] == "plan.review")
        .expect("缺少 plan.review 事件");
    assert_eq!(plan_review["reviewer_turns_used"], 2);
    assert_eq!(plan_review["reviewer_turns_limit"], 64);
    assert_eq!(plan_review["reviewer_stop_reason"], "completed");
    assert!(plan_review["plan_id"].as_str().unwrap().starts_with("plan_"));
    cleanup_home(&home);
}

#[tokio::test]
async fn reviewer_writes_warning_event_on_second_round() {
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
        ok_review(),
        ok_review(),
    ])));
    rt.enter_planning().unwrap();
    let out1 = create_plan::execute_with_reviewer(&rt, good_args_with_todo(), true)
        .await
        .unwrap();
    let plan_id = out1["plan_id"].as_str().unwrap().to_string();
    let _ = rt.dispatch_reviewer(&plan_id, true).await;

    let events = captured.lock();
    let warning = events
        .iter()
        .find(|v| v["event"] == "plan.review.warning")
        .expect("第二轮应有 plan.review.warning");
    assert_eq!(warning["rounds"], 2);
    cleanup_home(&home);
}

#[tokio::test]
async fn reviewer_dispatch_passes_through_abort_signal() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");

    struct AbortPeekMock {
        saw_signal: std::sync::Arc<AtomicBool>,
    }
    #[async_trait]
    impl ReviewerDispatcher for AbortPeekMock {
        async fn dispatch(
            &self,
            _plan_id: &str,
            _plan_text: &str,
            _kind: ReviewKind,
            _allow_review_edit: bool,
            abort: std::sync::Arc<AtomicBool>,
        ) -> ReviewSummary {
            self.saw_signal.store(true, Ordering::Release);
            if abort.load(Ordering::Acquire) {
                ReviewSummary::aborted_with("aborted by parent cascade")
            } else {
                ok_review()
            }
        }
    }
    let saw = std::sync::Arc::new(AtomicBool::new(false));
    rt.attach_reviewer(std::sync::Arc::new(AbortPeekMock {
        saw_signal: std::sync::Arc::clone(&saw),
    }));
    rt.enter_planning().unwrap();
    let out = create_plan::execute_with_reviewer(&rt, good_args_with_todo(), true)
        .await
        .unwrap();
    assert!(saw.load(Ordering::Acquire), "dispatcher 未收到 abort signal");
    assert_eq!(out["review"]["aborted"], serde_json::Value::Bool(false));
    cleanup_home(&home);
}

#[tokio::test]
async fn reviewer_round_count_warns_after_threshold() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    rt.attach_reviewer(std::sync::Arc::new(MockReviewerDispatcher::new(vec![
        ok_review(),
        ok_review(),
    ])));
    rt.enter_planning().unwrap();

    let out1 = create_plan::execute_with_reviewer(&rt, good_args_with_todo(), false)
        .await
        .unwrap();
    let plan_id_1 = out1["plan_id"].as_str().unwrap().to_string();
    assert!(!out1["review"]["summary"].as_str().unwrap().starts_with("[round"));
    assert_eq!(rt.reviewer_rounds(&plan_id_1), 1);

    let summary = rt.dispatch_reviewer(&plan_id_1, false).await;
    assert!(summary.summary.starts_with("[round 2]"), "{summary:?}");
    assert_eq!(rt.reviewer_rounds(&plan_id_1), 2);
    cleanup_home(&home);
}
