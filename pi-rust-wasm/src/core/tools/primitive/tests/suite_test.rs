use super::super::*;
use crate::core::permission::{DefaultPermissionGate, GateConfig, PermissionGate, SessionGrants};
use crate::core::tools::primitive::{
    EditOperation, EditOperationType, PrimitiveExecutor, PrimitiveOperation,
};
use crate::core::{AllowAllConfirmation, DenyAllConfirmation};
use crate::infra::error::AppError;
use crate::infra::{
    AuditRecorder, PrimitiveAuditEntry, PrimitiveConfig, ToolAuditEntry, TracingAuditRecorder,
};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

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

#[tokio::test]
async fn read_file_success() {
    let dir = std::env::temp_dir().join("pi_wasm_exec_read");
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
async fn read_file_binary_returns_product_error() {
    let dir = std::env::temp_dir().join("pi_wasm_exec_binary_read");
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
    let dir = std::env::temp_dir().join("pi_wasm_exec_list");
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
    let dir = std::env::temp_dir().join("pi_wasm_exec_write");
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
async fn write_file_user_denied_returns_permission_and_audit() {
    let dir = std::env::temp_dir().join("pi_wasm_exec_deny");
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
    let dir = std::env::temp_dir().join("pi_wasm_exec_edit");
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
async fn execute_bash_success() {
    let dir = std::env::temp_dir().join("pi_wasm_exec_bash");
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
        .execute_bash("echo ok", Some(&path_str), "p1", None)
        .await
        .unwrap();
    assert_eq!(res.exit_code, 0);
    assert!(res.stdout.trim().contains("ok"));
    let _ = std::fs::remove_dir(&dir);
}

#[tokio::test]
async fn execute_bash_forbidden() {
    // builtin bash_forbidden 命中 (`pi config set llm.api_key`) → Deny；
    // 走 gate 主路径，不再依赖 PrimitiveConfig.bash_forbidden 字段。
    let dir = std::env::temp_dir().join("pi_wasm_exec_forbid");
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
        .execute_bash("pi config set llm.api_key xxx", Some(&path_str), "p1", None)
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
    let exec = DefaultPrimitiveExecutor::new(
        PrimitiveConfig::default(),
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
    let dir = std::env::temp_dir().join("pi_wasm_exec_read_dir");
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
    let dir = std::env::temp_dir().join("pi_wasm_exec_autoconfirm");
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
    let dir = std::env::temp_dir().join("pi_wasm_exec_backup");
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
async fn workspace_roots_allow_external_path() {
    let ws_dir = std::env::temp_dir().join("pi_wasm_exec_extra_ws");
    std::fs::create_dir_all(&ws_dir).unwrap();
    let ws_dir = ws_dir.canonicalize().unwrap();

    let ext_dir = std::env::temp_dir().join("pi_wasm_exec_extra_ext");
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
    let ws_dir = std::env::temp_dir().join("pi_wasm_exec_extra_reject");
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
