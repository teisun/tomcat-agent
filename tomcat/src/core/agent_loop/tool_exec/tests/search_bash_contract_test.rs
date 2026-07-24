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
    foreground_wait_ms: u64,
) -> Arc<dyn PrimitiveExecutor> {
    Arc::new(
        DefaultPrimitiveExecutor::new(
            PrimitiveConfig::default(),
            Arc::new(AllowAllConfirmation),
            Arc::new(TracingAuditRecorder),
            make_gate(definition),
        )
        .with_bash_foreground_wait_ms(foreground_wait_ms),
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
async fn bash_contract_stops_pipe_holder_without_hanging_when_no_registry() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().canonicalize().unwrap();
    let marker = root.join("pipe-holder-leak.txt");
    // 后台 child 持有 stdout/stderr 写端；若前台等待到期后不 kill 整个进程组，
    // 这条子进程会继续活到 9s 并写 marker，形成 runaway。
    let primitive = make_executor_with_bash_timeout(&root, 300);
    let tc = ToolCallInfo {
        id: "tc-bash-bg-pipe-holder".to_string(),
        name: "bash".to_string(),
        arguments: serde_json::json!({
            "command": format!("sleep 9 && printf leaked > {} & echo done", marker.display()),
            "cwd": root.display().to_string(),
            "foreground_wait_ms": 300
        })
        .to_string(),
    };

    let (text, is_error, _) = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        execute_tool(&primitive, &None, &None, None, &tc),
    )
    .await
    .expect("前台等待窗口到期即返回，绝不能挂死到后台 child 自然退出");

    assert!(
        !is_error,
        "到期就地收口仍是成功回执，不应变成 tool error: {}",
        text
    );
    assert!(text.contains("done"), "应保留前台 stdout，实际: {}", text);
    assert!(
        text.contains("stopped in this context"),
        "应明示该上下文已停止命令，实际: {}",
        text
    );
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    assert!(
        !marker.exists(),
        "marker 出现表示等待到期后的后台 child 仍在继续跑"
    );
}
