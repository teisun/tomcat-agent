use super::super::*;
use std::path::Path;

#[test]
fn path_for_audit_returns_path_string() {
    let path = Path::new("/tmp/foo");
    let s = path_for_audit(path);
    assert_eq!(s, path.to_string_lossy().into_owned());
    assert!(s.contains("tmp"));
    assert!(s.contains("foo"));
}

#[test]
fn tracing_audit_recorder_default_works() {
    let r = TracingAuditRecorder;
    r.record_primitive(PrimitiveAuditEntry {
        operation: AuditPrimitiveOp::Read,
        path_or_cmd: "/x".to_string(),
        plugin_id: "p1".to_string(),
        user_approved: true,
        success: true,
        detail: None,
    });
}

#[test]
fn tracing_audit_recorder_records_primitive() {
    let r = TracingAuditRecorder;
    r.record_primitive(PrimitiveAuditEntry {
        operation: AuditPrimitiveOp::Read,
        path_or_cmd: "/tmp/foo".to_string(),
        plugin_id: "p1".to_string(),
        user_approved: true,
        success: true,
        detail: None,
    });
}

#[test]
fn tracing_audit_recorder_records_tool() {
    let r = TracingAuditRecorder;
    r.record_tool_call(ToolAuditEntry {
        tool_name: "run".to_string(),
        plugin_id: "p1".to_string(),
        caller_plugin_id: "p1".to_string(),
        success: true,
        detail: None,
    });
}

#[test]
fn tracing_audit_recorder_records_hostcall() {
    let r = TracingAuditRecorder;
    r.record_hostcall(HostcallAuditEntry {
        plugin_id: "p1".to_string(),
        module: "fs".to_string(),
        method: "readFile".to_string(),
        success: true,
        detail: None,
    });
}

#[test]
fn tracing_audit_recorder_records_plugin_lifecycle() {
    let r = TracingAuditRecorder;
    r.record_plugin_lifecycle(PluginLifecycleAuditEntry {
        plugin_id: "p1".to_string(),
        action: "load".to_string(),
        success: true,
        detail: None,
    });
}
