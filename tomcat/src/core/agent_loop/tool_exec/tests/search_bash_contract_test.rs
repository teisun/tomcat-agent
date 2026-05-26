use std::path::Path;
use std::sync::Arc;

use super::super::{execute_tool, ToolCallInfo};
use crate::core::permission::{DefaultPermissionGate, GateConfig, PermissionGate, SessionGrants};
use crate::core::tools::primitive::{DefaultPrimitiveExecutor, PrimitiveExecutor};
use crate::core::AllowAllConfirmation;
use crate::infra::{PrimitiveConfig, TracingAuditRecorder};

fn make_gate(definition: &Path) -> Arc<dyn PermissionGate> {
    DefaultPermissionGate::new(
        GateConfig {
            agent_definition_dir: definition.to_path_buf(),
            workspace_roots: vec![],
            agent_trail_readonly_dirs: vec![],
            user_path_rules: vec![],
            user_bash_forbidden: vec![],
            user_bash_approval: vec![],
            auto_confirm: false,
        },
        SessionGrants::new(),
    )
    .into_arc()
}

fn make_executor(definition: &Path) -> Arc<dyn PrimitiveExecutor> {
    Arc::new(DefaultPrimitiveExecutor::new(
        PrimitiveConfig::default(),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(definition),
    ))
}

#[tokio::test]
async fn search_files_contract_ignores_empty_type_string() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().canonicalize().unwrap();
    std::fs::write(root.join("README.md"), "needle\n").unwrap();
    let primitive = make_executor(&root);
    let tc = ToolCallInfo {
        id: "tc-search-empty-type".to_string(),
        name: "search_files".to_string(),
        arguments: serde_json::json!({
            "pattern": "needle",
            "path": root.display().to_string(),
            "glob": "*.md",
            "type": "",
            "output_mode": "files_with_matches"
        })
        .to_string(),
    };

    let (text, is_error, _) = execute_tool(&primitive, &None, &None, None, &tc).await;
    assert!(!is_error, "search_files 空 type 不应报错: {}", text);

    let value: serde_json::Value = serde_json::from_str(&text).expect("valid search_files json");
    assert_eq!(value["query"]["fileType"], serde_json::Value::Null);
    assert_eq!(value["query"]["glob"], "*.md");
    assert!(
        value["files"][0]
            .as_str()
            .unwrap_or_default()
            .ends_with("README.md"),
        "返回文件应指向 README.md，实际: {}",
        value["files"][0]
    );
}

#[tokio::test]
async fn bash_contract_surfaces_cwd_context_in_user_visible_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().canonicalize().unwrap();
    let primitive = make_executor(&root);
    let raw_cwd = "$HOME/this-does-not-exist";
    let tc = ToolCallInfo {
        id: "tc-bash-bad-cwd".to_string(),
        name: "bash".to_string(),
        arguments: serde_json::json!({
            "command": "echo hi",
            "cwd": raw_cwd
        })
        .to_string(),
    };

    let (text, is_error, _) = execute_tool(&primitive, &None, &None, None, &tc).await;
    assert!(is_error, "坏 cwd 应返回 tool error");
    assert!(text.contains("bash.cwd does not exist:"), "实际: {}", text);
    assert!(
        text.contains(&format!("input: {:?}", raw_cwd)),
        "实际: {}",
        text
    );
    assert!(
        text.contains("environment variables are not expanded here"),
        "实际: {}",
        text
    );
    assert!(
        !text.contains("No such file or directory (os error 2)"),
        "不应再回退成裸 os error 2: {}",
        text
    );
}
