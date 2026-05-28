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
