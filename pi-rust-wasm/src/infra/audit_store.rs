//! # 审计日志专用存储
//!
//! 独立 JSONL 文件、仅追加、不可篡改；支持查询、导出与按保留天数清理。
//! 与 [audit](super::audit) 模块的 `AuditRecorder` 配合，由 `FileAuditRecorder` 写入。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::config::AppConfig;
use super::error::AppError;
use super::platform::write_file_atomic;

/// 审计记录类型。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuditKindPayload {
    Primitive {
        operation: String,
        path_or_cmd: String,
        plugin_id: String,
        user_approved: bool,
        success: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    ToolCall {
        tool_name: String,
        plugin_id: String,
        caller_plugin_id: String,
        success: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    Hostcall {
        plugin_id: String,
        module: String,
        method: String,
        success: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    PluginLifecycle {
        plugin_id: String,
        action: String,
        success: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
}

/// 单条审计记录（存储格式：无 id，写入时仅 timestamp + payload）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntryRow {
    pub timestamp: String,
    #[serde(flatten)]
    pub payload: AuditKindPayload,
}

/// 对外查询/展示用的审计条目，带稳定 id（行号 1-based）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub id: u64,
    pub timestamp: String,
    #[serde(flatten)]
    pub payload: AuditKindPayload,
}

impl AuditEntry {
    /// 该条审计是否成功（依 kind 不同取对应 success 字段）。
    pub fn success(&self) -> bool {
        match &self.payload {
            AuditKindPayload::Primitive { success, .. }
            | AuditKindPayload::ToolCall { success, .. }
            | AuditKindPayload::Hostcall { success, .. }
            | AuditKindPayload::PluginLifecycle { success, .. } => *success,
        }
    }

    /// 类型标签，用于 CLI 展示。
    pub fn kind_label(&self) -> &'static str {
        match &self.payload {
            AuditKindPayload::Primitive { .. } => "primitive",
            AuditKindPayload::ToolCall { .. } => "tool_call",
            AuditKindPayload::Hostcall { .. } => "hostcall",
            AuditKindPayload::PluginLifecycle { .. } => "plugin_lifecycle",
        }
    }

    /// 简短详情字符串，用于 list 输出。
    pub fn detail_short(&self) -> String {
        match &self.payload {
            AuditKindPayload::Primitive {
                operation,
                path_or_cmd,
                plugin_id,
                ..
            } => format!("{} {} plugin_id={}", operation, path_or_cmd, plugin_id),
            AuditKindPayload::ToolCall {
                tool_name,
                plugin_id,
                ..
            } => format!("tool_name={} plugin_id={}", tool_name, plugin_id),
            AuditKindPayload::Hostcall {
                module,
                method,
                plugin_id,
                ..
            } => format!("{} {} plugin_id={}", module, method, plugin_id),
            AuditKindPayload::PluginLifecycle {
                plugin_id, action, ..
            } => format!("action={} plugin_id={}", action, plugin_id),
        }
    }
}

/// 查询过滤条件。
#[derive(Debug, Default, Clone)]
pub struct AuditFilter {
    /// 仅保留该时间之后（含）的记录（ISO8601 字符串比较）。
    pub since: Option<String>,
    /// 仅保留该时间之前（含）的记录。
    pub until: Option<String>,
    /// 仅保留指定类型。
    pub kind: Option<String>,
    /// 仅保留涉及该 plugin_id 的记录（primitive/tool_call/hostcall/plugin_lifecycle 的 plugin_id）。
    pub plugin_id: Option<String>,
    /// 最多返回条数（默认 50）；None 表示不限制。
    pub limit: Option<u32>,
}

/// 审计专用存储：单文件 JSONL，仅追加；支持查询、导出、按保留天数清理。
pub struct AuditStore {
    path: PathBuf,
    retention_days: u32,
    append_guard: Mutex<()>,
}

impl AuditStore {
    /// 从配置构建：解析审计目录与保留天数，审计文件为 `{audit_dir}/audit.jsonl`。
    pub fn new(cfg: &AppConfig) -> Result<Self, AppError> {
        let path = super::config::resolve_audit_dir(cfg)?.join("audit.jsonl");
        let retention_days = cfg.security.audit_log_retention_days;
        Ok(Self {
            path,
            retention_days,
            append_guard: Mutex::new(()),
        })
    }

    /// 仅当配置启用审计且目录可写时创建；否则返回 None（调用方使用 TracingAuditRecorder）。
    pub fn open_if_enabled(cfg: &AppConfig) -> Result<Option<Self>, AppError> {
        if !cfg.security.enable_audit_log {
            return Ok(None);
        }
        let store = Self::new(cfg)?;
        if let Some(parent) = store.path.parent() {
            std::fs::create_dir_all(parent).map_err(AppError::Io)?;
        }
        Ok(Some(store))
    }

    /// 追加一条记录（仅追加，不可篡改）。写入格式：一行 JSON + 换行。
    pub fn append(&self, row: &AuditEntryRow) -> Result<(), AppError> {
        let _guard = self
            .append_guard
            .lock()
            .map_err(|e| AppError::Audit(format!("audit append lock poisoned: {}", e)))?;
        let line = serde_json::to_string(row).map_err(|e| AppError::Audit(e.to_string()))?;
        let mut content = line;
        content.push('\n');
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(AppError::Io)?
            .write_all(content.as_bytes())
            .map_err(AppError::Io)?;
        Ok(())
    }

    /// 读取全部行并解析为带 id 的条目（id 为 1-based 行号），再应用过滤。
    pub fn query(&self, filter: &AuditFilter) -> Result<Vec<AuditEntry>, AppError> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let content = std::fs::read_to_string(&self.path).map_err(AppError::Io)?;
        let mut entries = Vec::new();
        for (zero_based, line) in content.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let row: AuditEntryRow = serde_json::from_str(line)
                .map_err(|e| AppError::Audit(format!("audit line parse: {}", e)))?;
            let id = (zero_based + 1) as u64;
            if !filter_matches(&row, id, filter) {
                continue;
            }
            entries.push(AuditEntry {
                id,
                timestamp: row.timestamp,
                payload: row.payload,
            });
        }
        // 文件按时间顺序写入，通常最新在末尾；list 习惯看最新在前，故反转。
        entries.reverse();
        let limit = filter.limit.unwrap_or(50) as usize;
        if entries.len() > limit {
            entries.truncate(limit);
        }
        Ok(entries)
    }

    /// 导出审计记录到指定路径（JSON 数组格式，便于阅读与工具处理）。
    pub fn export_to(&self, path: &Path) -> Result<(), AppError> {
        let filter = AuditFilter {
            limit: None,
            ..Default::default()
        };
        let entries = self.query(&filter)?;
        let json =
            serde_json::to_string_pretty(&entries).map_err(|e| AppError::Audit(e.to_string()))?;
        write_file_atomic(path, json.as_bytes())?;
        Ok(())
    }

    /// 按保留天数清理：删除早于 cutoff 的记录，重写文件（原子替换）。
    pub fn cleanup_retention(&self, days: u32) -> Result<(), AppError> {
        let days = if days == 0 { self.retention_days } else { days };
        let cutoff_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
            .saturating_sub(days as u64 * 24 * 3600);

        if !self.path.exists() {
            return Ok(());
        }
        let content = std::fs::read_to_string(&self.path).map_err(AppError::Io)?;
        let mut kept = Vec::new();
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let row: AuditEntryRow = match serde_json::from_str(line) {
                Ok(r) => r,
                Err(_) => continue,
            };
            let ts_secs = parse_timestamp_approx_secs(&row.timestamp).unwrap_or(0);
            if ts_secs >= cutoff_secs {
                kept.push(line.to_string());
            }
        }
        let new_content = kept.join("\n");
        let new_content = if new_content.is_empty() {
            String::new()
        } else {
            format!("{}\n", new_content)
        };
        write_file_atomic(&self.path, new_content.as_bytes())?;
        Ok(())
    }

    /// 使用配置中的保留天数执行清理。
    pub fn cleanup(&self) -> Result<(), AppError> {
        self.cleanup_retention(self.retention_days)
    }
}

fn filter_matches(row: &AuditEntryRow, _id: u64, f: &AuditFilter) -> bool {
    if let Some(ref since) = f.since {
        if row.timestamp.as_str() < since.as_str() {
            return false;
        }
    }
    if let Some(ref until) = f.until {
        if row.timestamp.as_str() > until.as_str() {
            return false;
        }
    }
    let kind_label = match &row.payload {
        AuditKindPayload::Primitive { .. } => "primitive",
        AuditKindPayload::ToolCall { .. } => "tool_call",
        AuditKindPayload::Hostcall { .. } => "hostcall",
        AuditKindPayload::PluginLifecycle { .. } => "plugin_lifecycle",
    };
    if let Some(ref k) = f.kind {
        if kind_label != k.as_str() {
            return false;
        }
    }
    if let Some(ref pid) = f.plugin_id {
        let entry_pid = match &row.payload {
            AuditKindPayload::Primitive { plugin_id, .. }
            | AuditKindPayload::ToolCall { plugin_id, .. }
            | AuditKindPayload::Hostcall { plugin_id, .. }
            | AuditKindPayload::PluginLifecycle { plugin_id, .. } => plugin_id.as_str(),
        };
        if entry_pid != pid.as_str() {
            return false;
        }
    }
    true
}

/// 将 ISO8601 或类似时间串解析为近似秒数（仅用于清理比较）。
fn parse_timestamp_approx_secs(ts: &str) -> Option<u64> {
    // 支持 "2025-03-13T12:00:00Z" 或 "2025-03-13T12:00:00.123Z"
    let parsed = chrono::DateTime::parse_from_rfc3339(ts).ok()?;
    let secs = parsed.timestamp();
    if secs < 0 {
        return None;
    }
    Some(secs as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_entry_row_roundtrip() {
        let row = AuditEntryRow {
            timestamp: "2025-03-13T12:00:00Z".to_string(),
            payload: AuditKindPayload::Primitive {
                operation: "Read".to_string(),
                path_or_cmd: "/tmp/foo".to_string(),
                plugin_id: "p1".to_string(),
                user_approved: true,
                success: true,
                detail: None,
            },
        };
        let j = serde_json::to_string(&row).unwrap();
        let back: AuditEntryRow = serde_json::from_str(&j).unwrap();
        assert_eq!(back.timestamp, row.timestamp);
    }

    #[test]
    fn audit_store_append_and_query() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        let store = AuditStore {
            path: path.clone(),
            retention_days: 90,
            append_guard: Mutex::new(()),
        };
        let row1 = AuditEntryRow {
            timestamp: "2025-03-13T10:00:00Z".to_string(),
            payload: AuditKindPayload::Primitive {
                operation: "Read".to_string(),
                path_or_cmd: "/tmp/a".to_string(),
                plugin_id: "p1".to_string(),
                user_approved: true,
                success: true,
                detail: None,
            },
        };
        let row2 = AuditEntryRow {
            timestamp: "2025-03-13T11:00:00Z".to_string(),
            payload: AuditKindPayload::ToolCall {
                tool_name: "run".to_string(),
                plugin_id: "p1".to_string(),
                caller_plugin_id: "p1".to_string(),
                success: true,
                detail: None,
            },
        };
        store.append(&row1).unwrap();
        store.append(&row2).unwrap();
        let entries = store.query(&AuditFilter::default()).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, 2);
        assert_eq!(entries[0].kind_label(), "tool_call");
        assert_eq!(entries[1].id, 1);
        assert_eq!(entries[1].kind_label(), "primitive");
    }
}
