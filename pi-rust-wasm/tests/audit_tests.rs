//! 集成测试：审计日志模块（AuditStore、FileAuditRecorder）写入、查询、导出。
//! 黑盒测试，仅通过 pi_wasm 公共 API；使用临时目录隔离审计文件。
//!
//! 意义：TASK-04 审计日志系统——独立存储仅追加、可查询可导出、与 CLI 行为一致。

mod common;

use pi_wasm::{
    ensure_work_dir_structure, AppConfig, AuditRecorder, FileAuditRecorder,
    PluginLifecycleAuditEntry, PrimitiveAuditEntry,
};
use std::sync::Arc;
use tempfile::TempDir;

/// [AuditStore + FileAuditRecorder] 写入原语与插件生命周期记录后，query 可查出且 export_to 可导出
///
/// 验证：open_if_enabled 得到 store → FileAuditRecorder 写入 2 条 → query 返回 2 条 → export_to 生成文件且内容含两条
/// 意义：TASK-04 审计日志——独立存储写入与查询/导出端到端
#[test]
fn test_audit_store_write_query_export_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = tracing::info_span!("test_audit_store_write_query_export_roundtrip").entered();

    let tmp = TempDir::new()?;
    let work_dir = tmp.path().to_path_buf();
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(work_dir.to_string_lossy().to_string());
    cfg.security.enable_audit_log = true;
    cfg.security.audit_log_retention_days = 90;

    ensure_work_dir_structure(&cfg)?;
    let store = pi_wasm::AuditStore::open_if_enabled(&cfg)?
        .expect("enable_audit_log=true 且目录已创建，应返回 Some(store)");
    let store = Arc::new(store);
    let recorder = FileAuditRecorder::new(store.clone());

    recorder.record_primitive(PrimitiveAuditEntry {
        operation: pi_wasm::AuditPrimitiveOp::Read,
        path_or_cmd: "/tmp/foo".to_string(),
        plugin_id: "test_plugin".to_string(),
        user_approved: true,
        success: true,
        detail: None,
        permission_level: None,
        grant_source: None,
        in_working_dir: None,
    });
    recorder.record_plugin_lifecycle(PluginLifecycleAuditEntry {
        plugin_id: "test_plugin".to_string(),
        action: "load".to_string(),
        success: true,
        detail: None,
    });

    let filter = pi_wasm::AuditFilter::default();
    let entries = store.query(&filter)?;
    assert_eq!(entries.len(), 2, "应查出 2 条审计记录");
    assert_eq!(entries[0].kind_label(), "plugin_lifecycle");
    assert_eq!(entries[1].kind_label(), "primitive");

    let export_path = tmp.path().join("audit_export.json");
    store.export_to(&export_path)?;
    assert!(export_path.is_file(), "export_to 应生成文件");
    let content = std::fs::read_to_string(&export_path)?;
    assert!(content.contains("primitive"));
    assert!(content.contains("plugin_lifecycle"));

    Ok(())
}
