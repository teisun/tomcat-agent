use super::super::file_store::{
    write_plan, PlanFile, PlanFileFrontmatter, PlanFileMode, TodoItem, TodoStatus,
};
use super::super::{safety, PlanRuntime};

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
