use super::*;
use crate::core::plan_runtime::{
    file_store::{
        parse_plan_file, read_plan, write_plan, PlanFile, PlanFileState, TodoItem, TodoKind,
        TodoStatus,
    },
    NextAction, PlanRuntime,
};

#[tokio::test]
async fn acceptance_in_progress_requests_verify() {
    let mut frontmatter = sample_frontmatter();
    frontmatter.state = PlanFileState::Executing;
    frontmatter.todos = vec![TodoItem {
        id: "acceptance".into(),
        content: "验收".into(),
        status: TodoStatus::InProgress,
        evidence: Vec::new(),
        kind: TodoKind::Acceptance,
    }];

    let runtime = PlanRuntime::new("session");
    assert_eq!(
        runtime.next_action(&frontmatter).await,
        NextAction::RunVerify
    );
}

#[test]
fn park_preserves_in_progress_acceptance_todo() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("delivery.plan.md");
    let mut frontmatter = sample_frontmatter();
    frontmatter.state = PlanFileState::Executing;
    frontmatter.todos = vec![TodoItem {
        id: "acceptance".into(),
        content: "验收".into(),
        status: TodoStatus::InProgress,
        evidence: Vec::new(),
        kind: TodoKind::Acceptance,
    }];
    write_plan(
        &path,
        &PlanFile {
            frontmatter,
            body: String::new(),
        },
        1_000,
    )
    .unwrap();

    let runtime = PlanRuntime::new("session");
    runtime.bind_plan_file_for_test(path.clone());
    runtime.park_executing_plan().unwrap();
    let restored = read_plan(&path).unwrap();
    assert_eq!(restored.frontmatter.state, PlanFileState::Pending);
    assert_eq!(restored.frontmatter.todos[0].status, TodoStatus::InProgress);
}

#[test]
fn historical_gate_kind_deserializes_as_unknown_and_old_fields_round_trip() {
    let plan = parse_plan_file(
        r#"---
plan_id: legacy
goal: legacy plan
state: pending
created_at: 2026-01-01T00:00:00Z
schema_version: 1
todos:
  - id: legacy-gate
    content: old gate
    status: pending
    kind: gate_acceptance
acceptance_commands: ["cargo test"]
green_build_pass: true
---
legacy body
"#,
    )
    .unwrap();

    assert_eq!(plan.frontmatter.todos[0].kind, TodoKind::Unknown);
    assert!(plan.frontmatter.unknown.contains_key("acceptance_commands"));
    assert!(plan.frontmatter.unknown.contains_key("green_build_pass"));
}
