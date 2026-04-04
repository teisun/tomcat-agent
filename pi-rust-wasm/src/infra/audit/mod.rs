//! # 审计记录扩展点
//!
//! 4 原语、工具调用、插件生命周期等关键路径通过本 trait 记录审计；
//! P0 提供基于 tracing 的默认实现，T1-P1-001 可替换为完整审计模块（FileAuditRecorder + AuditStore）。

use std::path::Path;
use std::sync::Arc;

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

/// 插件生命周期审计记录（load / enable / disable / unload）。
#[derive(Debug, Clone)]
pub struct PluginLifecycleAuditEntry {
    pub plugin_id: String,
    pub action: String,
    pub success: bool,
    pub detail: Option<String>,
}

/// 审计记录器：4 原语、工具调用、Hostcall 与插件生命周期等写入此处，P0 可用 tracing，P1 可落盘。
pub trait AuditRecorder: Send + Sync + 'static {
    /// 记录 4 原语操作。
    fn record_primitive(&self, entry: PrimitiveAuditEntry);
    /// 记录工具调用。
    fn record_tool_call(&self, entry: ToolAuditEntry);
    /// 记录单笔 Hostcall（来源插件、module/method、成功与否、可选详情）。
    fn record_hostcall(&self, entry: HostcallAuditEntry);
    /// 记录插件生命周期操作（load / enable / disable / unload）。
    fn record_plugin_lifecycle(&self, entry: PluginLifecycleAuditEntry);
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

    fn record_plugin_lifecycle(&self, entry: PluginLifecycleAuditEntry) {
        tracing::info!(
            plugin_id = %entry.plugin_id,
            action = %entry.action,
            success = entry.success,
            detail = ?entry.detail,
            "audit plugin_lifecycle"
        );
    }
}

/// 基于专用 JSONL 文件的审计记录器，仅追加、不可篡改；与 [`super::audit_store::AuditStore`] 配合使用。
#[derive(Clone)]
pub struct FileAuditRecorder {
    store: Arc<super::audit_store::AuditStore>,
}

impl FileAuditRecorder {
    /// 使用已有 AuditStore 构造。
    pub fn new(store: Arc<super::audit_store::AuditStore>) -> Self {
        Self { store }
    }
}

impl AuditRecorder for FileAuditRecorder {
    fn record_primitive(&self, entry: PrimitiveAuditEntry) {
        let op = match entry.operation {
            AuditPrimitiveOp::Read => "Read",
            AuditPrimitiveOp::Write => "Write",
            AuditPrimitiveOp::Edit => "Edit",
            AuditPrimitiveOp::Bash => "Bash",
        };
        let row = super::audit_store::AuditEntryRow {
            timestamp: chrono::Utc::now().to_rfc3339(),
            payload: super::audit_store::AuditKindPayload::Primitive {
                operation: op.to_string(),
                path_or_cmd: entry.path_or_cmd,
                plugin_id: entry.plugin_id,
                user_approved: entry.user_approved,
                success: entry.success,
                detail: entry.detail,
            },
        };
        let _ = self.store.append(&row);
    }

    fn record_tool_call(&self, entry: ToolAuditEntry) {
        let row = super::audit_store::AuditEntryRow {
            timestamp: chrono::Utc::now().to_rfc3339(),
            payload: super::audit_store::AuditKindPayload::ToolCall {
                tool_name: entry.tool_name,
                plugin_id: entry.plugin_id,
                caller_plugin_id: entry.caller_plugin_id,
                success: entry.success,
                detail: entry.detail,
            },
        };
        let _ = self.store.append(&row);
    }

    fn record_hostcall(&self, entry: HostcallAuditEntry) {
        let row = super::audit_store::AuditEntryRow {
            timestamp: chrono::Utc::now().to_rfc3339(),
            payload: super::audit_store::AuditKindPayload::Hostcall {
                plugin_id: entry.plugin_id,
                module: entry.module,
                method: entry.method,
                success: entry.success,
                detail: entry.detail,
            },
        };
        let _ = self.store.append(&row);
    }

    fn record_plugin_lifecycle(&self, entry: PluginLifecycleAuditEntry) {
        let row = super::audit_store::AuditEntryRow {
            timestamp: chrono::Utc::now().to_rfc3339(),
            payload: super::audit_store::AuditKindPayload::PluginLifecycle {
                plugin_id: entry.plugin_id,
                action: entry.action,
                success: entry.success,
                detail: entry.detail,
            },
        };
        let _ = self.store.append(&row);
    }
}

/// 将 path 转为可审计的字符串（避免敏感路径在日志中完整暴露时可做脱敏）。
#[allow(dead_code)]
pub fn path_for_audit(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests;
