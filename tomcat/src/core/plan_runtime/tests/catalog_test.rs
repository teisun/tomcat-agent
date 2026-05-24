use serde_json::Value;

use super::super::catalog::visible_tools_for_mode;
use super::super::mode::PlanMode;

fn names(values: &[Value]) -> std::collections::BTreeSet<String> {
    values
        .iter()
        .map(|v| v["function"]["name"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn chat_mode_excludes_create_plan_only() {
    let tools = visible_tools_for_mode(&PlanMode::Chat);
    let n = names(&tools);
    assert!(
        !n.contains("create_plan"),
        "CHAT mode must hide create_plan"
    );
    for kept in ["update_plan", "todos", "ask_question", "write", "bash"] {
        assert!(n.contains(kept), "CHAT must expose {kept}, got: {n:?}");
    }
}

#[test]
fn planning_mode_exposes_full_set_including_writers_and_bash() {
    let tools = visible_tools_for_mode(&PlanMode::Planning);
    let n = names(&tools);
    for plan_tool in ["create_plan", "update_plan", "todos", "ask_question"] {
        assert!(
            n.contains(plan_tool),
            "PLANNING must expose {plan_tool}, got: {n:?}"
        );
    }
    for kept in ["write", "edit", "bash"] {
        assert!(
            n.contains(kept),
            "PLANNING must NOT hide writer/bash at catalog layer (path policy guards it): {kept}, got: {n:?}"
        );
    }
}

#[test]
fn executing_mode_excludes_create_plan_and_ask_question() {
    let tools = visible_tools_for_mode(&PlanMode::Executing {
        plan_id: "demo".into(),
    });
    let n = names(&tools);
    assert!(n.contains("update_plan"), "EXEC must keep update_plan");
    assert!(n.contains("todos"), "EXEC must keep todos");
    for hidden in ["create_plan", "ask_question"] {
        assert!(!n.contains(hidden), "EXEC must hide {hidden}, got: {n:?}");
    }
    assert!(n.contains("write"), "EXEC must keep write at catalog layer");
    assert!(n.contains("bash"), "EXEC must keep bash");
}

#[test]
fn pending_mode_view_equals_chat_view() {
    let pending = visible_tools_for_mode(&PlanMode::Pending {
        plan_id: "demo".into(),
    });
    let chat = visible_tools_for_mode(&PlanMode::Chat);
    assert_eq!(names(&pending), names(&chat));
}

#[test]
fn completed_mode_view_equals_chat_view() {
    let done = visible_tools_for_mode(&PlanMode::Completed {
        plan_id: "demo".into(),
    });
    let chat = visible_tools_for_mode(&PlanMode::Chat);
    assert_eq!(names(&done), names(&chat));
}
