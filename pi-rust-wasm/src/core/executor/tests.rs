use super::*;
use crate::core::primitives::{
    EditOperation, EditOperationType, PrimitiveExecutor, PrimitiveOperation,
};
use crate::core::{AllowAllConfirmation, DenyAllConfirmation};
use crate::infra::error::AppError;
use crate::infra::{
    AuditRecorder, PrimitiveAuditEntry, PrimitiveConfig, ToolAuditEntry, TracingAuditRecorder,
};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

fn temp_whitelist_config(dir: &std::path::Path) -> PrimitiveConfig {
    let mut c = PrimitiveConfig::default();
    let canonical = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    c.path_whitelist = vec![canonical.to_string_lossy().into_owned()];
    c.require_approval_for_all_write = false;
    c.require_approval_for_all_bash = false;
    c.bash_whitelist = vec!["echo".to_string()];
    c
}

#[tokio::test]
async fn read_file_success() {
    let dir = std::env::temp_dir().join("pi_wasm_exec_read");
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let f = dir.join("f.txt");
    std::fs::write(&f, "hello").unwrap();
    let path_str = f.to_string_lossy().to_string();
    let config = temp_whitelist_config(&dir);
    let exec = DefaultPrimitiveExecutor::new(
        config,
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        dir.clone(),
    );
    let out = exec.read_file(&path_str, "p1").await.unwrap();
    assert_eq!(out, "hello");
    let _ = std::fs::remove_file(&f);
    let _ = std::fs::remove_dir(&dir);
}

#[tokio::test]
async fn read_file_path_not_in_whitelist() {
    let config = PrimitiveConfig::default();
    let exec = DefaultPrimitiveExecutor::new(
        config,
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        PathBuf::from("/nonexistent_pi_workspace"),
    );
    let r = exec.read_file("/tmp/any", "p1").await;
    assert!(r.is_err());
    assert!(matches!(r.unwrap_err(), AppError::Permission(_)));
}

#[tokio::test]
async fn list_dir_success() {
    let dir = std::env::temp_dir().join("pi_wasm_exec_list");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("f.txt"), "").unwrap();
    let dir = dir.canonicalize().unwrap();
    let config = temp_whitelist_config(&dir);
    let exec = DefaultPrimitiveExecutor::new(
        config,
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        dir.clone(),
    );
    let path_str = dir.to_string_lossy().to_string();
    let entries = exec.list_dir(&path_str, "p1").await.unwrap();
    assert!(!entries.is_empty());
    let _ = std::fs::remove_file(dir.join("f.txt"));
    let _ = std::fs::remove_dir(&dir);
}

#[tokio::test]
async fn write_file_success() {
    let dir = std::env::temp_dir().join("pi_wasm_exec_write");
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let f = dir.join("w.txt");
    let path_str = f.to_string_lossy().to_string();
    let config = temp_whitelist_config(&dir);
    let exec = DefaultPrimitiveExecutor::new(
        config,
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        dir.clone(),
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
async fn write_file_user_denied_returns_permission_and_audit() {
    let dir = std::env::temp_dir().join("pi_wasm_exec_deny");
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let f = dir.join("d.txt");
    std::fs::write(&f, "old").unwrap();
    let path_str = f.to_string_lossy().to_string();
    let mut c = temp_whitelist_config(&dir);
    c.require_approval_for_all_write = true;
    let audit_entries: Arc<Mutex<Vec<PrimitiveAuditEntry>>> = Arc::new(Mutex::new(Vec::new()));
    let audit = Arc::new(DenyAuditRecorder(audit_entries.clone()));
    let exec = DefaultPrimitiveExecutor::new(c, Arc::new(DenyAllConfirmation), audit, dir.clone());
    let r = exec.write_file(&path_str, "new", true, "p1").await;
    assert!(r.is_err());
    assert!(matches!(r.unwrap_err(), AppError::Permission(_)));
    let entries = audit_entries.lock().unwrap();
    assert!(!entries.is_empty());
    let last = entries.last().unwrap();
    assert!(!last.user_approved);
    assert!(!last.success);
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
    let dir = std::env::temp_dir().join("pi_wasm_exec_edit");
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let f = dir.join("e.txt");
    std::fs::write(&f, "line1\nline2\nline3").unwrap();
    let path_str = f.to_string_lossy().to_string();
    let mut c = temp_whitelist_config(&dir);
    c.require_approval_for_all_write = false;
    let exec = DefaultPrimitiveExecutor::new(
        c,
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        dir.clone(),
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
async fn execute_bash_success() {
    let dir = std::env::temp_dir().join("pi_wasm_exec_bash");
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let path_str = dir.to_string_lossy().to_string();
    let config = temp_whitelist_config(&dir);
    let exec = DefaultPrimitiveExecutor::new(
        config,
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        dir.clone(),
    );
    let res = exec
        .execute_bash("echo ok", Some(&path_str), "p1", None)
        .await
        .unwrap();
    assert_eq!(res.exit_code, 0);
    assert!(res.stdout.trim().contains("ok"));
    let _ = std::fs::remove_dir(&dir);
}

#[tokio::test]
async fn execute_bash_forbidden() {
    let dir = std::env::temp_dir().join("pi_wasm_exec_forbid");
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let path_str = dir.to_string_lossy().to_string();
    let mut c = temp_whitelist_config(&dir);
    c.bash_forbidden = vec!["rm".to_string()];
    let exec = DefaultPrimitiveExecutor::new(
        c,
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        dir.clone(),
    );
    let r = exec
        .execute_bash("rm -rf /", Some(&path_str), "p1", None)
        .await;
    assert!(r.is_err());
    assert!(matches!(r.unwrap_err(), AppError::Permission(_)));
    let _ = std::fs::remove_dir(&dir);
}

#[tokio::test]
async fn require_user_confirmation_deny_returns_false() {
    let config = PrimitiveConfig::default();
    let exec = DefaultPrimitiveExecutor::new(
        config,
        Arc::new(DenyAllConfirmation),
        Arc::new(TracingAuditRecorder),
        PathBuf::from("/nonexistent_pi_workspace"),
    );
    let ok = exec
        .require_user_confirmation(PrimitiveOperation::Write, "preview", "p1")
        .await
        .unwrap();
    assert!(!ok);
}

#[tokio::test]
async fn list_dir_path_in_blacklist_returns_err() {
    let dir = std::env::temp_dir().join("pi_wasm_exec_blacklist");
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let path_str = dir.to_string_lossy().to_string();
    let mut c = temp_whitelist_config(&dir);
    c.path_blacklist = vec![path_str.clone()];
    let exec = DefaultPrimitiveExecutor::new(
        c,
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        dir.clone(),
    );
    let r = exec.list_dir(&path_str, "p1").await;
    assert!(r.is_err());
    assert!(r.unwrap_err().to_string().contains("黑名单"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn read_file_on_directory_returns_err() {
    let dir = std::env::temp_dir().join("pi_wasm_exec_read_dir");
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let path_str = dir.to_string_lossy().to_string();
    let config = temp_whitelist_config(&dir);
    let exec = DefaultPrimitiveExecutor::new(
        config,
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        dir.clone(),
    );
    let r = exec.read_file(&path_str, "p1").await;
    assert!(r.is_err());
    assert!(r.unwrap_err().to_string().contains("目录"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn write_file_auto_confirm_skips_confirmation() {
    let dir = std::env::temp_dir().join("pi_wasm_exec_autoconfirm");
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let f = dir.join("ac.txt");
    std::fs::write(&f, "old").unwrap();
    let path_str = f.to_string_lossy().to_string();
    let mut c = temp_whitelist_config(&dir);
    c.auto_confirm = true;
    c.require_approval_for_all_write = true;
    let exec = DefaultPrimitiveExecutor::new(
        c,
        Arc::new(DenyAllConfirmation),
        Arc::new(TracingAuditRecorder),
        dir.clone(),
    );
    let res = exec.write_file(&path_str, "new", true, "p1").await.unwrap();
    assert!(res.written);
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "new");
    let _ = std::fs::remove_file(&f);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn write_file_overwrite_creates_backup() {
    let dir = std::env::temp_dir().join("pi_wasm_exec_backup");
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let f = dir.join("overwrite.txt");
    std::fs::write(&f, "original").unwrap();
    let path_str = f.to_string_lossy().to_string();
    let config = temp_whitelist_config(&dir);
    let exec = DefaultPrimitiveExecutor::new(
        config,
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        dir.clone(),
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
async fn extra_roots_allow_external_path() {
    let ws_dir = std::env::temp_dir().join("pi_wasm_exec_extra_ws");
    std::fs::create_dir_all(&ws_dir).unwrap();
    let ws_dir = ws_dir.canonicalize().unwrap();

    let ext_dir = std::env::temp_dir().join("pi_wasm_exec_extra_ext");
    std::fs::create_dir_all(&ext_dir).unwrap();
    let ext_dir = ext_dir.canonicalize().unwrap();
    let ext_file = ext_dir.join("ext.txt");
    std::fs::write(&ext_file, "external").unwrap();

    let config = PrimitiveConfig::default();
    let exec = DefaultPrimitiveExecutor::new(
        config,
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        ws_dir.clone(),
    )
    .with_extra_roots(vec![ext_dir.clone()]);

    let content = exec
        .read_file(&ext_file.to_string_lossy(), "p1")
        .await
        .unwrap();
    assert_eq!(content, "external");

    let _ = std::fs::remove_dir_all(&ws_dir);
    let _ = std::fs::remove_dir_all(&ext_dir);
}

#[tokio::test]
async fn extra_roots_still_rejects_unlisted_path() {
    let ws_dir = std::env::temp_dir().join("pi_wasm_exec_extra_reject");
    std::fs::create_dir_all(&ws_dir).unwrap();
    let ws_dir = ws_dir.canonicalize().unwrap();

    let config = PrimitiveConfig::default();
    let exec = DefaultPrimitiveExecutor::new(
        config,
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        ws_dir.clone(),
    )
    .with_extra_roots(vec![]);

    let r = exec.read_file("/tmp/some_other_path/file.txt", "p1").await;
    assert!(r.is_err());
    assert!(matches!(r.unwrap_err(), AppError::Permission(_)));

    let _ = std::fs::remove_dir_all(&ws_dir);
}
