use super::super::*;
use crate::core::permission::{DefaultPermissionGate, GateConfig, PermissionGate, SessionGrants};
use crate::core::tools::primitive::{
    EditOperation, EditOperationType, PrimitiveExecutor, PrimitiveOperation, SearchFilesArgs,
    SearchFilesOutputMode, SearchFilesTarget,
};
use crate::core::{AllowAllConfirmation, DenyAllConfirmation};
use crate::infra::error::AppError;
use crate::infra::{
    AuditPrimitiveOp, AuditRecorder, PrimitiveAuditEntry, PrimitiveConfig, ToolAuditEntry,
    TracingAuditRecorder,
};
use serial_test::serial;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

fn temp_primitive_config(_dir: &Path) -> PrimitiveConfig {
    PrimitiveConfig::default()
}

/// 测试 helper：把 `dir` 作为 `agent_definition_dir`（默认 writable）构造 gate。
fn make_gate(definition: &Path) -> Arc<dyn PermissionGate> {
    make_gate_with(definition, vec![], false)
}

fn make_gate_with(
    definition: &Path,
    workspace_roots: Vec<PathBuf>,
    auto_confirm: bool,
) -> Arc<dyn PermissionGate> {
    DefaultPermissionGate::new(
        GateConfig {
            agent_definition_dir: definition.to_path_buf(),
            workspace_roots,
            agent_trail_readonly_dirs: vec![],
            user_path_rules: vec![],
            user_bash_forbidden: vec![],
            user_bash_approval: vec![],
            auto_confirm,
        },
        SessionGrants::new(),
    )
    .into_arc()
}

struct CurrentDirGuard {
    _lock: crate::test_support::TestLockGuard<'static>,
    previous: PathBuf,
}

impl CurrentDirGuard {
    fn set(path: &Path) -> Self {
        let lock = crate::test_support::cwd_lock().lock().unwrap();
        let previous = std::env::current_dir().expect("current_dir");
        std::env::set_current_dir(path).expect("set_current_dir");
        Self {
            _lock: lock,
            previous,
        }
    }
}

impl Drop for CurrentDirGuard {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.previous);
    }
}

#[tokio::test]
async fn read_file_success() {
    let dir = std::env::temp_dir().join("tomcat_exec_read");
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let f = dir.join("f.txt");
    std::fs::write(&f, "hello").unwrap();
    let path_str = f.to_string_lossy().to_string();
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    );
    let out = exec.read_file(&path_str, "p1").await.unwrap();
    assert_eq!(out, "hello");
    let _ = std::fs::remove_file(&f);
    let _ = std::fs::remove_dir(&dir);
}

#[tokio::test]
async fn read_file_missing_path_returns_not_found_error() {
    let dir = std::env::temp_dir().join("tomcat_exec_read_missing");
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let missing = dir.join("missing_file_for_read.txt");
    let path_str = missing.to_string_lossy().to_string();
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    );
    let err = exec
        .read_file(&path_str, "p1")
        .await
        .expect_err("读取不存在路径应返回错误");
    let msg = err.to_string().to_ascii_lowercase();
    assert!(
        msg.contains("no such file")
            || msg.contains("not found")
            || msg.contains("os error 2")
            || msg.contains("不存在"),
        "错误文案应包含路径不存在语义，实际: {}",
        err
    );
    let _ = std::fs::remove_dir(&dir);
}

#[tokio::test]
async fn read_file_binary_returns_product_error() {
    let dir = std::env::temp_dir().join("tomcat_exec_binary_read");
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let f = dir.join("image.png");
    std::fs::write(&f, [0xff, 0xfe, 0x00, 0x01]).unwrap();
    let path_str = f.to_string_lossy().to_string();
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    );

    let err = exec.read_file(&path_str, "p1").await.unwrap_err();
    match err {
        AppError::Primitive(msg) => {
            assert!(msg.contains("文件存在且权限已通过检查"));
            assert!(msg.contains("二进制或非 UTF-8 文本"));
        }
        other => panic!("expected product primitive error, got {:?}", other),
    }
    let _ = std::fs::remove_file(&f);
    let _ = std::fs::remove_dir(&dir);
}

#[tokio::test]
async fn read_file_outside_writable_set_user_denied_returns_permission() {
    // gate 模式：路径不在 writable 集合内 → NeedConfirm；用户拒绝 → Permission。
    let exec = DefaultPrimitiveExecutor::new(
        PrimitiveConfig::default(),
        Arc::new(DenyAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&PathBuf::from("/nonexistent_pi_workspace")),
    );
    let r = exec.read_file("/tmp/any", "p1").await;
    assert!(r.is_err());
    assert!(matches!(r.unwrap_err(), AppError::Permission(_)));
}

#[tokio::test]
async fn list_dir_success() {
    let dir = std::env::temp_dir().join("tomcat_exec_list");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("f.txt"), "").unwrap();
    let dir = dir.canonicalize().unwrap();
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    );
    let path_str = dir.to_string_lossy().to_string();
    let entries = exec.list_dir(&path_str, "p1").await.unwrap();
    assert!(!entries.is_empty());
    let _ = std::fs::remove_file(dir.join("f.txt"));
    let _ = std::fs::remove_dir(&dir);
}

#[tokio::test]
async fn write_file_success() {
    let dir = std::env::temp_dir().join("tomcat_exec_write");
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let f = dir.join("w.txt");
    let path_str = f.to_string_lossy().to_string();
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    );
    let res = exec
        .write_file(&path_str, "content", false, "p1")
        .await
        .unwrap();
    assert!(res.written);
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "content");
    let _ = std::fs::remove_file(&f);
    let _ = std::fs::remove_dir(&dir);
}

#[tokio::test]
async fn write_file_with_cancel_skips_disk_write() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dir = dir.path().canonicalize().unwrap();
    let f = dir.join("cancel-write.txt");
    std::fs::write(&f, "old").unwrap();
    let path_str = f.to_string_lossy().to_string();
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    );
    let cancel = CancellationToken::new();
    cancel.cancel();

    let result = exec
        .write_file_with_cancel(&path_str, "new", true, &cancel, "p1")
        .await
        .expect("cancelled write should return a structured non-write result");
    assert!(!result.written);
    assert_eq!(result.bytes_written, 0);
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "old");
}

#[tokio::test]
async fn write_file_user_denied_returns_permission_and_audit() {
    let dir = std::env::temp_dir().join("tomcat_exec_deny");
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let f = dir.join("d.txt");
    std::fs::write(&f, "old").unwrap();
    let path_str = f.to_string_lossy().to_string();
    let audit_entries: Arc<Mutex<Vec<PrimitiveAuditEntry>>> = Arc::new(Mutex::new(Vec::new()));
    let audit = Arc::new(DenyAuditRecorder(audit_entries.clone()));
    // 把目标路径放在 writable set 之外（gate 的 agent_definition_dir 指向不存在目录），
    // gate 会返回 NeedConfirm；DenyAllConfirmation 模拟用户拒绝 → Permission。
    let exec = DefaultPrimitiveExecutor::new(
        PrimitiveConfig::default(),
        Arc::new(DenyAllConfirmation),
        audit,
        make_gate(&PathBuf::from("/nonexistent_pi_workspace")),
    );
    let r = exec.write_file(&path_str, "new", true, "p1").await;
    assert!(r.is_err());
    assert!(matches!(r.unwrap_err(), AppError::Permission(_)));
    // 在 gate 模式下 deny 直接由 gate_check_path 内部返回 Permission，不再单独 record
    // 一条 user_approved=false 的审计；此处只断言外层错误正确传播即可。
    drop(audit_entries);
    let _ = std::fs::remove_file(&f);
    let _ = std::fs::remove_dir(&dir);
}

struct DenyAuditRecorder(pub Arc<Mutex<Vec<PrimitiveAuditEntry>>>);
impl AuditRecorder for DenyAuditRecorder {
    fn record_primitive(&self, entry: PrimitiveAuditEntry) {
        self.0.lock().unwrap().push(entry);
    }
    fn record_tool_call(&self, _entry: ToolAuditEntry) {}
    fn record_hostcall(&self, _entry: crate::infra::HostcallAuditEntry) {}
    fn record_plugin_lifecycle(&self, _entry: crate::infra::PluginLifecycleAuditEntry) {}
}

#[tokio::test]
async fn edit_file_success() {
    let dir = std::env::temp_dir().join("tomcat_exec_edit");
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let f = dir.join("e.txt");
    std::fs::write(&f, "line1\nline2\nline3").unwrap();
    let path_str = f.to_string_lossy().to_string();
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    );
    let edits = vec![EditOperation {
        operation_type: EditOperationType::Replace,
        start_line: Some(2),
        end_line: Some(2),
        old_content: Some("line2".to_string()),
        new_content: "replaced".to_string(),
    }];
    let res = exec.edit_file(&path_str, edits, "p1").await.unwrap();
    assert!(res.applied);
    assert_eq!(
        std::fs::read_to_string(&f).unwrap(),
        "line1\nreplaced\nline3"
    );
    let _ = std::fs::remove_file(&f);
    let _ = std::fs::remove_dir(&dir);
}

#[tokio::test]
async fn edit_file_with_cancel_skips_disk_write() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dir = dir.path().canonicalize().unwrap();
    let f = dir.join("cancel-edit.txt");
    std::fs::write(&f, "line1\nline2\nline3\n").unwrap();
    let path_str = f.to_string_lossy().to_string();
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    );
    let cancel = CancellationToken::new();
    cancel.cancel();

    let edits = vec![EditOperation {
        operation_type: EditOperationType::Replace,
        start_line: Some(2),
        end_line: Some(2),
        old_content: Some("line2".to_string()),
        new_content: "changed".to_string(),
    }];
    let result = exec
        .edit_file_with_cancel(&path_str, edits, &cancel, "p1")
        .await
        .expect("cancelled edit should return a structured non-apply result");
    assert!(!result.applied);
    assert_eq!(
        std::fs::read_to_string(&f).unwrap(),
        "line1\nline2\nline3\n"
    );
}

#[tokio::test]
async fn hashline_edit_with_cancel_skips_disk_write() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dir = dir.path().canonicalize().unwrap();
    let f = dir.join("cancel-hashline.txt");
    std::fs::write(&f, "alpha\nbeta\ngamma\n").unwrap();
    let path_str = f.to_string_lossy().to_string();
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    );
    let cancel = CancellationToken::new();
    cancel.cancel();

    let segment = crate::core::tools::primitive::HashlineSegment {
        op: crate::core::tools::primitive::HashlineOp::Replace,
        start_line: 2,
        start_hash: crate::core::tools::primitive::compute_line_hash("beta", 2),
        end_line: 2,
        end_hash: crate::core::tools::primitive::compute_line_hash("beta", 2),
        lines: "changed\n".to_string(),
    };
    let result = exec
        .hashline_edit_with_cancel(&path_str, vec![segment], &cancel, "p1")
        .await
        .expect("cancelled hashline_edit should return a structured non-apply result");
    assert!(!result.applied);
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "alpha\nbeta\ngamma\n");
}

#[tokio::test]
async fn execute_bash_success() {
    let dir = std::env::temp_dir().join("tomcat_exec_bash");
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let path_str = dir.to_string_lossy().to_string();
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    );
    let res = exec
        .execute_bash("echo ok", Some(&path_str), "p1", None, None)
        .await
        .unwrap();
    assert_eq!(res.exit_code, 0);
    assert!(res.stdout.trim().contains("ok"));
    assert!(
        !res.stderr.contains("run_in_background=true"),
        "普通前台命令不应误报后台残留提示，实际 stderr={:?}",
        res.stderr
    );
    let _ = std::fs::remove_dir(&dir);
}

#[tokio::test]
async fn execute_bash_marks_child_as_nested_agent() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dir = dir.path().canonicalize().unwrap();
    let path_str = dir.to_string_lossy().to_string();
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    );
    #[cfg(unix)]
    let command = r#"printf "%s" "${TOMCAT_AGENT_ACTIVE:-missing}""#;
    #[cfg(windows)]
    let command = r#"echo %TOMCAT_AGENT_ACTIVE%"#;

    let res = exec
        .execute_bash(command, Some(&path_str), "p1", None, None)
        .await
        .expect("bash command should succeed");

    assert_eq!(res.exit_code, 0);
    assert_eq!(res.stdout.trim(), "1");
}

#[tokio::test]
#[serial(env_lock)]
async fn execute_bash_empty_string_cwd_treated_as_none() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dir = dir.path().canonicalize().unwrap();
    let _cwd = CurrentDirGuard::set(&dir);
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    );

    let res = exec
        .execute_bash("pwd", Some(""), "p1", None, None)
        .await
        .expect("空 cwd 应视同未传");

    assert_eq!(res.exit_code, 0);
    assert_eq!(res.stdout.trim(), dir.display().to_string());
}

#[tokio::test]
async fn execute_bash_nonexistent_absolute_cwd_returns_clear_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dir = dir.path().canonicalize().unwrap();
    let missing = dir.join("missing-subdir");
    let path_str = missing.display().to_string();
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    );

    let err = exec
        .execute_bash("echo nope", Some(&path_str), "p1", None, None)
        .await
        .expect_err("不存在 cwd 应前置报错");
    let msg = err.to_string();
    assert!(msg.contains("bash.cwd does not exist:"));
    assert!(msg.contains(&path_str));
    assert!(msg.contains(&format!("input: {:?}", path_str)));
}

#[tokio::test]
async fn execute_bash_non_directory_cwd_returns_clear_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dir = dir.path().canonicalize().unwrap();
    let file = dir.join("not-a-dir.txt");
    std::fs::write(&file, "hello").unwrap();
    let path_str = file.display().to_string();
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    );

    let err = exec
        .execute_bash("echo nope", Some(&path_str), "p1", None, None)
        .await
        .expect_err("文件路径不应被接受为 cwd");
    let msg = err.to_string();
    assert!(msg.contains("bash.cwd is not a directory:"));
    assert!(msg.contains(&path_str));
    assert!(msg.contains(&format!("input: {:?}", path_str)));
}

#[tokio::test]
async fn execute_bash_dollar_var_like_cwd_returns_hint_when_path_missing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dir = dir.path().canonicalize().unwrap();
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    );

    let raw_cwd = "$HOME/this-does-not-exist";
    let err = exec
        .execute_bash("echo nope", Some(raw_cwd), "p1", None, None)
        .await
        .expect_err("字面 $HOME 且目录缺失时应返回提示");
    let msg = err.to_string();
    assert!(msg.contains("bash.cwd does not exist:"));
    assert!(msg.contains(&format!("input: {:?}", raw_cwd)));
    assert!(msg.contains("environment variables are not expanded here"));
}

#[tokio::test]
#[serial(env_lock)]
async fn execute_bash_dollar_var_like_cwd_executes_when_real_dir() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dir = dir.path().canonicalize().unwrap();
    let literal = dir.join("$HOME");
    std::fs::create_dir_all(&literal).unwrap();
    let _cwd = CurrentDirGuard::set(&dir);
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    );

    let res = exec
        .execute_bash("pwd", Some("$HOME"), "p1", None, None)
        .await
        .expect("客观存在的 $HOME 字面目录不应被误伤");

    assert_eq!(res.exit_code, 0);
    assert_eq!(res.stdout.trim(), literal.display().to_string());
}

#[tokio::test]
async fn execute_bash_pre_validation_failure_writes_audit() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dir = dir.path().canonicalize().unwrap();
    let missing = dir.join("missing-audit-subdir");
    let path_str = missing.display().to_string();
    let entries = Arc::new(Mutex::new(Vec::<PrimitiveAuditEntry>::new()));
    let audit = Arc::new(DenyAuditRecorder(entries.clone()));
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        audit,
        make_gate(&dir),
    );

    let err = exec
        .execute_bash("echo nope", Some(&path_str), "p1", None, None)
        .await
        .expect_err("不存在 cwd 应写失败审计");
    assert!(err.to_string().contains("bash.cwd does not exist:"));

    let entries = entries.lock().unwrap();
    assert_eq!(entries.len(), 1, "前置校验失败应写一条审计");
    let entry = &entries[0];
    assert_eq!(entry.operation, AuditPrimitiveOp::Bash);
    assert_eq!(entry.path_or_cmd, "echo nope");
    assert!(!entry.success);
    assert!(
        entry
            .detail
            .as_deref()
            .unwrap_or_default()
            .contains("bash.cwd does not exist:"),
        "detail 应记录前置校验错误，实际: {:?}",
        entry.detail
    );
}

#[tokio::test]
async fn execute_bash_spawn_error_includes_cwd_and_input() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dir = dir.path().canonicalize().unwrap();
    let path_str = dir.display().to_string();
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    );
    let argv = vec!["--version".to_string()];

    let err = exec
        .execute_bash(
            "definitely_missing_binary_for_bash_test",
            Some(&path_str),
            "p1",
            Some(&argv),
            None,
        )
        .await
        .expect_err("缺失 binary 应走 spawn 失败路径");
    let msg = err.to_string();
    assert!(msg.contains("bash spawn failed"));
    assert!(msg.contains(&format!("cwd={}", path_str)));
    assert!(msg.contains(&format!("input={:?}", path_str)));
}

#[test]
fn tokio_spawn_with_dollar_home_returns_enoent_when_literal_path_missing() {
    let err = tokio::process::Command::new("sh")
        .arg("-c")
        .arg("pwd")
        .current_dir("$HOME/definitely-missing-literal")
        .spawn()
        .expect_err("tokio spawn 应返回 ENOENT");
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
}

#[tokio::test]
async fn execute_bash_empty_argv_uses_shell_mode() {
    let dir = std::env::temp_dir().join("tomcat_exec_bash_empty_argv");
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let path_str = dir.to_string_lossy().to_string();
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    );
    let empty_args: Vec<String> = vec![];
    let res = exec
        .execute_bash(
            "echo empty-argv-ok",
            Some(&path_str),
            "p1",
            Some(&empty_args),
            None,
        )
        .await
        .unwrap();
    assert_eq!(res.exit_code, 0);
    assert!(res.stdout.trim().contains("empty-argv-ok"));
    let _ = std::fs::remove_dir(&dir);
}

#[tokio::test]
async fn execute_bash_shell_launcher_command_merges_with_argv() {
    let dir = std::env::temp_dir().join("tomcat_exec_bash_shell_launcher");
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let path_str = dir.to_string_lossy().to_string();
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    );
    let argv = vec!["printf shell-launch-ok".to_string()];
    let res = exec
        .execute_bash("sh -c", Some(&path_str), "p1", Some(&argv), None)
        .await
        .unwrap();
    assert_eq!(res.exit_code, 0);
    assert_eq!(res.stdout.trim(), "shell-launch-ok");
    let _ = std::fs::remove_dir(&dir);
}

#[tokio::test]
async fn execute_bash_tokens_no_longer_trigger_path_gate() {
    let dir = std::env::temp_dir().join("tomcat_exec_bash_url");
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let path_str = dir.to_string_lossy().to_string();
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(DenyAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    );
    let res = exec
        .execute_bash(
            "printf '%s\\n' http://127.0.0.1:4173/ node:fs/promises @playwright/test",
            Some(&path_str),
            "p1",
            None,
            None,
        )
        .await
        .expect("bash token 不应再触发路径授权");
    assert_eq!(res.exit_code, 0);
    assert!(res.stdout.contains("http://127.0.0.1:4173/"));
    assert!(res.stdout.contains("node:fs/promises"));
    assert!(res.stdout.contains("@playwright/test"));
    let _ = std::fs::remove_dir(&dir);
}

#[tokio::test]
async fn read_file_url_like_returns_non_permission_error() {
    let exec = DefaultPrimitiveExecutor::new(
        PrimitiveConfig::default(),
        Arc::new(DenyAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&PathBuf::from("/nonexistent_pi_workspace")),
    );
    let err = exec
        .read_file("http://127.0.0.1:4173/", "p1")
        .await
        .expect_err("URL-like 输入应由文件工具自然失败，而不是走路径授权");
    assert!(
        !matches!(err, AppError::Permission(_)),
        "不应再返回路径权限错误: {:?}",
        err
    );
    assert!(
        err.to_string().contains("No such file or directory"),
        "应返回文件系统自然失败文案: {}",
        err
    );
}

#[tokio::test]
async fn search_files_url_like_returns_non_permission_error() {
    let exec = DefaultPrimitiveExecutor::new(
        PrimitiveConfig::default(),
        Arc::new(DenyAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&PathBuf::from("/nonexistent_pi_workspace")),
    );
    let err = exec
        .search_files(
            SearchFilesArgs {
                pattern: "needle".to_string(),
                target: SearchFilesTarget::Content,
                path: Some("http://127.0.0.1:4173/".to_string()),
                glob: None,
                file_type: None,
                output_mode: SearchFilesOutputMode::FilesWithMatches,
                context: None,
                head_limit: Some(Some(10)),
                offset: 0,
                case_insensitive: false,
                include_hidden: false,
            },
            "p1",
        )
        .await
        .expect_err("search_files 的 URL-like path 不应触发路径授权");
    assert!(
        !matches!(err, AppError::Permission(_)),
        "不应再返回路径权限错误: {:?}",
        err
    );
    assert!(
        err.to_string().contains("No such file or directory"),
        "应返回文件系统自然失败文案: {}",
        err
    );
}

/// T2-P0-016 PR-E.2 / bash.md §10 T1：墙钟超时 → kill 子进程 + 标记 timed_out。
///
/// 用 `with_bash_timeout_ms(50)` 把超时压到 50 ms，命令 `sleep 5` 触发 Elapsed 分支；
/// 期望 `timed_out=true`、`exit_code=-1`、stderr 含 "timed out" 提示。
#[tokio::test]
async fn bash_wallclock_timeout_kills_process() {
    let dir = std::env::temp_dir().join("tomcat_exec_bash_timeout");
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let path_str = dir.to_string_lossy().to_string();
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    )
    .with_bash_timeout_ms(50);

    let started = std::time::Instant::now();
    let res = exec
        .execute_bash("sleep 5", Some(&path_str), "p1", None, None)
        .await
        .expect("bash impl 应返回 Ok（即便超时）");
    let elapsed = started.elapsed();

    assert!(res.timed_out, "墙钟超时应置 timed_out=true");
    assert_eq!(res.exit_code, -1, "超时退出码约定 -1");
    assert!(
        res.stderr.contains("timed out"),
        "stderr 应携带 timed out 提示，实际: {:?}",
        res.stderr
    );
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "墙钟超时 50ms + kill 后整体应远小于 sleep 时长，实际 elapsed={:?}",
        elapsed
    );
    let _ = std::fs::remove_dir(&dir);
}

/// 前台 shell 先退出、后台 `cmd &` 仍攥着 stdout/stderr 时，同步 bash 不应永久卡死。
/// 小 timeout 场景下，主 shell 退出后的善后 grace 还应被剩余预算 clamp，不能把 80ms 调用
/// 反向拖成多秒；同时尽量保留前台已输出的内容。
#[tokio::test]
async fn bash_backgrounded_pipe_holder_does_not_hang() {
    let dir = std::env::temp_dir().join("tomcat_exec_bash_bg_pipe_holder");
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let path_str = dir.to_string_lossy().to_string();
    let audit_entries: Arc<Mutex<Vec<PrimitiveAuditEntry>>> = Arc::new(Mutex::new(Vec::new()));
    let audit = Arc::new(DenyAuditRecorder(audit_entries.clone()));
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        audit,
        make_gate(&dir),
    )
    .with_bash_timeout_ms(80);

    let started = std::time::Instant::now();
    let res = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        exec.execute_bash("sleep 30 & echo done", Some(&path_str), "p1", None, None),
    )
    .await
    .expect("后台残留路径不应把测试挂死")
    .expect("bash 应返回 Ok");
    let elapsed = started.elapsed();

    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "应在测试级 timeout 前返回，实际 elapsed={:?}",
        elapsed
    );
    assert!(
        !res.timed_out,
        "这里不是前台 wait 超时，而是 drain 检测到后台残留"
    );
    assert_eq!(res.exit_code, 0, "主 shell 已正常退出，应保留 exit_code=0");
    assert!(
        res.stdout.contains("done"),
        "前台输出应被尽量保留，实际 stdout={:?}",
        res.stdout
    );
    assert!(
        res.stderr.contains("run_in_background=true"),
        "stderr 应提示长任务改走后台机制，实际 stderr={:?}",
        res.stderr
    );

    let entries = audit_entries.lock().unwrap();
    let bash_entry = entries
        .iter()
        .rev()
        .find(|entry| entry.operation == AuditPrimitiveOp::Bash)
        .expect("应写 bash 审计");
    let detail = bash_entry.detail.as_deref().unwrap_or("");
    assert!(
        detail.contains("lingering_children=true"),
        "audit detail 应标记 lingering_children=true，实际 detail={}",
        detail
    );

    let _ = std::fs::remove_dir(&dir);
}

/// 大 timeout 回归：默认/120s 级别预算下，主 shell 退出后的 lingering 检测也不应把同步
/// bash 拖到完整 timeout_ms；应在 post-exit grace 内快速判残留并返回 warning。
#[tokio::test]
async fn bash_backgrounded_pipe_holder_returns_fast_under_large_timeout() {
    let dir = std::env::temp_dir().join("tomcat_exec_bash_bg_pipe_holder_large_timeout");
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let path_str = dir.to_string_lossy().to_string();
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    )
    .with_bash_timeout_ms(120_000);

    let started = std::time::Instant::now();
    let res = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        exec.execute_bash("sleep 30 & echo done", Some(&path_str), "p1", None, None),
    )
    .await
    .expect("大 timeout 下也不应把测试挂死")
    .expect("bash 应返回 Ok");
    let elapsed = started.elapsed();

    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "大 timeout 场景也应在数秒内返回，而不是吃满 120s，实际 elapsed={:?}",
        elapsed
    );
    assert!(
        !res.timed_out,
        "这里不是前台 wait 超时，而是 post-exit grace 检测到后台残留"
    );
    assert_eq!(res.exit_code, 0, "主 shell 已正常退出，应保留 exit_code=0");
    assert!(
        res.stdout.contains("done"),
        "前台输出应被尽量保留，实际 stdout={:?}",
        res.stdout
    );
    assert!(
        res.stderr.contains("run_in_background=true"),
        "stderr 应提示长任务改走后台机制，实际 stderr={:?}",
        res.stderr
    );

    let _ = std::fs::remove_dir(&dir);
}

/// T2-P0-016 PR-E.3 / bash.md §10 T1：超长 stdout 走 EndTruncatingAccumulator 头尾保留。
#[tokio::test]
async fn bash_output_truncation_keeps_head_tail() {
    let dir = std::env::temp_dir().join("tomcat_exec_bash_truncate");
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let path_str = dir.to_string_lossy().to_string();
    // max_output_chars 压到 64：fixture 命令打印 ~2000 字符肯定触发截断。
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    )
    .with_bash_max_output_chars(64);

    let res = exec
        .execute_bash(
            // 用 printf 输出可控长度，避开 yes/seq 平台差异。
            r#"printf 'A%.0s' $(seq 1 2000)"#,
            Some(&path_str),
            "p1",
            None,
            None,
        )
        .await
        .expect("bash 应返回 Ok");

    assert_eq!(res.exit_code, 0);
    assert!(res.truncated, "stdout 超 64 应置 truncated=true");
    assert!(
        res.stdout.contains("[truncated"),
        "截断后文本应含 [truncated 标记，实际: {:?}",
        res.stdout
    );
    assert!(
        res.persisted_output_path.is_none(),
        "未注入 bash_persist_dir 时，应不落盘",
    );
    // stdout 字符数 应 ≤ max_output_chars + truncation hint 余量
    assert!(
        res.stdout.chars().count() < 1500,
        "截断后字符数应远小于原始 2000，实际 {}",
        res.stdout.chars().count()
    );
    let _ = std::fs::remove_dir(&dir);
}

/// T2-P0-016 PR-E.3 / bash.md §10 T1：截断 + 落盘——`persisted_output_path` 指向完整原文。
#[tokio::test]
async fn bash_persists_full_output_when_truncated() {
    let dir = std::env::temp_dir().join("tomcat_exec_bash_persist");
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let path_str = dir.to_string_lossy().to_string();
    let persist_dir = dir.join("tool-results");
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    )
    .with_bash_max_output_chars(64)
    .with_bash_persist_dir(persist_dir.clone());

    let res = exec
        .execute_bash(
            r#"printf 'A%.0s' $(seq 1 2000)"#,
            Some(&path_str),
            "p1",
            None,
            None,
        )
        .await
        .expect("bash 应返回 Ok");

    assert!(res.truncated, "应置 truncated=true");
    let p = res
        .persisted_output_path
        .as_ref()
        .expect("注入 persist_dir 后应回填路径");
    let on_disk = std::fs::read_to_string(p).expect("应能读盘");
    assert_eq!(on_disk.chars().count(), 2000, "落盘字符数应等于原始 stdout");
    assert!(p.contains("bash-stdout-"));
    let _ = std::fs::remove_dir_all(&persist_dir);
    let _ = std::fs::remove_dir(&dir);
}

// ─── T2-P0-016 PR-L（bash T3）AST allowlist 集成测 ──────────────────────────

/// bash.md §10 T3：denylist 命中应在到达 `gate_check_bash` 之前以 `AstDeny` 拒绝，
/// 即便对应命令本不在 builtin forbidden 集合中。复合命令 `git pull && rm -rf X`
/// 中第二段 `rm` 命中，整条命令早退、磁盘上的 fixture 文件**不**被删除。
#[tokio::test]
async fn bash_ast_allowlist_denies_compound_command_short_circuit() {
    use crate::core::permission::BashAstChecker;

    let dir = std::env::temp_dir().join("tomcat_exec_bash_ast_deny");
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let path_str = dir.to_string_lossy().to_string();
    // 留个 fixture 文件验证 rm 真没执行。
    let probe = dir.join("must_survive.txt");
    std::fs::write(&probe, b"alive").unwrap();

    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    )
    .with_bash_ast(BashAstChecker::new(
        true,
        vec!["git".to_string()], // git 命中 allow 仍跳 approval；rm 命中 deny → 整条拒
        vec!["rm".to_string()],
    ));

    let cmd = format!("git --version && rm -rf {}", probe.display());
    let err = exec
        .execute_bash(&cmd, Some(&path_str), "p1", None, None)
        .await
        .expect_err("AST deny 应当返回 Err，不进入 gate / spawn");
    let msg = err.to_string();
    assert!(msg.contains("AstDeny"), "错误文案应含 AstDeny：{}", msg);
    assert!(
        msg.contains("rm"),
        "错误文案应指出 deny token 是 rm：{}",
        msg
    );
    assert!(probe.exists(), "AST 早退后 rm 不应执行；文件应仍在");

    let _ = std::fs::remove_file(&probe);
    let _ = std::fs::remove_dir(&dir);
}

/// bash.md §10 T3：空 allow/deny 列表 + enabled=true → 行为与今日（无 AST）字节级等价。
/// 这是 [bash-pr-l-scope.md §4 兼容性] 的硬性回归。
#[tokio::test]
async fn bash_ast_default_empty_lists_keeps_legacy_behavior() {
    use crate::core::permission::BashAstChecker;

    let dir = std::env::temp_dir().join("tomcat_exec_bash_ast_default");
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let path_str = dir.to_string_lossy().to_string();
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    )
    .with_bash_ast(BashAstChecker::new(true, vec![], vec![]));

    let res = exec
        .execute_bash("echo ast-skeleton-ok", Some(&path_str), "p1", None, None)
        .await
        .expect("空 list 时 AST 不应改变行为");
    assert_eq!(res.exit_code, 0);
    assert!(res.stdout.contains("ast-skeleton-ok"));
    let _ = std::fs::remove_dir(&dir);
}

/// bash.md §10 T3：MVP 不支持 heredoc → AstUnsupported 早退；
/// 不进入 gate / spawn，与 deny 路径同形态（仅错误前缀不同）。
#[tokio::test]
async fn bash_ast_heredoc_returns_unsupported_error() {
    use crate::core::permission::BashAstChecker;

    let dir = std::env::temp_dir().join("tomcat_exec_bash_ast_heredoc");
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let path_str = dir.to_string_lossy().to_string();
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    )
    .with_bash_ast(BashAstChecker::new(true, vec![], vec![]));

    let err = exec
        .execute_bash("cat <<EOF\nhi\nEOF\n", Some(&path_str), "p1", None, None)
        .await
        .expect_err("heredoc 应当 AstUnsupported 早退");
    assert!(err.to_string().contains("AstUnsupported"));
    let _ = std::fs::remove_dir(&dir);
}

#[tokio::test]
async fn execute_bash_forbidden() {
    // builtin bash_forbidden 命中 (`tomcat config set llm.api_key`) → Deny；
    // 走 gate 主路径，不再依赖 PrimitiveConfig.bash_forbidden 字段。
    let dir = std::env::temp_dir().join("tomcat_exec_forbid");
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let path_str = dir.to_string_lossy().to_string();
    let exec = DefaultPrimitiveExecutor::new(
        PrimitiveConfig::default(),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    );
    let r = exec
        .execute_bash(
            "tomcat config set llm.api_key xxx",
            Some(&path_str),
            "p1",
            None,
            None,
        )
        .await;
    assert!(r.is_err());
    assert!(matches!(r.unwrap_err(), AppError::Permission(_)));
    let _ = std::fs::remove_dir(&dir);
}

#[tokio::test]
async fn require_user_confirmation_read_returns_true() {
    let exec = DefaultPrimitiveExecutor::new(
        PrimitiveConfig::default(),
        Arc::new(DenyAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&PathBuf::from("/nonexistent_pi_workspace")),
    );
    // Read 操作在 require_user_confirmation 中直接返回 true。
    let ok = exec
        .require_user_confirmation(PrimitiveOperation::Read, "preview", "p1")
        .await
        .unwrap();
    assert!(ok);
}

#[tokio::test]
async fn require_user_confirmation_deny_returns_false() {
    let cfg = PrimitiveConfig {
        auto_confirm: false,
        ..PrimitiveConfig::default()
    };
    let exec = DefaultPrimitiveExecutor::new(
        cfg,
        Arc::new(DenyAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&PathBuf::from("/nonexistent_pi_workspace")),
    );
    // 写类操作 + DenyAllConfirmation → 返回 false。
    let ok = exec
        .require_user_confirmation(PrimitiveOperation::Write, "preview", "p1")
        .await
        .unwrap();
    assert!(!ok);
}

#[tokio::test]
async fn read_file_on_directory_returns_err() {
    let dir = std::env::temp_dir().join("tomcat_exec_read_dir");
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let path_str = dir.to_string_lossy().to_string();
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    );
    let r = exec.read_file(&path_str, "p1").await;
    assert!(r.is_err());
    assert!(r.unwrap_err().to_string().contains("目录"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn write_file_auto_confirm_skips_confirmation() {
    let dir = std::env::temp_dir().join("tomcat_exec_autoconfirm");
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let f = dir.join("ac.txt");
    std::fs::write(&f, "old").unwrap();
    let path_str = f.to_string_lossy().to_string();
    let mut c = temp_primitive_config(&dir);
    c.auto_confirm = true;
    // gate 也开 auto_confirm，layer-2 NeedConfirm 直接放行；这里目标在 dir 内，
    // 本身就是 Allow，DenyAll 不会被调用。
    let exec = DefaultPrimitiveExecutor::new(
        c,
        Arc::new(DenyAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate_with(&dir, vec![], true),
    );
    let res = exec.write_file(&path_str, "new", true, "p1").await.unwrap();
    assert!(res.written);
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "new");
    let _ = std::fs::remove_file(&f);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn write_file_overwrite_creates_backup() {
    let dir = std::env::temp_dir().join("tomcat_exec_backup");
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let f = dir.join("overwrite.txt");
    std::fs::write(&f, "original").unwrap();
    let path_str = f.to_string_lossy().to_string();
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    );
    let res = exec
        .write_file(&path_str, "overwritten", true, "p1")
        .await
        .unwrap();
    assert!(res.written);
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "overwritten");
    let backup = dir.join("overwrite.bak");
    assert!(backup.exists());
    assert_eq!(std::fs::read_to_string(&backup).unwrap(), "original");
    let _ = std::fs::remove_file(&f);
    let _ = std::fs::remove_file(&backup);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn write_file_overwrite_backup_failure_preserves_original() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dir = dir.path().canonicalize().unwrap();
    let f = dir.join("overwrite.txt");
    std::fs::write(&f, "original").unwrap();
    let backup = dir.join("overwrite.bak");
    std::fs::create_dir(&backup).unwrap();
    let path_str = f.to_string_lossy().to_string();
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    );

    let err = exec
        .write_file(&path_str, "overwritten", true, "p1")
        .await
        .expect_err("backup copy failure should abort overwrite");

    assert!(matches!(err, AppError::Io(_)));
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "original");
    assert!(backup.is_dir());
}

#[cfg(unix)]
#[tokio::test]
async fn write_file_overwrite_rollback_failure_surfaces_error() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().expect("tempdir");
    let dir = dir.path().canonicalize().unwrap();
    let f = dir.join("overwrite.txt");
    let original = "original";
    std::fs::write(&f, original.as_bytes()).unwrap();
    let backup = dir.join("overwrite.bak");
    let file_mode = std::fs::metadata(&f).unwrap().permissions().mode();
    std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o444)).unwrap();
    let cancel = CancellationToken::new();
    let err = simulate_failed_commit_with_backup_for_test(&f, &cancel, true, "write_file")
        .expect_err("rollback failure should surface as an error");
    std::fs::set_permissions(&f, std::fs::Permissions::from_mode(file_mode)).unwrap();

    let msg = err.to_string();
    assert!(
        matches!(err, AppError::Primitive(_)),
        "rollback failure should be reported as a primitive error: {msg}"
    );
    assert!(
        msg.contains("rollback"),
        "error should mention rollback failure: {msg}"
    );
    assert!(
        msg.contains("write_error=") && msg.contains("rollback_error="),
        "error should preserve both write and rollback causes: {msg}"
    );
    assert_eq!(std::fs::read_to_string(&f).unwrap(), original);
    assert!(
        backup.exists(),
        "failed rollback should keep .bak for postmortem"
    );
}

#[tokio::test]
async fn workspace_roots_allow_external_path() {
    let ws_dir = std::env::temp_dir().join("tomcat_exec_extra_ws");
    std::fs::create_dir_all(&ws_dir).unwrap();
    let ws_dir = ws_dir.canonicalize().unwrap();

    let ext_dir = std::env::temp_dir().join("tomcat_exec_extra_ext");
    std::fs::create_dir_all(&ext_dir).unwrap();
    let ext_dir = ext_dir.canonicalize().unwrap();
    let ext_file = ext_dir.join("ext.txt");
    std::fs::write(&ext_file, "external").unwrap();

    let exec = DefaultPrimitiveExecutor::new(
        PrimitiveConfig::default(),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate_with(&ws_dir, vec![ext_dir.clone()], false),
    );

    let content = exec
        .read_file(&ext_file.to_string_lossy(), "p1")
        .await
        .unwrap();
    assert_eq!(content, "external");

    let _ = std::fs::remove_dir_all(&ws_dir);
    let _ = std::fs::remove_dir_all(&ext_dir);
}

#[tokio::test]
async fn workspace_roots_still_rejects_unlisted_path() {
    let ws_dir = std::env::temp_dir().join("tomcat_exec_extra_reject");
    std::fs::create_dir_all(&ws_dir).unwrap();
    let ws_dir = ws_dir.canonicalize().unwrap();

    let exec = DefaultPrimitiveExecutor::new(
        PrimitiveConfig::default(),
        Arc::new(DenyAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate_with(&ws_dir, vec![], false),
    );

    let r = exec.read_file("/tmp/some_other_path/file.txt", "p1").await;
    assert!(r.is_err());
    assert!(matches!(r.unwrap_err(), AppError::Permission(_)));

    let _ = std::fs::remove_dir_all(&ws_dir);
}

// ─── T2-P0-017 PR-D（T1） edit 工具测试矩阵 ──────────────────────────────────

/// 用 `EDIT_REPLACE_ALL_MARKER` 构造一条 LLM 主路径段（无行号、`Replace`）。
fn edit_seg(old: &str, new: &str, replace_all: bool) -> EditOperation {
    let encoded = if replace_all {
        format!(
            "{}{}",
            crate::core::tools::primitive::EDIT_REPLACE_ALL_MARKER,
            old
        )
    } else {
        old.to_string()
    };
    EditOperation {
        operation_type: EditOperationType::Replace,
        start_line: None,
        end_line: None,
        old_content: Some(encoded),
        new_content: new.to_string(),
    }
}

fn temp_edit_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

#[tokio::test]
async fn edit_replace_all_replaces_every_match() {
    let dir = temp_edit_dir("tomcat_edit_replace_all");
    let f = dir.join("a.txt");
    // 多命中 + 多字节 + 尾换行：原文必须保留行尾 `\n`。
    let body = "TODO 文档\nbody\nTODO 文档\n";
    std::fs::write(&f, body).unwrap();
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    );
    let edits = vec![edit_seg("TODO 文档", "DONE 文档", true)];
    let res = exec
        .edit_file(&f.to_string_lossy(), edits, "p1")
        .await
        .unwrap();
    assert!(res.applied);
    assert_eq!(
        std::fs::read_to_string(&f).unwrap(),
        "DONE 文档\nbody\nDONE 文档\n",
        "replace_all 必须命中每一处且保留尾换行"
    );
    assert!(!dir.join("a.bak").exists(), "成功路径不应残留 .bak");
}

#[tokio::test]
async fn edit_multiple_edits_apply_against_original() {
    // 关键 fixture：第二段 old `B->X` 仅在原始文件中存在；如果实现是链式，
    // 第一段 `A->B` 改完后会出现两个 `B`（原 `B` + 新 `B`），第二段会变成
    // Ambiguous（count=2）。本测要求多段都对 `original` 算 → 应成功。
    let dir = temp_edit_dir("tomcat_edit_multi_original");
    let f = dir.join("b.txt");
    std::fs::write(&f, "A\nB\nC\n").unwrap();
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    );
    let edits = vec![
        edit_seg("A", "B", false), // 把 A 改成 B
        edit_seg("B", "X", false), // 同时把原文里的 B 改成 X
    ];
    let res = exec
        .edit_file(&f.to_string_lossy(), edits, "p1")
        .await
        .unwrap();
    assert!(res.applied);
    assert_eq!(
        std::fs::read_to_string(&f).unwrap(),
        "B\nX\nC\n",
        "多段 edit 必须都对 original 算 span，而不是链式"
    );
}

#[tokio::test]
async fn edit_overlap_rejected() {
    let dir = temp_edit_dir("tomcat_edit_overlap");
    let f = dir.join("c.txt");
    let original = "abcdef\n";
    std::fs::write(&f, original).unwrap();
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    );
    // 段1: "abcd" → "X"；段2: "cde" → "Y"。两段相交于 "cd"，必须 Overlap。
    let edits = vec![edit_seg("abcd", "X", false), edit_seg("cde", "Y", false)];
    let r = exec.edit_file(&f.to_string_lossy(), edits, "p1").await;
    assert!(r.is_err(), "重叠段必须拒绝");
    let msg = r.unwrap_err().to_string();
    assert!(msg.contains("Overlap"), "错误文案应含 Overlap：{}", msg);
    assert!(msg.contains("edits[0]"), "错误文案应指出左侧段号：{}", msg);
    assert!(msg.contains("edits[1]"), "错误文案应指出右侧段号：{}", msg);
    assert!(msg.contains("第 1 行"), "错误文案应给出行号：{}", msg);
    assert_eq!(
        std::fs::read_to_string(&f).unwrap(),
        original,
        "校验失败磁盘必须字节级未变"
    );
    assert!(!dir.join("c.bak").exists(), "校验失败必须无 .bak 残留");
}

#[tokio::test]
async fn edit_overlap_adjacent_not_rejected() {
    // 边界相邻（s2 == e1）不算重叠，必须允许。
    let dir = temp_edit_dir("tomcat_edit_adjacent");
    let f = dir.join("d.txt");
    std::fs::write(&f, "abcdef").unwrap();
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    );
    let edits = vec![edit_seg("abc", "1", false), edit_seg("def", "2", false)];
    let res = exec
        .edit_file(&f.to_string_lossy(), edits, "p1")
        .await
        .unwrap();
    assert!(res.applied);
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "12");
}

#[tokio::test]
async fn edit_overlap_nested_reports_subset_hint() {
    let dir = temp_edit_dir("tomcat_edit_overlap_nested");
    let f = dir.join("nested.txt");
    std::fs::write(&f, "abcdef\n").unwrap();
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    );
    let err = exec
        .edit_file(
            &f.to_string_lossy(),
            vec![edit_seg("abcdef", "X", false), edit_seg("abc", "Y", false)],
            "p1",
        )
        .await
        .expect_err("嵌套 edit 必须拒绝");
    let msg = err.to_string();
    assert!(msg.contains("Overlap"), "错误文案应含 Overlap：{}", msg);
    assert!(msg.contains("edits[0]"), "错误文案应指出外层段号：{}", msg);
    assert!(msg.contains("edits[1]"), "错误文案应指出内层段号：{}", msg);
    assert!(msg.contains("嵌套包含"), "错误文案应指出嵌套特例：{}", msg);
}

#[tokio::test]
async fn edit_validation_failure_restores_or_noop() {
    // NotFound：磁盘必须未变 + 无 .bak。
    let dir = temp_edit_dir("tomcat_edit_notfound");
    let f = dir.join("e.txt");
    let original = "hello\n";
    std::fs::write(&f, original).unwrap();
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    );
    let r = exec
        .edit_file(
            &f.to_string_lossy(),
            vec![edit_seg("missing", "x", false)],
            "p1",
        )
        .await;
    assert!(r.is_err());
    let msg = r.unwrap_err().to_string();
    assert!(msg.contains("NotFound"), "错误文案应含 NotFound：{}", msg);
    assert!(
        msg.contains("Stale"),
        "NotFound 应提示 stale/重读引导：{}",
        msg
    );
    assert!(
        msg.contains("连续原文"),
        "NotFound 应提示连续片段约束：{}",
        msg
    );
    assert_eq!(std::fs::read_to_string(&f).unwrap(), original);
    assert!(!dir.join("e.bak").exists());

    // Ambiguous：同样磁盘未变 + 无 .bak。
    let f2 = dir.join("f.txt");
    let original2 = "x\nx\n";
    std::fs::write(&f2, original2).unwrap();
    let r2 = exec
        .edit_file(&f2.to_string_lossy(), vec![edit_seg("x", "y", false)], "p1")
        .await;
    assert!(r2.is_err());
    let msg2 = r2.unwrap_err().to_string();
    assert!(
        msg2.contains("Ambiguous"),
        "错误文案应含 Ambiguous：{}",
        msg2
    );
    assert_eq!(std::fs::read_to_string(&f2).unwrap(), original2);
    assert!(!dir.join("f.bak").exists());
}

#[tokio::test]
async fn edit_notfound_with_cat_n_prefix_explains_remediation() {
    let dir = temp_edit_dir("tomcat_edit_cat_n_prefix");
    let f = dir.join("catn.txt");
    let original = "function foo() {\n  return 1;\n}\n";
    std::fs::write(&f, original).unwrap();
    let audit_entries: Arc<Mutex<Vec<PrimitiveAuditEntry>>> = Arc::new(Mutex::new(Vec::new()));
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        Arc::new(DenyAuditRecorder(audit_entries.clone())),
        make_gate(&dir),
    );
    let prefixed_old = "     1\tfunction foo() {\n     2\t  return 1;\n     3\t}\n";
    let err = exec
        .edit_file(
            &f.to_string_lossy(),
            vec![edit_seg(
                prefixed_old,
                "function foo() {\n  return 2;\n}\n",
                false,
            )],
            "p1",
        )
        .await
        .expect_err("带 cat -n 前缀的 old_content 应触发专用 NotFound 诊断");
    let msg = err.to_string();
    assert!(
        msg.contains("NotFound (line_prefix_suspected)"),
        "错误文案应含子码：{}",
        msg
    );
    assert!(
        msg.contains("cat -n 行号前缀"),
        "错误文案应指出 cat -n 来源：{}",
        msg
    );
    assert!(
        msg.contains("第 1 行"),
        "错误文案应给出命中行号 hint：{}",
        msg
    );
    assert!(msg.contains("\\t"), "escape_debug 摘要应暴露 Tab：{}", msg);
    assert_eq!(std::fs::read_to_string(&f).unwrap(), original);
    assert!(!dir.join("catn.bak").exists(), "校验失败不应生成 .bak");
    let audit = audit_entries.lock().unwrap();
    let detail = audit
        .last()
        .and_then(|entry| entry.detail.as_deref())
        .expect("失败 edit 应记录 audit detail");
    assert!(
        detail.contains("NotFound[line_prefix_suspected] edits[0]"),
        "audit detail 应带子分类标签：{}",
        detail
    );
}

#[tokio::test]
async fn edit_notfound_with_hashline_prefix_explains_remediation() {
    let dir = temp_edit_dir("tomcat_edit_hashline_prefix");
    let f = dir.join("hashline.txt");
    let original = "alpha\nbeta\ngamma\n";
    std::fs::write(&f, original).unwrap();
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    );
    let h1 = crate::core::tools::primitive::compute_line_hash("alpha", 1);
    let h2 = crate::core::tools::primitive::compute_line_hash("beta", 2);
    let h3 = crate::core::tools::primitive::compute_line_hash("gamma", 3);
    let prefixed_old = format!(
        "{:>6}#{}:alpha\n{:>6}#{}:beta\n{:>6}#{}:gamma\n",
        1, h1, 2, h2, 3, h3
    );
    let err = exec
        .edit_file(
            &f.to_string_lossy(),
            vec![edit_seg(&prefixed_old, "ALPHA\nBETA\nGAMMA\n", false)],
            "p1",
        )
        .await
        .expect_err("带 hashline 前缀的 old_content 应触发专用 NotFound 诊断");
    let msg = err.to_string();
    assert!(
        msg.contains("NotFound (line_prefix_suspected)"),
        "错误文案应含子码：{}",
        msg
    );
    assert!(
        msg.contains("hashline 前缀"),
        "错误文案应指出 hashline 来源：{}",
        msg
    );
    assert!(
        msg.contains("第 1 行"),
        "错误文案应给出命中行号 hint：{}",
        msg
    );
    assert_eq!(std::fs::read_to_string(&f).unwrap(), original);
    assert!(!dir.join("hashline.bak").exists(), "校验失败不应生成 .bak");
}

#[tokio::test]
async fn edit_notfound_with_partial_prefix_does_not_misfire() {
    let dir = temp_edit_dir("tomcat_edit_partial_prefix");
    let f = dir.join("partial.txt");
    let original = "alpha\nbeta\ngamma\n";
    std::fs::write(&f, original).unwrap();
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    );
    let mixed_old = "     1\talpha\nbeta\n";
    let err = exec
        .edit_file(
            &f.to_string_lossy(),
            vec![edit_seg(mixed_old, "X\n", false)],
            "p1",
        )
        .await
        .expect_err("混合前缀不应误判为整段 line_prefix_suspected");
    let msg = err.to_string();
    assert!(msg.contains("NotFound:"), "仍应返回普通 NotFound：{}", msg);
    assert!(
        !msg.contains("line_prefix_suspected"),
        "混合前缀不应误触发专用子码：{}",
        msg
    );
    assert_eq!(std::fs::read_to_string(&f).unwrap(), original);
}

#[tokio::test]
async fn edit_notfound_plain_missing_stays_plain_notfound() {
    let dir = temp_edit_dir("tomcat_edit_plain_notfound");
    let f = dir.join("plain.txt");
    let original = "alpha\nbeta\ngamma\n";
    std::fs::write(&f, original).unwrap();
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    );
    let err = exec
        .edit_file(
            &f.to_string_lossy(),
            vec![edit_seg("missing", "replacement", false)],
            "p1",
        )
        .await
        .expect_err("普通缺失文本应保持普通 NotFound");
    let msg = err.to_string();
    assert!(msg.contains("NotFound:"), "错误文案应含 NotFound：{}", msg);
    assert!(
        !msg.contains("line_prefix_suspected"),
        "普通缺失文本不应落入专用子码：{}",
        msg
    );
    assert!(
        msg.contains("escape_debug"),
        "普通 NotFound 应附 escape_debug 摘要：{}",
        msg
    );
    assert!(
        msg.contains("Stale"),
        "普通 NotFound 也应带 stale 引导：{}",
        msg
    );
    assert_eq!(std::fs::read_to_string(&f).unwrap(), original);
}

#[tokio::test]
async fn edit_preserves_trailing_newline() {
    // 旧实现 `lines().join("\n")` 会吃掉尾换行；本测锁定尾换行保留。
    let dir = temp_edit_dir("tomcat_edit_trailing_lf");
    let f = dir.join("g.txt");
    std::fs::write(&f, "line1\nline2\n").unwrap();
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    );
    let res = exec
        .edit_file(
            &f.to_string_lossy(),
            vec![edit_seg("line1", "X", false)],
            "p1",
        )
        .await
        .unwrap();
    assert!(res.applied);
    assert_eq!(
        std::fs::read_to_string(&f).unwrap(),
        "X\nline2\n",
        "尾换行必须保留"
    );
}

#[tokio::test]
async fn edit_curly_quote_matches_disk_straight_quote() {
    let dir = temp_edit_dir("tomcat_edit_curly");
    let f = dir.join("q.txt");
    // 磁盘：直引号；模型：弯引号
    std::fs::write(&f, "let s = \"hello\";\n").unwrap();
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    );
    let r = exec
        .edit_file(
            &f.to_string_lossy(),
            vec![edit_seg("\u{201C}hello\u{201D}", "\"world\"", false)],
            "p1",
        )
        .await
        .unwrap();
    assert!(r.applied);
    assert_eq!(
        std::fs::read_to_string(&f).unwrap(),
        "let s = \"world\";\n",
        "弯引号 old 应当命中直引号磁盘"
    );
}

#[tokio::test]
async fn edit_desanitize_matches_nbsp_and_zwsp() {
    // 磁盘有 NBSP + 零宽空格；模型用普通空格 + 删掉零宽。
    let dir = temp_edit_dir("tomcat_edit_desanitize");
    let f = dir.join("d.txt");
    let body = "foo\u{00A0}\u{200B}bar\n";
    std::fs::write(&f, body).unwrap();
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    );
    let r = exec
        .edit_file(
            &f.to_string_lossy(),
            vec![edit_seg("foo bar", "X Y", false)],
            "p1",
        )
        .await
        .unwrap();
    assert!(r.applied);
    let after = std::fs::read_to_string(&f).unwrap();
    assert_eq!(after, "X Y\n", "归一化匹配后磁盘应当被改成 X Y\\n");
}

#[tokio::test]
async fn edit_preserves_crlf_line_endings() {
    let dir = temp_edit_dir("tomcat_edit_crlf");
    let f = dir.join("crlf.txt");
    let body = b"alpha\r\nbeta\r\ngamma\r\n";
    std::fs::write(&f, body).unwrap();
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    );
    let r = exec
        .edit_file(
            &f.to_string_lossy(),
            // 模型用 LF；磁盘是 CRLF；管线应当保留 CRLF。
            vec![edit_seg("beta", "BETA", false)],
            "p1",
        )
        .await
        .unwrap();
    assert!(r.applied);
    let bytes = std::fs::read(&f).unwrap();
    assert_eq!(
        bytes, b"alpha\r\nBETA\r\ngamma\r\n",
        "CRLF 文件改后行尾必须仍是 CRLF"
    );
}

#[tokio::test]
async fn edit_preserves_bom() {
    let dir = temp_edit_dir("tomcat_edit_bom");
    let f = dir.join("bom.txt");
    let mut body = vec![0xEF, 0xBB, 0xBF];
    body.extend_from_slice(b"head\nline2\n");
    std::fs::write(&f, &body).unwrap();
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    );
    let r = exec
        .edit_file(
            &f.to_string_lossy(),
            vec![edit_seg("head", "HEAD", false)],
            "p1",
        )
        .await
        .unwrap();
    assert!(r.applied);
    let bytes = std::fs::read(&f).unwrap();
    assert_eq!(&bytes[..3], &[0xEF, 0xBB, 0xBF], "BOM 必须仍在文件头");
    assert_eq!(&bytes[3..], b"HEAD\nline2\n");
}

#[tokio::test]
async fn edit_secrets_hit_denied_reverts_to_no_op() {
    let dir = temp_edit_dir("tomcat_edit_secrets_deny");
    let f = dir.join("s.rs");
    std::fs::write(&f, "let x = 1;\n").unwrap();
    let cfg = PrimitiveConfig {
        auto_confirm: false,
        ..temp_primitive_config(&dir)
    };
    let exec = DefaultPrimitiveExecutor::new(
        cfg,
        // DenyAll：confirm 返回 false → SecretsRejected
        Arc::new(DenyAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    );
    let r = exec
        .edit_file(
            &f.to_string_lossy(),
            // 引入 OpenAI key 模式 → secrets::scan 命中
            vec![edit_seg(
                "let x = 1;",
                "let k = \"sk-ABCDEFGHIJKLMNOPQRSTUV\";",
                false,
            )],
            "p1",
        )
        .await;
    assert!(r.is_err(), "DenyAll confirmation 下应当被拒");
    let msg = r.unwrap_err().to_string();
    assert!(
        msg.contains("SecretsRejected"),
        "错误文案应含 SecretsRejected：{}",
        msg
    );
    assert_eq!(
        std::fs::read_to_string(&f).unwrap(),
        "let x = 1;\n",
        "拒绝时磁盘必须未变"
    );
    assert!(!dir.join("s.bak").exists(), "拒绝时不应有 .bak 残留");
}

#[tokio::test]
async fn edit_legacy_line_oriented_path_still_works() {
    // 兼容路径：`Replace` 带 start_line（dispatcher / extension 内部使用）走旧逻辑。
    let dir = temp_edit_dir("tomcat_edit_line_oriented");
    let f = dir.join("h.txt");
    std::fs::write(&f, "line1\nline2\nline3").unwrap();
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    );
    let edits = vec![EditOperation {
        operation_type: EditOperationType::Replace,
        start_line: Some(2),
        end_line: Some(2),
        old_content: Some("line2".to_string()),
        new_content: "replaced".to_string(),
    }];
    let res = exec
        .edit_file(&f.to_string_lossy(), edits, "p1")
        .await
        .unwrap();
    assert!(res.applied);
    assert_eq!(
        std::fs::read_to_string(&f).unwrap(),
        "line1\nreplaced\nline3"
    );
}

// ─── T2-P0-016 PR-G：write LF 规范化 + 字节数 / diff 回执 ──────────────────

#[tokio::test]
async fn write_normalizes_crlf_when_enabled() {
    let dir = std::env::temp_dir().join("tomcat_exec_lf_on");
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let f = dir.join("crlf.txt");
    let path_str = f.to_string_lossy().to_string();
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    );
    let res = exec
        .write_file(&path_str, "a\r\nb\r\nc\r\n", false, "p1")
        .await
        .unwrap();
    assert!(res.written);
    let on_disk = std::fs::read(&f).unwrap();
    assert_eq!(on_disk, b"a\nb\nc\n", "CRLF 应被折叠为 LF");
    assert_eq!(res.bytes_written, on_disk.len() as u64);
    assert!(res.diff_hint.is_none(), "新建文件不带 diff hint");
    let _ = std::fs::remove_file(&f);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn write_does_not_normalize_when_disabled() {
    let dir = std::env::temp_dir().join("tomcat_exec_lf_off");
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let f = dir.join("crlf_off.txt");
    let path_str = f.to_string_lossy().to_string();
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    )
    .with_write_normalize_crlf(false);
    let res = exec
        .write_file(&path_str, "a\r\nb\r\n", false, "p1")
        .await
        .unwrap();
    assert!(res.written);
    let on_disk = std::fs::read(&f).unwrap();
    assert_eq!(on_disk, b"a\r\nb\r\n", "normalize_crlf=false 时字节透传");
    assert_eq!(res.bytes_written, on_disk.len() as u64);
    let _ = std::fs::remove_file(&f);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn write_secrets_pass_when_no_hit() {
    let dir = temp_edit_dir("tomcat_write_secrets_pass");
    let f = dir.join("clean.rs");
    let path_str = f.to_string_lossy().to_string();
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    );
    let res = exec
        .write_file(&path_str, "fn main() { println!(\"hi\"); }\n", false, "p1")
        .await
        .unwrap();
    assert!(res.written, "无敏感命中应直接写盘");
    assert!(f.exists());
}

#[tokio::test]
async fn write_secrets_hit_proceeds_with_allow_all_confirmation() {
    let dir = temp_edit_dir("tomcat_write_secrets_allow");
    let f = dir.join("k.rs");
    let path_str = f.to_string_lossy().to_string();
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    );
    let res = exec
        .write_file(
            &path_str,
            "let k = \"sk-ABCDEFGHIJKLMNOPQRSTUV\";\n",
            false,
            "p1",
        )
        .await
        .unwrap();
    assert!(res.written, "AllowAll 下命中应放行写盘");
    let on_disk = std::fs::read_to_string(&f).unwrap();
    assert!(on_disk.contains("sk-ABCDEFGHIJKLMNOPQRSTUV"));
}

#[tokio::test]
async fn write_secrets_hit_denied_reverts_to_no_op() {
    let dir = temp_edit_dir("tomcat_write_secrets_deny");
    let f = dir.join("k.rs");
    let path_str = f.to_string_lossy().to_string();
    let cfg = PrimitiveConfig {
        auto_confirm: false,
        ..temp_primitive_config(&dir)
    };
    let exec = DefaultPrimitiveExecutor::new(
        cfg,
        Arc::new(DenyAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    );
    let r = exec
        .write_file(
            &path_str,
            "let k = \"sk-ABCDEFGHIJKLMNOPQRSTUV\";\n",
            false,
            "p1",
        )
        .await;
    assert!(r.is_err(), "DenyAll 下命中必须被拒");
    let msg = r.unwrap_err().to_string();
    assert!(
        msg.contains("SecretsRejected"),
        "错误文案应含 SecretsRejected：{}",
        msg
    );
    assert!(!f.exists(), "拒绝时新文件不应被创建（磁盘字节级未变）");
}

#[tokio::test]
async fn write_result_includes_byte_count_and_diff_hint() {
    let dir = std::env::temp_dir().join("tomcat_exec_diff");
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let f = dir.join("diff.txt");
    std::fs::write(&f, "line1\nline2\nline3\n").unwrap();
    let path_str = f.to_string_lossy().to_string();
    let exec = DefaultPrimitiveExecutor::new(
        temp_primitive_config(&dir),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&dir),
    );
    let new_content = "line1\nLINE2\nline3\n";
    let res = exec
        .write_file(&path_str, new_content, true, "p1")
        .await
        .unwrap();
    assert!(res.written);
    assert_eq!(res.bytes_written, new_content.len() as u64);
    let hint = res.diff_hint.expect("覆盖写应返回 diff hint");
    assert!(
        hint.contains("line2") && hint.contains("LINE2"),
        "diff hint 应同时含旧行与新行：{}",
        hint
    );
    let _ = std::fs::remove_file(&f);
    let _ = std::fs::remove_file(dir.join("diff.bak"));
    let _ = std::fs::remove_dir_all(&dir);
}
