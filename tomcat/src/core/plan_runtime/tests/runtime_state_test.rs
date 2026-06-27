use super::super::{PlanRuntime, PlanRuntimeError, PlanState};

#[test]
fn enter_planning_from_chat_transitions_to_planning() {
    let rt = PlanRuntime::new("sess-1");
    assert!(matches!(rt.mode(), PlanState::Chat));
    rt.enter_planning().unwrap();
    assert!(matches!(rt.mode(), PlanState::Planning));
}

#[test]
fn enter_planning_from_completed_transitions_to_planning() {
    let rt = PlanRuntime::new("sess-1");
    rt.set_mode_completed("done-1".into());
    rt.enter_planning().unwrap();
    assert!(matches!(rt.mode(), PlanState::Planning));
}

#[test]
fn enter_planning_twice_is_rejected() {
    let rt = PlanRuntime::new("sess-1");
    rt.enter_planning().unwrap();
    let err = rt.enter_planning().unwrap_err();
    assert!(matches!(err, PlanRuntimeError::AlreadyInMode(_)));
    assert!(matches!(rt.mode(), PlanState::Planning));
}

#[test]
fn exit_to_chat_from_planning_resets() {
    let rt = PlanRuntime::new("sess-1");
    rt.enter_planning().unwrap();
    rt.exit_to_chat().unwrap();
    assert!(matches!(rt.mode(), PlanState::Chat));
}

#[test]
fn exit_to_chat_when_already_chat_errors() {
    let rt = PlanRuntime::new("sess-1");
    let err = rt.exit_to_chat().unwrap_err();
    assert!(matches!(err, PlanRuntimeError::AlreadyInMode(_)));
}

#[test]
fn enter_and_exit_write_transition_events() {
    let rt = PlanRuntime::new("sess-1");
    let events = std::sync::Arc::new(parking_lot::Mutex::new(Vec::<serde_json::Value>::new()));
    {
        let events = events.clone();
        rt.attach_transcript_appender(std::sync::Arc::new(move |extra| {
            events.lock().push(extra);
            Ok(())
        }));
    }

    rt.enter_planning().unwrap();
    rt.exit_to_chat().unwrap();

    let events = events.lock();
    assert_eq!(events[0]["event"], crate::infra::wire::WIRE_PLAN_ENTER);
    assert_eq!(events[0]["state"], "planning");
    assert_eq!(events[1]["event"], crate::infra::wire::WIRE_PLAN_EXIT);
    assert_eq!(events[1]["state"], "chat");
}

#[test]
fn completed_and_pending_modes_emit_transition_events() {
    let rt = PlanRuntime::new("sess-1");
    let events = std::sync::Arc::new(parking_lot::Mutex::new(Vec::<serde_json::Value>::new()));
    {
        let events = events.clone();
        rt.attach_transcript_appender(std::sync::Arc::new(move |extra| {
            events.lock().push(extra);
            Ok(())
        }));
    }

    rt.set_mode_completed("done-1".into());
    rt.set_mode_pending("done-1".into());

    let events = events.lock();
    assert_eq!(events[0]["event"], crate::infra::wire::WIRE_PLAN_COMPLETE);
    assert_eq!(events[0]["plan_id"], "done-1");
    assert_eq!(events[0]["state"], "completed");
    assert_eq!(events[1]["event"], crate::infra::wire::WIRE_PLAN_PENDING);
    assert_eq!(events[1]["plan_id"], "done-1");
    assert_eq!(events[1]["state"], "pending");
}

#[test]
fn finalize_completed_to_chat_does_not_emit_event() {
    let rt = PlanRuntime::new("sess-1");
    let events = std::sync::Arc::new(parking_lot::Mutex::new(Vec::<serde_json::Value>::new()));
    {
        let events = events.clone();
        rt.attach_transcript_appender(std::sync::Arc::new(move |extra| {
            events.lock().push(extra);
            Ok(())
        }));
    }

    rt.set_mode_completed("done-1".into());
    events.lock().clear();

    assert_eq!(rt.finalize_completed_to_chat().as_deref(), Some("done-1"));
    assert!(events.lock().is_empty());
}

#[test]
fn recover_in_initial_chat_is_noop() {
    let rt = PlanRuntime::new("sess-1");
    rt.recover().unwrap();
    assert!(matches!(rt.mode(), PlanState::Chat));
}

#[test]
fn mode_active_plan_id_returns_some_only_for_attached_states() {
    assert_eq!(PlanState::Chat.active_plan_id(), None);
    assert_eq!(PlanState::Planning.active_plan_id(), None);
    assert_eq!(
        PlanState::Executing {
            plan_id: "x".into()
        }
        .active_plan_id(),
        Some("x")
    );
    assert_eq!(
        PlanState::Pending {
            plan_id: "x".into()
        }
        .active_plan_id(),
        Some("x")
    );
    assert_eq!(
        PlanState::Completed {
            plan_id: "x".into()
        }
        .active_plan_id(),
        Some("x")
    );
}
