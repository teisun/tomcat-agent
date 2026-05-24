//! file_store unit tests — §9.3B（P2）。
#![allow(unused_imports)]

pub(super) use super::*;
pub(super) use fs2::FileExt;
pub(super) use std::sync::Arc;
pub(super) use std::sync::atomic::Ordering;
pub(super) use std::time::{Duration, Instant};

mod file_store;
mod frontmatter;
mod plan_path;

pub(super) fn sample_frontmatter() -> PlanFileFrontmatter {
    PlanFileFrontmatter {
        plan_id: "demo_plan_1".to_string(),
        goal: "为 chat 模式补齐 todos 与 /plan 闭环".to_string(),
        mode: PlanFileMode::Planning,
        session_key: None,
        session_id: None,
        created_at: "2026-05-19T10:00:00+08:00".to_string(),
        schema_version: PLAN_FILE_SCHEMA_VERSION,
        todos: vec![
            TodoItem {
                id: "t1".into(),
                content: "step 1".into(),
                status: TodoStatus::Pending,
            },
            TodoItem {
                id: "t2".into(),
                content: "step 2".into(),
                status: TodoStatus::InProgress,
            },
        ],
        unknown: serde_yaml::Mapping::new(),
    }
}

pub(super) fn temp_plans_dir() -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!(
        "tomcat_plan_test_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&p).unwrap();
    p
}
