use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;

use super::common::*;

fn consent_to_review(rt: &PlanRuntime) {
    use crate::core::plan_runtime::panels::{Answer, AskQuestionResult, MockAskQuestionPanel};
    rt.attach_ask_question_panel(std::sync::Arc::new(MockAskQuestionPanel::new(vec![
        AskQuestionResult::answered(vec![Answer {
            question_id: "plan-review".into(),
            option_ids: vec!["review".into()],
            custom_text: None,
            skipped: false,
            picked_recommended: true,
        }]),
    ])));
}

#[tokio::test]
async fn plan_review_decisions_require_explicit_consent_and_emit_exactly_one_result() {
    use crate::core::plan_runtime::panels::{
        Answer, AskQuestionOutcome, AskQuestionResult, MockAskQuestionPanel,
    };
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let choice = |ids: &[&str], custom: Option<&str>, skipped: bool, recommended: bool| {
        AskQuestionResult::answered(vec![Answer {
            question_id: "plan-review".into(),
            option_ids: ids.iter().map(|id| id.to_string()).collect(),
            custom_text: custom.map(str::to_owned),
            skipped,
            picked_recommended: recommended,
        }])
    };
    let cases = [
        (choice(&["review"], None, false, false), true, ""),
        (choice(&["skip"], None, false, true), false, "user_skipped"),
        (choice(&[], None, true, false), false, "user_skipped"),
        (
            AskQuestionResult::terminal(AskQuestionOutcome::Skipped),
            false,
            "user_skipped",
        ),
        (
            choice(&["review", "skip"], None, false, true),
            false,
            "user_skipped",
        ),
        (
            choice(&["unknown"], None, false, true),
            false,
            "user_skipped",
        ),
        (
            choice(&["review"], Some("yes"), false, true),
            false,
            "user_skipped",
        ),
        (choice(&["review"], None, true, true), false, "user_skipped"),
        (
            AskQuestionResult::terminal(AskQuestionOutcome::Interrupted),
            false,
            "parent_abort",
        ),
        (
            AskQuestionResult::terminal(AskQuestionOutcome::HostDisconnected),
            false,
            "parent_abort",
        ),
    ];
    for (index, (answer, review, stop)) in cases.into_iter().enumerate() {
        let rt = PlanRuntime::new("decision");
        let dispatcher = std::sync::Arc::new(MockPlanReviewerDispatcher::new(vec![ok_review()]));
        rt.attach_plan_reviewer(dispatcher.clone());
        rt.attach_ask_question_panel(std::sync::Arc::new(MockAskQuestionPanel::new(vec![answer])));
        let captured = std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
        let sink = captured.clone();
        rt.attach_transcript_appender(std::sync::Arc::new(move |extra| {
            sink.lock().push(extra);
            Ok(())
        }));
        rt.enter_plan().unwrap();
        let mut args = good_args_with_todo();
        args.goal = format!("decision case {index}");
        let out = create_plan::execute_with_reviewer(&rt, args, false)
            .await
            .unwrap();
        let id = out["plan_id"].as_str().unwrap();
        assert!(plan_path_for_id(id).unwrap().exists());
        assert_eq!(rt.reviewer_rounds(id), u32::from(review));
        assert_eq!(
            dispatcher.call_count.load(Ordering::Relaxed),
            usize::from(review)
        );
        let events = captured.lock();
        let results = events
            .iter()
            .filter(|event| event["event"] == "plan.review")
            .collect::<Vec<_>>();
        assert_eq!(results.len(), 1, "case {index}");
        assert_eq!(results[0]["reviewer_stop_reason"], stop, "case {index}");
        if !review {
            assert_eq!(out["review"]["aborted"], true);
            assert_eq!(out["review"]["applied_changes"], false);
            assert_eq!(out["review"]["reviewer_turns_used"], 0);
        }
    }
    cleanup_home(&home);
}

#[tokio::test]
async fn plan_review_wait_holds_no_file_lock_and_parent_stop_keeps_saved_plan() {
    use crate::core::plan_runtime::panels::{
        AskQuestionPanel, AskQuestionResult, AskQuestionTermination, Question,
    };
    struct WaitingPanel {
        runtime: std::sync::Arc<PlanRuntime>,
        ready: std::sync::Arc<tokio::sync::Notify>,
    }
    #[async_trait]
    impl AskQuestionPanel for WaitingPanel {
        async fn ask(
            &self,
            questions: Vec<Question>,
            termination: AskQuestionTermination,
        ) -> AskQuestionResult {
            assert_eq!(questions.len(), 1);
            assert!(!questions[0].allow_custom);
            assert_eq!(
                questions[0].prompt,
                crate::infra::i18n::tr_in(
                    crate::infra::i18n::Locale::En,
                    "plan.review.prompt",
                    &[],
                )
            );
            let active = self.runtime.active_plan().unwrap();
            // An actual read/write under the same advisory lock succeeds while the user decides.
            let plan = read_plan(&active.path).unwrap();
            write_plan(&active.path, &plan, 150).unwrap();
            self.ready.notify_one();
            loop {
                if let Some(result) = termination.result() {
                    return result;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        }
    }
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("waiting");
    let ready = std::sync::Arc::new(tokio::sync::Notify::new());
    rt.attach_ask_question_panel(std::sync::Arc::new(WaitingPanel {
        runtime: rt.clone(),
        ready: ready.clone(),
    }));
    let dispatcher = std::sync::Arc::new(MockPlanReviewerDispatcher::new(vec![ok_review()]));
    rt.attach_plan_reviewer(dispatcher.clone());
    rt.enter_plan().unwrap();
    let termination = AskQuestionTermination::default();
    let stop = termination.clone();
    let cancel = async {
        ready.notified().await;
        assert_eq!(dispatcher.call_count.load(Ordering::Relaxed), 0);
        stop.interrupt();
    };
    let (out, ()) = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        tokio::join!(
            create_plan::execute_for_tool(
                &rt,
                good_args_with_todo(),
                true,
                termination,
                Some("create-call")
            ),
            cancel
        )
    })
    .await
    .expect("parent stop must end the wait");
    let out = out.unwrap();
    assert_eq!(out["review"]["reviewer_stop_reason"], "parent_abort");
    assert!(plan_path_for_id(out["plan_id"].as_str().unwrap())
        .unwrap()
        .exists());
    assert_eq!(dispatcher.call_count.load(Ordering::Relaxed), 0);
    cleanup_home(&home);
}

#[tokio::test]
async fn create_plan_internally_dispatches_reviewer_with_real_summary() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    rt.attach_plan_reviewer(std::sync::Arc::new(MockPlanReviewerDispatcher::new(vec![
        ok_review(),
    ])));
    consent_to_review(&rt);
    rt.enter_plan().unwrap();
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
    rt.attach_plan_reviewer(std::sync::Arc::new(MockPlanReviewerDispatcher::new(vec![
        PlanReviewSummary::aborted_with("simulated parse error"),
    ])));
    consent_to_review(&rt);
    rt.enter_plan().unwrap();
    let out = create_plan::execute_with_reviewer(&rt, good_args_with_todo(), false)
        .await
        .unwrap();
    let plan_id = out["plan_id"].as_str().unwrap().to_string();
    assert_eq!(out["review"]["aborted"], serde_json::Value::Bool(true));
    assert!(out["review"]["summary"]
        .as_str()
        .unwrap()
        .contains("parse error"));
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
    consent_to_review(&rt);
    rt.enter_plan().unwrap();
    let out = create_plan::execute_with_reviewer(&rt, good_args_with_todo(), false)
        .await
        .unwrap();
    assert_eq!(out["review"]["aborted"], serde_json::Value::Bool(true));
    assert!(out["review"]["summary"]
        .as_str()
        .unwrap()
        .contains(&crate::infra::i18n::tr_in(
            crate::infra::i18n::Locale::En,
            "plan.review.unavailable",
            &[]
        )));
    cleanup_home(&home);
}

#[tokio::test]
async fn dispatch_reviewer_releases_plan_lock_before_spawn() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");

    struct LockAcquiringMock;
    #[async_trait]
    impl PlanReviewerDispatcher for LockAcquiringMock {
        async fn dispatch(
            &self,
            plan_id: &str,
            _plan_text: &str,
            _allow_review_edit: bool,
        ) -> PlanReviewSummary {
            use crate::core::plan_runtime::file_store::{plan_path_for_id, with_advisory_lock};
            let path = plan_path_for_id(plan_id).unwrap();
            let lock_path = path.with_file_name(format!(
                "{}.lock",
                path.file_name().unwrap().to_string_lossy()
            ));
            let r = with_advisory_lock(&lock_path, 150, || {
                Ok::<_, crate::core::plan_runtime::file_store::PlanError>(())
            });
            match r {
                Ok(()) => PlanReviewSummary {
                    aborted: false,
                    summary: "lock acquired by reviewer (write_plan 已释放)".into(),
                    changes_summary: "none".into(),
                    applied_changes: false,
                    ..Default::default()
                },
                Err(e) => PlanReviewSummary::aborted_with(format!("LockBusy: {e}")),
            }
        }
    }

    rt.attach_plan_reviewer(std::sync::Arc::new(LockAcquiringMock));
    consent_to_review(&rt);
    rt.enter_plan().unwrap();
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

    consent_to_review(&rt);
    rt.enter_plan().unwrap();
    let out = create_plan::execute(&rt, good_args_with_todo()).expect("create_plan OK");
    let plan_id = out["plan_id"].as_str().unwrap().to_string();

    let events = captured.lock();
    let plan_create = events
        .iter()
        .find(|v| v["event"] == "plan.create")
        .expect("缺少 plan.create 事件");
    assert_eq!(plan_create["plan_id"], plan_id);
    assert_eq!(plan_create["state"], "planning");
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

    let summary = PlanReviewSummary {
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
    rt.attach_plan_reviewer(std::sync::Arc::new(MockPlanReviewerDispatcher::new(vec![
        summary,
    ])));
    consent_to_review(&rt);
    rt.enter_plan().unwrap();
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
    assert!(plan_review["plan_id"]
        .as_str()
        .unwrap()
        .starts_with("plan_"));
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
    rt.attach_plan_reviewer(std::sync::Arc::new(MockPlanReviewerDispatcher::new(vec![
        ok_review(),
        ok_review(),
    ])));
    consent_to_review(&rt);
    rt.enter_plan().unwrap();
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
async fn reviewer_dispatch_invokes_mock_without_abort_param() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");

    struct CallTrackingMock {
        called: std::sync::Arc<AtomicBool>,
    }
    #[async_trait]
    impl PlanReviewerDispatcher for CallTrackingMock {
        async fn dispatch(
            &self,
            _plan_id: &str,
            _plan_text: &str,
            _allow_review_edit: bool,
        ) -> PlanReviewSummary {
            self.called.store(true, Ordering::Release);
            ok_review()
        }
    }
    let called = std::sync::Arc::new(AtomicBool::new(false));
    rt.attach_plan_reviewer(std::sync::Arc::new(CallTrackingMock {
        called: std::sync::Arc::clone(&called),
    }));
    consent_to_review(&rt);
    rt.enter_plan().unwrap();
    let out = create_plan::execute_with_reviewer(&rt, good_args_with_todo(), true)
        .await
        .unwrap();
    assert!(called.load(Ordering::Acquire), "dispatcher 未被调用");
    assert_eq!(out["review"]["aborted"], serde_json::Value::Bool(false));
    cleanup_home(&home);
}

#[tokio::test]
async fn reviewer_round_count_warns_after_threshold() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    rt.attach_plan_reviewer(std::sync::Arc::new(MockPlanReviewerDispatcher::new(vec![
        ok_review(),
        ok_review(),
    ])));
    consent_to_review(&rt);
    rt.enter_plan().unwrap();

    let out1 = create_plan::execute_with_reviewer(&rt, good_args_with_todo(), false)
        .await
        .unwrap();
    let plan_id_1 = out1["plan_id"].as_str().unwrap().to_string();
    assert!(!out1["review"]["summary"]
        .as_str()
        .unwrap()
        .starts_with("[round"));
    assert_eq!(rt.reviewer_rounds(&plan_id_1), 1);

    let summary = rt.dispatch_reviewer(&plan_id_1, false).await;
    assert!(summary.summary.starts_with("[round 2]"), "{summary:?}");
    assert_eq!(rt.reviewer_rounds(&plan_id_1), 2);
    cleanup_home(&home);
}
