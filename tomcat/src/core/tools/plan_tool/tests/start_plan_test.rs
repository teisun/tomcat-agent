use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use super::common::*;
use crate::core::plan_runtime::panels::{
    Answer, AskQuestionIdentity, AskQuestionOutcome, AskQuestionPanel, AskQuestionResult,
    AskQuestionTermination, MockAskQuestionPanel, Question,
};
use crate::core::tools::plan_tool::start_plan::{self, StartPlanArgs};

fn choice(ids: &[&str], custom: Option<&str>, skipped: bool) -> AskQuestionResult {
    AskQuestionResult::answered(vec![Answer {
        question_id: "plan-start".into(),
        option_ids: ids.iter().map(|s| s.to_string()).collect(),
        custom_text: custom.map(str::to_owned),
        skipped,
        picked_recommended: false,
    }])
}

#[tokio::test]
async fn start_plan_requires_explicit_consent_and_commits_only_once() {
    let _guard = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let cases = [
        (choice(&["start"], None, false), true, ""),
        (choice(&["stay"], None, false), false, "user_declined"),
        (choice(&[], None, true), false, "user_declined"),
        (
            choice(&["start", "stay"], None, false),
            false,
            "user_declined",
        ),
        (choice(&["unknown"], None, false), false, "user_declined"),
        (
            choice(&["start"], Some("yes"), false),
            false,
            "user_declined",
        ),
        (choice(&["start"], None, true), false, "user_declined"),
        (
            AskQuestionResult::terminal(AskQuestionOutcome::Skipped),
            false,
            "user_declined",
        ),
        (
            AskQuestionResult::terminal(AskQuestionOutcome::Interrupted),
            false,
            "interrupted",
        ),
        (
            AskQuestionResult::terminal(AskQuestionOutcome::HostDisconnected),
            false,
            "interrupted",
        ),
        (
            AskQuestionResult::terminal(AskQuestionOutcome::CancelledUnknown),
            false,
            "interrupted",
        ),
        (AskQuestionResult::answered(vec![]), false, "user_declined"),
        (
            AskQuestionResult::answered(vec![choice(&["start"], None, false).answers.remove(0); 2]),
            false,
            "user_declined",
        ),
    ];
    for (index, (answer, started, reason)) in cases.into_iter().enumerate() {
        let rt = PlanRuntime::new("start-session");
        let id = format!("start_{index}");
        let path = write_disk_plan(&id, PlanFileState::Planning);
        let mut plan = read_plan(&path).unwrap();
        plan.frontmatter.code_review_rounds = 2;
        write_plan(&path, &plan, 1000).unwrap();
        rt.bind_plan_file_for_test(path.clone());
        rt.enter_plan().unwrap();
        rt.attach_ask_question_panel(Arc::new(MockAskQuestionPanel::new(vec![answer])));
        let events = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let sink = events.clone();
        rt.attach_transcript_appender(Arc::new(move |value| {
            sink.lock().push(value);
            Ok(())
        }));
        let before = std::fs::read(&path).unwrap();
        let result =
            start_plan::execute_at_path(&rt, path.clone(), Default::default(), Some("start-call"))
                .await
                .unwrap();
        assert_eq!(result["started"], started, "case {index}");
        let builds = events
            .lock()
            .iter()
            .filter(|v| v["event"] == "plan.build")
            .count();
        assert_eq!(builds, usize::from(started));
        if started {
            assert_eq!(rt.mode(), AgentMode::Chat);
            assert_eq!(rt.executing_plan_id().as_deref(), Some(id.as_str()));
            let saved = read_plan(&path).unwrap();
            assert_eq!(saved.frontmatter.state, PlanFileState::Executing);
            assert_eq!(
                saved.frontmatter.session_key.as_deref(),
                Some("start-session")
            );
            assert_eq!(saved.frontmatter.code_review_rounds, 0);
            let committed = std::fs::read(&path).unwrap();
            assert!(
                start_plan::execute_at_path(&rt, path.clone(), Default::default(), None)
                    .await
                    .is_err()
            );
            assert_eq!(std::fs::read(&path).unwrap(), committed);
            assert_eq!(
                events
                    .lock()
                    .iter()
                    .filter(|v| v["event"] == "plan.build")
                    .count(),
                1
            );
        } else {
            assert_eq!(result["reason"], reason);
            assert_eq!(rt.mode(), AgentMode::Plan);
            assert!(rt.executing_plan_id().is_none());
            assert_eq!(std::fs::read(&path).unwrap(), before);
            assert!(events.lock().is_empty());
        }
    }
    cleanup_home(&home);
}

#[tokio::test]
async fn start_plan_confirmation_carries_identity_and_holds_no_plan_lock() {
    struct InspectPanel {
        path: std::path::PathBuf,
    }
    #[async_trait]
    impl AskQuestionPanel for InspectPanel {
        async fn ask(&self, _: Vec<Question>, _: AskQuestionTermination) -> AskQuestionResult {
            panic!("must associate the confirmation with the tool call");
        }
        async fn ask_with_identity(
            &self,
            identity: AskQuestionIdentity,
            questions: Vec<Question>,
            _: AskQuestionTermination,
        ) -> AskQuestionResult {
            assert_eq!(identity.tool_call_id.as_deref(), Some("start-call"));
            assert_eq!(identity.session_id.as_deref(), Some("session-id"));
            assert_eq!(questions.len(), 1);
            let question = &questions[0];
            assert_eq!(question.id, "plan-start");
            assert!(!question.allow_custom);
            assert_eq!(question.options.len(), 2);
            assert_eq!(question.options[0].id, "start");
            assert!(question.options[0].recommended);
            assert_eq!(question.options[1].id, "stay");
            assert!(!question.options[1].recommended);
            assert_eq!(
                question.prompt,
                crate::infra::i18n::tr_in(
                    crate::infra::i18n::Locale::En,
                    "plan.start.prompt",
                    &[("id", "identity")]
                )
            );
            let plan = read_plan(&self.path).unwrap();
            write_plan(&self.path, &plan, 100).expect("confirmation cannot hold the plan lock");
            choice(&["start"], None, false)
        }
    }
    let _guard = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new_with_session_id("start-session", "session-id");
    rt.enter_plan().unwrap();
    let path = write_disk_plan("identity", PlanFileState::Planning);
    rt.attach_ask_question_panel(Arc::new(InspectPanel { path: path.clone() }));
    start_plan::execute_at_path(&rt, path.clone(), Default::default(), Some("start-call"))
        .await
        .unwrap();
    assert_eq!(
        read_plan(&path).unwrap().frontmatter.session_id.as_deref(),
        Some("session-id")
    );
    cleanup_home(&home);
}

#[tokio::test]
async fn start_plan_chat_skips_confirmation_and_plan_without_panel_fails_closed() {
    let _guard = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("start-session");
    let path = write_disk_plan("resume", PlanFileState::Pending);
    rt.bind_plan_file_for_test(path.clone());
    let args = StartPlanArgs { plan_id: None };
    assert_eq!(
        start_plan::resolve_target_plan_path(&rt, &args).unwrap(),
        path
    );
    let panel = Arc::new(MockAskQuestionPanel::new(vec![choice(
        &["stay"],
        None,
        false,
    )]));
    rt.attach_ask_question_panel(panel.clone());
    let result = start_plan::execute_at_path(&rt, path.clone(), Default::default(), None)
        .await
        .unwrap();
    assert_eq!(result["started"], true);
    assert_eq!(panel.remaining(), 1);
    assert_eq!(
        read_plan(&path).unwrap().frontmatter.todos[0].status,
        TodoStatus::Pending
    );
    assert!(start_plan::resolve_target_plan_path(&rt, &args).is_err());

    let rt = PlanRuntime::new("without-panel");
    rt.enter_plan().unwrap();
    let path = write_disk_plan("no_panel", PlanFileState::Planning);
    let before = std::fs::read(&path).unwrap();
    let error = start_plan::execute_at_path(&rt, path.clone(), Default::default(), None)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("/plan build"));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(rt.mode(), AgentMode::Plan);
    cleanup_home(&home);
}

#[tokio::test]
async fn start_plan_preserves_existing_state_gates_and_interruption() {
    let _guard = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    for state in [PlanFileState::Executing, PlanFileState::Completed] {
        let rt = PlanRuntime::new("another-session");
        let path = write_disk_plan("blocked", state);
        let before = std::fs::read(&path).unwrap();
        assert!(
            start_plan::execute_at_path(&rt, path.clone(), Default::default(), None)
                .await
                .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(rt.active_plan().is_none());
    }
    let rt = PlanRuntime::new("interrupted");
    rt.enter_plan().unwrap();
    let path = write_disk_plan("interrupted", PlanFileState::Planning);
    rt.attach_ask_question_panel(Arc::new(
        MockAskQuestionPanel::new(vec![choice(&["start"], None, false)]).ignore_cancel(),
    ));
    let before = std::fs::read(&path).unwrap();
    let termination = AskQuestionTermination::default();
    termination.interrupt();
    let result = start_plan::execute_at_path(&rt, path.clone(), termination, None)
        .await
        .unwrap();
    assert_eq!(result, json!({"started": false, "reason": "interrupted"}));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(rt.mode(), AgentMode::Plan);
    assert!(start_plan::resolve_target_plan_path(
        &rt,
        &StartPlanArgs {
            plan_id: Some("../outside".into())
        }
    )
    .is_err());
    assert!(serde_json::from_value::<StartPlanArgs>(json!({"path": "/tmp/a.plan.md"})).is_err());
    cleanup_home(&home);
}

#[tokio::test]
async fn editing_or_creating_other_plans_cannot_replace_executing_binding() {
    let _guard = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("active-session");
    let a = write_disk_plan("active_a", PlanFileState::Planning);
    let b = write_disk_plan("other_b", PlanFileState::Planning);
    rt.build_plan("active_a", None).unwrap();
    update_plan::execute(
        &rt,
        serde_json::from_value(json!({
            "plan_id": "other_b", "ops": [{"kind":"upsert", "id":"step1", "content":"edited B"}]
        }))
        .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(rt.executing_plan_id().as_deref(), Some("active_a"));
    assert_eq!(
        read_plan(&b).unwrap().frontmatter.todos[0].content,
        "edited B"
    );
    assert!(rt.build_plan("other_b", None).is_err());
    rt.enter_plan().unwrap();
    let created = create_plan::execute(&rt, good_args_with_todo()).unwrap();
    assert_eq!(rt.executing_plan_id().as_deref(), Some("active_a"));
    assert!(rt
        .build_plan(created["plan_id"].as_str().unwrap(), None)
        .is_err());
    assert_eq!(
        read_plan(&a).unwrap().frontmatter.state,
        PlanFileState::Executing
    );
    cleanup_home(&home);
}
