use super::super::file_store::{PlanFileState, TodoItem, TodoKind, TodoStatus};
use super::super::{NextAction, PlanRuntime, PlanRuntimeError};
use crate::core::session::AgentMode;

#[test]
fn enter_plan_changes_only_the_session_mode() {
    let runtime = PlanRuntime::new("session");

    runtime.enter_plan().unwrap();

    assert_eq!(runtime.mode(), AgentMode::Plan);
    assert!(runtime.active_plan().is_none());
}

#[test]
fn entering_plan_twice_is_rejected() {
    let runtime = PlanRuntime::new("session");
    runtime.enter_plan().unwrap();

    assert!(matches!(
        runtime.enter_plan().unwrap_err(),
        PlanRuntimeError::AlreadyInMode(_)
    ));
    assert_eq!(runtime.mode(), AgentMode::Plan);
}

#[test]
fn exit_plan_changes_only_the_session_mode() {
    let runtime = PlanRuntime::new("session");
    runtime.enter_plan().unwrap();

    runtime.exit_plan().unwrap();

    assert_eq!(runtime.mode(), AgentMode::Chat);
    assert!(runtime.active_plan().is_none());
}

#[test]
fn mode_transitions_are_durable_before_notifier_observes_them() {
    let runtime = PlanRuntime::new("session");
    let transcript = std::sync::Arc::new(parking_lot::Mutex::new(Vec::<serde_json::Value>::new()));
    let observed_modes = std::sync::Arc::new(parking_lot::Mutex::new(Vec::<AgentMode>::new()));
    {
        let transcript = transcript.clone();
        runtime.attach_transcript_appender(std::sync::Arc::new(move |event| {
            transcript.lock().push(event);
            Ok(())
        }));
    }
    {
        let observed_modes = observed_modes.clone();
        let runtime_for_notifier = runtime.clone();
        runtime.attach_transcript_event_notifier(std::sync::Arc::new(move |_| {
            observed_modes.lock().push(runtime_for_notifier.mode());
        }));
    }

    runtime.enter_plan().unwrap();
    runtime.exit_plan().unwrap();

    assert_eq!(
        *transcript.lock(),
        vec![
            serde_json::json!({
                "event": crate::infra::wire::WIRE_SESSION_AGENT_MODE_CHANGED,
                "agentMode": "plan",
            }),
            serde_json::json!({
                "event": crate::infra::wire::WIRE_SESSION_AGENT_MODE_CHANGED,
                "agentMode": "chat",
            }),
        ]
    );
    assert_eq!(
        *observed_modes.lock(),
        vec![AgentMode::Plan, AgentMode::Chat]
    );
}

#[test]
fn transcript_append_failure_does_not_change_in_memory_mode_or_notify() {
    let runtime = PlanRuntime::new("session");
    let notifications = std::sync::Arc::new(parking_lot::Mutex::new(0usize));
    runtime.attach_transcript_appender(std::sync::Arc::new(|_| {
        Err(crate::AppError::Config("simulated append failure".into()))
    }));
    {
        let notifications = notifications.clone();
        runtime.attach_transcript_event_notifier(std::sync::Arc::new(move |_| {
            *notifications.lock() += 1;
        }));
    }

    assert!(matches!(
        runtime.enter_plan().unwrap_err(),
        PlanRuntimeError::Io(message) if message.contains("simulated append failure")
    ));
    assert_eq!(runtime.mode(), AgentMode::Chat);
    assert_eq!(*notifications.lock(), 0);
}

#[test]
fn concurrent_enter_plan_commits_a_single_mode_event() {
    let runtime = PlanRuntime::new("session");
    let events = std::sync::Arc::new(parking_lot::Mutex::new(Vec::<serde_json::Value>::new()));
    {
        let events = events.clone();
        runtime.attach_transcript_appender(std::sync::Arc::new(move |event| {
            events.lock().push(event);
            Ok(())
        }));
    }

    let first = {
        let runtime = runtime.clone();
        std::thread::spawn(move || runtime.enter_plan())
    };
    let second = {
        let runtime = runtime.clone();
        std::thread::spawn(move || runtime.enter_plan())
    };

    assert!(first.join().unwrap().is_ok() ^ second.join().unwrap().is_ok());
    assert_eq!(runtime.mode(), AgentMode::Plan);
    assert_eq!(events.lock().len(), 1);
}

#[test]
fn executing_plan_id_is_derived_from_the_active_plan_cache() {
    let runtime = PlanRuntime::new("session");
    assert!(runtime.executing_plan_id().is_none());

    runtime.seed_active_plan_for_test(
        "plan_a".into(),
        crate::core::plan_runtime::file_store::PlanFileState::Executing,
    );

    assert_eq!(runtime.mode(), AgentMode::Chat);
    assert_eq!(runtime.executing_plan_id().as_deref(), Some("plan_a"));
}

#[test]
fn recovering_without_sidecar_state_defaults_to_chat() {
    let runtime = PlanRuntime::new("session");
    runtime.recover().unwrap();

    assert_eq!(runtime.mode(), AgentMode::Chat);
    assert!(runtime.active_plan().is_none());
}

#[tokio::test]
async fn next_action_is_the_single_close_out_decision_source() {
    let runtime = PlanRuntime::new("session");
    let mut frontmatter = super::sample_frontmatter();
    frontmatter.todos = vec![
        TodoItem {
            id: "work".into(),
            content: "implement".into(),
            status: TodoStatus::Completed,
            evidence: Vec::new(),
            kind: TodoKind::Work,
        },
        TodoItem {
            id: "gate-review".into(),
            content: "[gate] review".into(),
            status: TodoStatus::Pending,
            evidence: Vec::new(),
            kind: TodoKind::GateCodeReview,
        },
        TodoItem {
            id: "gate-acceptance".into(),
            content: "[gate] Acceptance".into(),
            status: TodoStatus::Pending,
            evidence: Vec::new(),
            kind: TodoKind::GateAcceptance,
        },
    ];
    assert_eq!(
        runtime.next_action(&frontmatter).await,
        NextAction::StartReview
    );

    frontmatter.todos[0].status = TodoStatus::Pending;
    assert_eq!(
        runtime.next_action(&frontmatter).await,
        NextAction::ContinueWork {
            remaining_work: vec!["- work (pending)".into()]
        }
    );

    frontmatter.todos[0].status = TodoStatus::Completed;
    frontmatter.code_review_pass = true;
    assert!(matches!(
        runtime.next_action(&frontmatter).await,
        NextAction::RunAcceptance { .. }
    ));

    frontmatter.code_review_pass = false;
    frontmatter.code_review_open_findings = vec![super::super::review::Finding::new(
        "P1".into(),
        "logic".into(),
        "fix".into(),
    )];
    assert!(matches!(
        runtime.next_action(&frontmatter).await,
        NextAction::FixFindings { .. }
    ));

    frontmatter.code_review_handoff = true;
    assert!(matches!(
        runtime.next_action(&frontmatter).await,
        NextAction::HandOff {
            reason: "p0_residual",
            ..
        }
    ));

    frontmatter.state = PlanFileState::Completed;
    assert_eq!(runtime.next_action(&frontmatter).await, NextAction::Done);
}
