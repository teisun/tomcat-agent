mod catalog;
mod completion_flow;
mod dispatch;
mod file_store;
mod file_store_frontmatter;
mod file_store_plan_path;
mod ops;
mod prod_reviewer;
mod refresh_signals;
mod review;
mod runtime_state;
mod safety;
mod todo_runtime;
mod verify;

pub(super) fn sample_frontmatter() -> super::file_store::PlanFileFrontmatter {
    super::file_store::PlanFileFrontmatter {
        plan_id: "demo_plan_1".to_string(),
        goal: "为 chat 模式补齐 todos 与 /plan 闭环".to_string(),
        mode: super::file_store::PlanFileMode::Planning,
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
