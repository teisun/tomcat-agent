use super::super::file_store::{
    write_plan, PlanFile, PlanFileFrontmatter, PlanFileMode, TodoItem, TodoStatus,
};
use super::super::{safety, session_prefix, PlanMode, PlanRuntime};

#[test]
fn safety_assert_plan_id_safe_accepts_normal_id() {
    safety::assert_plan_id_safe("ship-plan-mode_001").unwrap();
}

#[test]
fn safety_assert_plan_id_safe_rejects_traversal_paths() {
    let bad = [
        "",
        "..",
        "../etc",
        "a/b",
        "a\\b",
        "a b",
        "A",
        "ship!",
        "ship\nbad",
    ];
    for id in bad {
        let r = safety::assert_plan_id_safe(id);
        assert!(
            r.is_err(),
            "should reject unsafe plan_id {id:?}, got: {r:?}"
        );
    }
}

#[test]
fn session_prefix_for_chat_is_empty() {
    assert!(session_prefix::user_prefix_for_mode(&PlanMode::Chat, None).is_empty());
}

#[test]
fn session_prefix_for_planning_carries_plan_path_when_present() {
    let p = session_prefix::user_prefix_for_mode(
        &PlanMode::Planning,
        Some(std::path::Path::new("/tmp/active.plan.md")),
    );
    assert!(p.starts_with("[mode: PLAN "));
    assert!(p.contains("plan_path=/tmp/active.plan.md"));
}

#[test]
fn session_prefix_for_executing_carries_plan_id() {
    let p = session_prefix::user_prefix_for_mode(
        &PlanMode::Executing {
            plan_id: "ship-001".into(),
        },
        Some(std::path::Path::new("/tmp/exec.plan.md")),
    );
    assert!(p.contains("[mode: EXEC plan_id=ship-001"));
    assert!(p.contains("plan_path=/tmp/exec.plan.md"));
}

#[test]
fn session_prefix_for_pending_is_empty() {
    let p = session_prefix::user_prefix_for_mode(
        &PlanMode::Pending {
            plan_id: "ship-001".into(),
        },
        None,
    );
    assert!(p.is_empty(), "pending must NOT prefix LLM input");
}

#[test]
fn strip_user_prefix_removes_plan_label_when_present() {
    let s = session_prefix::strip_user_prefix("[mode: PLAN]\nhello world");
    assert_eq!(s, "hello world");
}

#[test]
fn strip_user_prefix_removes_exec_label_with_plan_id() {
    let s = session_prefix::strip_user_prefix("[mode: EXEC plan_id=ship-001]\nstart please");
    assert_eq!(s, "start please");
}

#[test]
fn strip_user_prefix_passthrough_when_no_label() {
    let s = session_prefix::strip_user_prefix("normal user text\nwith linebreak");
    assert_eq!(s, "normal user text\nwith linebreak");
}

#[test]
fn strip_user_prefix_passthrough_when_lookalike_does_not_close() {
    let s = session_prefix::strip_user_prefix("[mode: malformed\nrest of text");
    assert_eq!(s, "[mode: malformed\nrest of text");
}

#[test]
fn resolved_plan_path_prefers_active_external_path() {
    let workspace = tempfile::tempdir().unwrap();
    let external_dir = workspace.path().join("external");
    std::fs::create_dir_all(&external_dir).unwrap();
    let external_path = external_dir.join("custom.plan.md");
    write_plan(
        &external_path,
        &PlanFile {
            frontmatter: PlanFileFrontmatter {
                plan_id: "external_path_plan".into(),
                goal: "goal".into(),
                mode: PlanFileMode::Planning,
                session_key: Some("sess".into()),
                session_id: Some("uuid".into()),
                created_at: "2026-05-24T00:00:00Z".into(),
                schema_version: 1,
                todos: vec![TodoItem {
                    id: "t1".into(),
                    content: "ship".into(),
                    status: TodoStatus::Pending,
                }],
                unknown: Default::default(),
            },
            body: "## Goal\nexternal\n".into(),
        },
        1000,
    )
    .unwrap();

    let runtime = PlanRuntime::new("sess");
    runtime
        .build_plan(&external_path.to_string_lossy(), Some("uuid-path".into()))
        .unwrap();

    assert_eq!(
        runtime.resolved_plan_path("external_path_plan").unwrap(),
        crate::normalize_path(&external_path.to_string_lossy()).unwrap()
    );
}
