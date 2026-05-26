mod catalog_test;
mod completion_flow_test;
mod dispatch_test;
mod file_store_frontmatter_test;
mod file_store_plan_path_test;
mod file_store_test;
mod ops_test;
mod prod_reviewer_test;
mod refresh_signals_test;
mod review_test;
mod runtime_state_test;
mod safety_test;
mod todo_runtime_test;
mod verify_test;

pub(super) fn sample_frontmatter() -> super::file_store::PlanFileFrontmatter {
    super::file_store::PlanFileFrontmatter {
        plan_id: "demo_plan_1".to_string(),
        goal: "为 chat 模式补齐 todos 与 /plan 闭环".to_string(),
        state: super::file_store::PlanFileState::Planning,
        session_key: None,
        session_id: None,
        created_at: "2026-05-19T10:00:00+08:00".to_string(),
        schema_version: super::file_store::PLAN_FILE_SCHEMA_VERSION,
        todos: vec![
            super::file_store::TodoItem {
                id: "t1".into(),
                content: "step 1".into(),
                status: super::file_store::TodoStatus::Pending,
            },
            super::file_store::TodoItem {
                id: "t2".into(),
                content: "step 2".into(),
                status: super::file_store::TodoStatus::InProgress,
            },
        ],
        unknown: serde_yaml::Mapping::new(),
    }
}

pub(super) fn temp_plans_dir() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "tomcat_plan_test_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}
