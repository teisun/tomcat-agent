use super::super::file_store::{
    write_plan, PlanFile, PlanFileFrontmatter, PlanFileState, TodoItem, TodoStatus,
};
use super::super::code_reviewer::code_reviewer_allowed_tools_with_policy;
use super::super::plan_reviewer::{build_review_prompt, plan_reviewer_allowed_tools_with_policy};
use super::super::prod_reviewer::{ProdCodeReviewerDispatcher, ProdPlanReviewerDispatcher};
use super::super::review::resolve_internal_tools;
use super::super::{CodeReviewerDispatcher, PlanReviewerDispatcher, PlanRuntime};
use crate::core::tools::contract::catalog::BUILTIN_TOOL_CATALOG;

#[tokio::test]
async fn prod_plan_reviewer_stub_returns_aborted_with_origin() {
    let d = ProdPlanReviewerDispatcher::stub("test_origin");
    let r = d.dispatch("demo", "noop", true).await;
    assert!(r.aborted);
    assert!(r.summary.contains("test_origin"));
    assert!(!r.applied_changes);
}

#[tokio::test]
async fn prod_code_reviewer_stub_returns_aborted_with_origin() {
    let d = ProdCodeReviewerDispatcher::stub("test_origin");
    let r = d.dispatch("demo", "noop").await;
    assert!(r.aborted);
    assert_eq!(r.verdict.as_deref(), Some("aborted"));
    assert!(r.summary.contains("test_origin"));
    assert!(!r.applied_changes);
}

#[test]
fn reviewer_not_in_catalog() {
    for entry in BUILTIN_TOOL_CATALOG.iter() {
        assert_ne!(entry.name, "reviewer", "catalog 不应暴露 `reviewer` 工具");
        assert_ne!(entry.name, "review", "catalog 不应暴露 `review` 工具");
    }
}

#[test]
fn reviewer_default_allowed_tools_no_create_plan() {
    let tools = resolve_internal_tools(&plan_reviewer_allowed_tools_with_policy(false));
    let names: std::collections::BTreeSet<String> = tools
        .iter()
        .map(|v| v["function"]["name"].as_str().unwrap().to_string())
        .collect();
    assert!(!names.contains("create_plan"));
    assert!(!names.contains("bash"));
    assert!(!names.contains("write"));
    assert!(!names.contains("dispatch_agent"));
    assert!(!names.contains("checkpoint"));
    assert!(names.contains("update_plan"));
    assert!(names.contains("edit"));
    assert!(names.contains("read"));
}

#[test]
fn code_reviewer_allowed_tools_include_bash_only_in_code_mode() {
    let tools = resolve_internal_tools(&code_reviewer_allowed_tools_with_policy(false));
    let names: std::collections::BTreeSet<String> = tools
        .iter()
        .map(|v| v["function"]["name"].as_str().unwrap().to_string())
        .collect();
    assert!(names.contains("bash"));
    assert!(!names.contains("todos"));
    assert!(!names.contains("update_plan"));
    assert!(!names.contains("edit"));
}

#[test]
fn reviewer_can_expose_load_skill_when_config_enabled() {
    let plan_tools = resolve_internal_tools(&plan_reviewer_allowed_tools_with_policy(true));
    let code_tools = resolve_internal_tools(&code_reviewer_allowed_tools_with_policy(true));
    let plan_names: std::collections::BTreeSet<String> = plan_tools
        .iter()
        .map(|v| v["function"]["name"].as_str().unwrap().to_string())
        .collect();
    let code_names: std::collections::BTreeSet<String> = code_tools
        .iter()
        .map(|v| v["function"]["name"].as_str().unwrap().to_string())
        .collect();
    assert!(plan_names.contains("load_skill"));
    assert!(code_names.contains("load_skill"));
}

#[test]
fn reviewer_max_turns_default_is_64() {
    let config = crate::infra::config::ReviewerConfig::default();
    assert_eq!(config.max_turns, 64);
}

#[test]
fn review_prompt_uses_active_external_plan_path() {
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
                state: PlanFileState::Planning,
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
    let resolved = runtime.resolved_plan_path("external_path_plan").unwrap();
    let resolved_display = crate::infra::platform::format_home_path(&resolved);
    let prompt = build_review_prompt("external_path_plan", "body", &resolved, None);
    assert_eq!(
        resolved,
        crate::normalize_path(&external_path.to_string_lossy()).unwrap()
    );
    assert!(prompt.contains(&resolved_display));
}
