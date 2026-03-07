//! # 审计记录扩展点
//!
//! 4 原语、工具调用、插件生命周期等关键路径通过本 trait 记录审计；
//! P0 提供基于 tracing 的默认实现，T1-P1-001 可替换为完整审计模块。

use std::path::Path;

/// 原语操作类型，与 core::PrimitiveOperation 对齐，避免 core 依赖 infra 的循环。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditPrimitiveOp {
    Read,
    Write,
    Edit,
    Bash,
}

/// 单条原语审计记录。
#[derive(Debug, Clone)]
pub struct PrimitiveAuditEntry {
    pub operation: AuditPrimitiveOp,
    pub path_or_cmd: String,
    pub plugin_id: String,
    pub user_approved: bool,
    pub success: bool,
    pub detail: Option<String>,
}

/// 工具调用审计记录。
#[derive(Debug, Clone)]
pub struct ToolAuditEntry {
    pub tool_name: String,
    pub plugin_id: String,
    pub caller_plugin_id: String,
    pub success: bool,
    pub detail: Option<String>,
}

/// 单笔 Hostcall 审计记录（008 分发层统一记录）。
#[derive(Debug, Clone)]
pub struct HostcallAuditEntry {
    pub plugin_id: String,
    pub module: String,
    pub method: String,
    pub success: bool,
    pub detail: Option<String>,
}

/// 审计记录器：4 原语、工具调用与 Hostcall 等写入此处，P0 可用 tracing，P1 可落盘。
pub trait AuditRecorder: Send + Sync + 'static {
    /// 记录 4 原语操作。
    fn record_primitive(&self, entry: PrimitiveAuditEntry);
    /// 记录工具调用。
    fn record_tool_call(&self, entry: ToolAuditEntry);
    /// 记录单笔 Hostcall（来源插件、module/method、成功与否、可选详情）。
    fn record_hostcall(&self, entry: HostcallAuditEntry);
}

/// 默认实现：仅通过 tracing 输出，便于 T1-P1-001 替换为持久化实现。
#[derive(Debug, Default)]
pub struct TracingAuditRecorder;

impl AuditRecorder for TracingAuditRecorder {
    fn record_primitive(&self, entry: PrimitiveAuditEntry) {
        tracing::info!(
            operation = ?entry.operation,
            path_or_cmd = %entry.path_or_cmd,
            plugin_id = %entry.plugin_id,
            user_approved = entry.user_approved,
            success = entry.success,
            detail = ?entry.detail,
            "audit primitive"
        );
    }

    fn record_tool_call(&self, entry: ToolAuditEntry) {
        tracing::info!(
            tool_name = %entry.tool_name,
            plugin_id = %entry.plugin_id,
            caller_plugin_id = %entry.caller_plugin_id,
            success = entry.success,
            detail = ?entry.detail,
            "audit tool_call"
        );
    }

    fn record_hostcall(&self, entry: HostcallAuditEntry) {
        tracing::info!(
            plugin_id = %entry.plugin_id,
            module = %entry.module,
            method = %entry.method,
            success = entry.success,
            detail = ?entry.detail,
            "audit hostcall"
        );
    }
}

/// 将 path 转为可审计的字符串（避免敏感路径在日志中完整暴露时可做脱敏）。
#[allow(dead_code)]
pub fn path_for_audit(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
