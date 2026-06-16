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

fn make_executor_with_bash_timeout(
    definition: &Path,
    timeout_ms: u64,
) -> Arc<dyn PrimitiveExecutor> {
    Arc::new(
        DefaultPrimitiveExecutor::new(
            PrimitiveConfig::default(),
            Arc::new(AllowAllConfirmation),
            Arc::new(TracingAuditRecorder),
            make_gate(definition),
        )
        .with_bash_timeout_ms(timeout_ms),
    )
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

#[tokio::test]
async fn bash_contract_returns_warning_for_background_pipe_holder_without_hanging() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().canonicalize().unwrap();
    // 留出足够窗口让前台 `echo done` 稳定落到输出里，再验证后台子进程持有管道时
    // 依旧会被 timeout 收敛为带 warning 的成功回执，而不是挂死整个 tool call。
    let primitive = make_executor_with_bash_timeout(&root, 300);
    let tc = ToolCallInfo {
        id: "tc-bash-bg-pipe-holder".to_string(),
        name: "bash".to_string(),
        arguments: serde_json::json!({
            "command": "sleep 30 & echo done",
            "cwd": root.display().to_string(),
            "timeout_ms": 300
        })
        .to_string(),
    };

    let (text, is_error, _) = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        execute_tool(&primitive, &None, &None, None, &tc),
    )
    .await
    .expect("tool-exec 不应因后台残留而挂死");

    assert!(
        !is_error,
        "后台残留清理属于带 warning 的成功回执，不应变成 tool error: {}",
        text
    );
    assert!(text.contains("done"), "应保留前台 stdout，实际: {}", text);
    assert!(
        text.contains("run_in_background=true"),
        "用户可见文本应提示长任务改走后台机制，实际: {}",
        text
    );
    assert!(
        text.contains("(exit code: 0)"),
        "应保留既有 bash 展示格式，实际: {}",
        text
    );
}
