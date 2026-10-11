use super::*;

#[test]
fn unattended_mode_allows_ten_attempts() {
    assert_eq!(
        retry_attempt_budget(true, DEFAULT_AGENT_MAX_ATTEMPTS),
        UNATTENDED_TRANSIENT_MAX_ATTEMPTS
    );
    assert_eq!(
        retry_attempt_budget(false, DEFAULT_AGENT_MAX_ATTEMPTS),
        DEFAULT_AGENT_MAX_ATTEMPTS
    );
    assert_eq!(
        compute_retry_delay_with_cap_ms(500, 20, 40, UNATTENDED_RETRY_DELAY_CAP_MS),
        UNATTENDED_RETRY_DELAY_CAP_MS
    );
}

use super::hand_back_unfinished_plan_if_needed;
use crate::core::plan_runtime::file_store::{
    read_plan, write_plan, PlanFile, PlanFileFrontmatter, PlanFileState, TodoItem, TodoKind,
    TodoStatus,
};
use crate::core::plan_runtime::PlanRuntime;

#[tokio::test]
async fn run_ended_while_executing_hands_back_visibly() {
    let directory = tempfile::tempdir().expect("plan directory");
    let path = directory.path().join("plan.plan.md");
    let plan = PlanFile {
        frontmatter: PlanFileFrontmatter {
            plan_id: "run-exit".into(),
            goal: "finish remaining work".into(),
            state: PlanFileState::Executing,
            session_key: Some("session".into()),
            session_id: None,
            created_at: "2026-09-18T00:00:00Z".into(),
            schema_version: 1,
            todos: vec![TodoItem {
                id: "work".into(),
                content: "finish implementation".into(),
                status: TodoStatus::Pending,
                evidence: Vec::new(),
                kind: TodoKind::Work,
            }],
            code_review_pass: false,
            code_review_rounds: 0,
            code_review_open_findings: Vec::new(),
            code_review_disputed_findings: Vec::new(),
            code_review_handoff: false,
            code_review_handoff_acknowledged: false,
            code_review_residual_findings: Vec::new(),
            unknown: Default::default(),
        },
        body: String::new(),
    };
    write_plan(&path, &plan, 1_000).expect("write plan");

    let runtime = PlanRuntime::new("session");
    runtime.bind_plan_file_for_test(path.clone());
    let events = std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
    {
        let events = events.clone();
        runtime.attach_transcript_appender(std::sync::Arc::new(move |event| {
            events.lock().push(event);
            Ok(())
        }));
    }

    hand_back_unfinished_plan_if_needed(true, Some(&runtime), "run_ended_while_executing").await;

    assert_eq!(
        read_plan(&path).expect("read plan").frontmatter.state,
        PlanFileState::Pending
    );
    assert!(events.lock().iter().any(|event| {
        event["event"] == crate::infra::wire::WIRE_PLAN_STALLED
            && event["reason"] == "run_ended_while_executing"
    }));
}
