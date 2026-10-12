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
async fn edit_success_returns_bounded_current_disk_view_for_the_next_edit() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().canonicalize().unwrap();
    let path = root.join("edit-feedback.txt");
    std::fs::write(&path, "before\nold value\nafter\n").expect("write fixture");
    let primitive = make_executor(&root);
    let first = ToolCallInfo {
        id: "tc-edit-feedback-first".to_string(),
        name: "edit".to_string(),
        arguments: serde_json::json!({
            "path": path,
            "edits": [{"old_content": "old value", "new_content": "new value"}]
        })
        .to_string(),
    };

    let (text, is_error, _) = execute_tool(&primitive, &None, &None, None, &first).await;
    assert!(!is_error, "edit 应成功: {text}");
    assert!(
        text.contains("Post-edit view"),
        "必须回喂当前磁盘视图: {text}"
    );
    for expected in ["     1\tbefore", "     2\tnew value", "     3\tafter"] {
        assert!(text.contains(expected), "视图缺少 {expected:?}: {text}");
    }
    assert!(text.len() <= 6 * 1024, "单文件反馈必须有界: {}", text.len());

    let second = ToolCallInfo {
        id: "tc-edit-feedback-second".to_string(),
        name: "edit".to_string(),
        arguments: serde_json::json!({
            "path": path,
            "edits": [{"old_content": "new value", "new_content": "final value"}]
        })
        .to_string(),
    };
    let (text, is_error, _) = execute_tool(&primitive, &None, &None, None, &second).await;
    assert!(!is_error, "使用回喂原文的连续编辑应成功: {text}");
    assert_eq!(
        std::fs::read_to_string(path).expect("read result"),
        "before\nfinal value\nafter\n"
    );
}

#[tokio::test]
async fn bash_and_edit_sequence_needs_no_recovery_turns_after_contract_fixes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().canonicalize().unwrap();
    let script = root.join("参数回显.mjs");
    let argument = "含空格的参数";
    std::fs::write(&script, "process.stdout.write(process.argv[2]);\n").expect("write script");
    let file = root.join("continuation.txt");
    std::fs::write(&file, "before\nold value\nafter\n").expect("write fixture");
    let primitive = make_executor(&root);

    let bash = ToolCallInfo {
        id: "tc-contract-sequence-bash".to_string(),
        name: "bash".to_string(),
        arguments: serde_json::json!({
            "command": format!("node {} \"{argument}\"", script.display()),
            "cwd": root.display().to_string()
        })
        .to_string(),
    };
    let (bash_text, bash_error, _) = execute_tool(&primitive, &None, &None, None, &bash).await;
    assert!(
        !bash_error,
        "单一 command 的 CJK 多 token bash 应成功: {bash_text}"
    );
    assert!(
        bash_text.contains(argument),
        "bash 必须回传脚本输出: {bash_text}"
    );

    let first_edit = ToolCallInfo {
        id: "tc-contract-sequence-edit-first".to_string(),
        name: "edit".to_string(),
        arguments: serde_json::json!({
            "path": file,
            "edits": [{"old_content": "old value", "new_content": "new value"}]
        })
        .to_string(),
    };
    let (first_text, first_error, _) =
        execute_tool(&primitive, &None, &None, None, &first_edit).await;
    assert!(!first_error, "首次 edit 应成功: {first_text}");
    assert!(
        first_text.contains("     2\tnew value"),
        "首次 edit 必须回喂可直接续编的当前文本: {first_text}"
    );

    let second_edit = ToolCallInfo {
        id: "tc-contract-sequence-edit-second".to_string(),
        name: "edit".to_string(),
        arguments: serde_json::json!({
            "path": file,
            "edits": [{"old_content": "new value", "new_content": "final value"}]
        })
        .to_string(),
    };
    let (second_text, second_error, _) =
        execute_tool(&primitive, &None, &None, None, &second_edit).await;
    assert!(
        !second_error,
        "使用回喂文本续编不应需要 NotFound 恢复回合: {second_text}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("continuation.txt")).expect("read result"),
        "before\nfinal value\nafter\n"
    );
}

#[tokio::test]
async fn edit_notfound_returns_nearby_current_disk_view_when_an_old_line_is_unique() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().canonicalize().unwrap();
    let path = root.join("edit-notfound-feedback.txt");
    std::fs::write(&path, "function target() {\n  return \"new\";\n}\n").expect("write fixture");
    let primitive = make_executor(&root);
    let stale = ToolCallInfo {
        id: "tc-edit-notfound-feedback".to_string(),
        name: "edit".to_string(),
        arguments: serde_json::json!({
            "path": path,
            "edits": [{
                "old_content": "function target() {\n  return \"old\";\n}",
                "new_content": "function target() {\n  return \"final\";\n}"
            }]
        })
        .to_string(),
    };

    let (text, is_error, _) = execute_tool(&primitive, &None, &None, None, &stale).await;
    assert!(is_error, "NotFound 必须保持失败语义: {text}");
    assert!(text.contains("NotFound:"), "应保留原始错误分类: {text}");
    assert!(
        text.contains("Nearby current view for old_content"),
        "NotFound 应直接回喂可纠正的当前视图: {text}"
    );
    for expected in [
        "     1\tfunction target() {",
        "     2\t  return \"new\";",
        "     3\t}",
    ] {
        assert!(text.contains(expected), "就近视图缺少 {expected:?}: {text}");
    }
    assert!(text.len() <= 6 * 1024, "失败回馈必须有界: {}", text.len());

    let corrected = ToolCallInfo {
        id: "tc-edit-notfound-corrected".to_string(),
        name: "edit".to_string(),
        arguments: serde_json::json!({
            "path": path,
            "edits": [{
                "old_content": "function target() {\n  return \"new\";\n}",
                "new_content": "function target() {\n  return \"final\";\n}"
            }]
        })
        .to_string(),
    };
    let (text, is_error, _) = execute_tool(&primitive, &None, &None, None, &corrected).await;
    assert!(!is_error, "根据回喂真相重试必须成功: {text}");
}

#[tokio::test]
async fn edit_notfound_without_a_reliable_anchor_requests_a_fresh_read() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().canonicalize().unwrap();
    let path = root.join("edit-notfound-unanchored.txt");
    std::fs::write(&path, "current file content\n").expect("write fixture");
    let primitive = make_executor(&root);
    let stale = ToolCallInfo {
        id: "tc-edit-notfound-unanchored".to_string(),
        name: "edit".to_string(),
        arguments: serde_json::json!({
            "path": path,
            "edits": [{"old_content": "unrelated stale text", "new_content": "replacement"}]
        })
        .to_string(),
    };

    let (text, is_error, _) = execute_tool(&primitive, &None, &None, None, &stale).await;
    assert!(is_error, "NotFound 必须保持失败语义: {text}");
    assert!(
        text.contains(
            "Could not reliably locate the current area from old_content; read the file again"
        ),
        "无法可靠定位时必须明确要求重新读取: {text}"
    );
    assert!(
        !text.contains("Nearby current view for old_content"),
        "不能猜测目标区域并伪造当前视图: {text}"
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

#[tokio::test]
async fn search_files_content_mode_defaults_to_three_context_lines() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().canonicalize().unwrap();
    let body: String = (1..=11)
        .map(|i| {
            if i == 6 {
                "needle\n".to_string()
            } else {
                format!("line {i}\n")
            }
        })
        .collect();
    std::fs::write(root.join("a.txt"), body).unwrap();
    let primitive = make_executor(&root);

    let with_default = ToolCallInfo {
        id: "tc-search-default-context".to_string(),
        name: "search_files".to_string(),
        arguments: serde_json::json!({
            "pattern": "needle",
            "path": root.display().to_string(),
            "output_mode": "content"
        })
        .to_string(),
    };
    let (text, is_error, _) = execute_tool(&primitive, &None, &None, None, &with_default).await;
    assert!(!is_error, "search_files 应成功: {}", text);
    // 命中行单独看往往不够判断，默认带 ±3 行省掉紧跟着的那次 read。
    for line in ["line 3", "line 5", "needle", "line 7", "line 9"] {
        assert!(text.contains(line), "缺少 {line}，实际: {text}");
    }
    assert!(!text.contains("line 2"), "上下文不应超过 3 行: {text}");

    let opted_out = ToolCallInfo {
        id: "tc-search-no-context".to_string(),
        name: "search_files".to_string(),
        arguments: serde_json::json!({
            "pattern": "needle",
            "path": root.display().to_string(),
            "output_mode": "content",
            "context": 0
        })
        .to_string(),
    };
    let (text, is_error, _) = execute_tool(&primitive, &None, &None, None, &opted_out).await;
    assert!(!is_error, "search_files 应成功: {}", text);
    assert!(text.contains("needle"));
    assert!(
        !text.contains("line 5") && !text.contains("line 7"),
        "显式 context=0 必须能关掉默认上下文: {text}"
    );
}
