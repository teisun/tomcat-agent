//! # 4+1 原语执行引擎（DefaultPrimitiveExecutor）
//!
//! 实现 [`crate::core::primitives::PrimitiveExecutor`] trait，是 Agent 与文件系统 / Shell
//! 的 **唯一受信通道**：任何 LLM 工具调用最终都要落到这 5 个方法上，安全策略
//! （白名单 / 用户确认 / 备份 / 原子写 / 审计）全部在此横切。
//!
//! ## 5 个原语 + 共享安全流水
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │  入口（trait 方法）                                                       │
//! │   ├─ read_file(path, plugin_id)                                          │
//! │   ├─ list_dir(path, plugin_id)                                           │
//! │   ├─ write_file(path, content, plugin_id, ...)                           │
//! │   ├─ edit_file(path, operations, plugin_id, ...)                         │
//! │   └─ execute_bash(cmd, args, plugin_id, ...)                             │
//! └─────────────────────────────────────────────────────────────────────────┘
//!    │
//!    │ ① 路径规范化                       ┌──────────────────────────────┐
//!    │   normalize_path(workspace, path) ─┤ infra::platform              │
//!    │   ── ~ 展开、相对路径解析、         │ （read_file_utf8 / write_     │
//!    │      symlink 还原、Win 反斜杠       │  file_atomic 也在这里）       │
//!    │                                    └──────────────────────────────┘
//!    │
//!    │ ② 路径校验（白名单合并）
//!    │   合法当且仅当 path 落入：
//!    │     workspace_dir          （隐式根，用户当前工作目录）
//!    │   ∪ extra_roots            （pi.config.toml [workspace] extra_roots）
//!    │   ∪ PrimitiveConfig.path_whitelist（运行时显式追加）
//!    │   未命中 ► AppError::Permission
//!    │
//!    │ ③ 用户确认（写类操作 + bash）
//!    │   confirmation.confirm(op_summary)
//!    │     ├─ Approve  ► 继续
//!    │     ├─ Deny     ► AppError::Permission(用户拒绝)
//!    │     └─ Timeout  ► 视为 Deny
//!    │   read_file / list_dir 跳过（只读）
//!    │
//!    │ ④ 业务执行（按方法分支）
//!    │   read_file    ► 大小预检（≤ MAX_READ_BYTES=10MB）► read_file_utf8
//!    │   list_dir     ► std::fs::read_dir → DirEntry 列表
//!    │   write_file   ► （可选）备份原文件 ► write_file_atomic（写临时 + rename）
//!    │   edit_file    ► load → apply EditOperation* → diff → 备份 ► atomic 写
//!    │   execute_bash ► 命令白名单校验 ► tokio::process::Command spawn
//!    │                  ► （未来：BASH_TIMEOUT_SECS 配合 timeout）
//!    │
//!    │ ⑤ 审计落库（无论 Ok / Err）
//!    │   audit.record_primitive(PrimitiveAuditEntry {
//!    │     plugin_id, op: AuditPrimitiveOp::*, success, detail, ...
//!    │   })
//!    ▼
//!   Result<T, AppError>
//! ```
//!
//! ## 横切配置
//!
//! - `PrimitiveConfig`（来自 [`crate::infra::PrimitiveConfig`]）：路径白名单 +
//!   命令白名单 + 备份策略。
//! - `MAX_READ_BYTES = 10 MiB`：read_file 单次读上限，防 OOM。
//! - `BASH_TIMEOUT_SECS = 30`：bash 默认超时（当前预留，后续接 `tokio::time::timeout`）。
//! - `workspace_dir` + `extra_roots`：构造时注入，后者来自配置文件，前者来自
//!   CLI 当前目录，两者并集 ∪ `path_whitelist` 形成最终白名单。
//!
//! ## 与同族子模块的边界
//!
//! - `super::diff::build_simple_diff`：edit_file 的 diff 文本生成。
//! - `super::primitives` 与 `super::confirmation`：trait + 用户确认 trait。
//! - 调用方：`agent_loop::tool_exec::execute_tool` 是唯一直接调用方，所有
//!   LLM 工具调用都从那里 dispatch 进来。

use crate::core::confirmation::{ConfirmDecision, UserConfirmationProvider};
use crate::core::permission::{GrantSource, PermissionDecision, PermissionGate, PermissionLevel};
use crate::core::primitives::{
    BashResult, DirEntry, EditFileResult, EditOperation, EditOperationType, PrimitiveExecutor,
    PrimitiveOperation, WriteFileResult,
};
use crate::infra::audit::{AuditPrimitiveOp, AuditRecorder, PrimitiveAuditEntry};
use crate::infra::error::AppError;
use crate::infra::platform::{normalize_path, read_file_utf8, write_file_atomic};
use crate::infra::PrimitiveConfig;
use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::process::Command;
use tracing::debug;

use super::diff::build_simple_diff;

/// 单次读取文件最大字节数，避免 OOM。
const MAX_READ_BYTES: u64 = 10 * 1024 * 1024; // 10 MiB
/// bash 执行默认超时（预留，后续可配合 tokio::time::timeout 使用）。
#[allow(dead_code)]
const BASH_TIMEOUT_SECS: u64 = 30;

/// 4 原语执行引擎默认实现：路径权限、用户确认、备份、原子化与审计。
///
/// **权限模型**：当注入了 [`PermissionGate`] 时（`with_gate`），路径与 bash 权限
/// 完全交由 gate 三层决策；否则回退到 legacy 模式（`config.path_whitelist` ∪
/// `workspace_dir` ∪ `extra_roots`），以保留旧测试 / 旧调用方零行为变化。
pub struct DefaultPrimitiveExecutor {
    config: PrimitiveConfig,
    confirmation: Arc<dyn UserConfirmationProvider>,
    audit: Arc<dyn AuditRecorder>,
    /// 默认工作目录：path_whitelist 为空时作为隐式白名单根目录。
    workspace_dir: PathBuf,
    /// `pi.config.toml` 中 `[workspace] extra_roots` 的额外授权根路径，与 `workspace_dir` 并集（全局，所有 agent 共用）。
    extra_roots: Vec<PathBuf>,
    /// 注入后取代 legacy 路径白名单与 confirm 决策；为 None 时走 legacy 模式。
    gate: Option<Arc<dyn PermissionGate>>,
}

impl DefaultPrimitiveExecutor {
    pub fn new(
        config: PrimitiveConfig,
        confirmation: Arc<dyn UserConfirmationProvider>,
        audit: Arc<dyn AuditRecorder>,
        workspace_dir: PathBuf,
    ) -> Self {
        Self {
            config,
            confirmation,
            audit,
            workspace_dir,
            extra_roots: Vec::new(),
            gate: None,
        }
    }

    /// 设置配置中解析得到的额外授权根路径（与 `pi workspace` / TOML 同源）。
    pub fn with_extra_roots(mut self, roots: Vec<PathBuf>) -> Self {
        self.extra_roots = roots;
        self
    }

    /// 注入 [`PermissionGate`]：路径与 bash 权限改走 3 层决策。
    pub fn with_gate(mut self, gate: Arc<dyn PermissionGate>) -> Self {
        self.gate = Some(gate);
        self
    }

    /// 路径白名单/黑名单校验；通过则返回规范化后的 PathBuf。
    ///
    /// 优先级：config.path_whitelist > (workspace_dir ∪ extra_roots)。
    /// 当 `path_whitelist` 为空时，以 `workspace_dir` 和 `extra_roots` 的并集作为隐式白名单。
    fn check_path(&self, path: &str) -> Result<PathBuf, AppError> {
        let normalized = normalize_path(path)?;
        let s = normalized.to_string_lossy();
        let allowed = if self.config.path_whitelist.is_empty() {
            let ws = self.workspace_dir.to_string_lossy();
            normalized_starts_with(&s, &ws)
                || self
                    .extra_roots
                    .iter()
                    .any(|r| normalized_starts_with(&s, &r.to_string_lossy()))
        } else {
            self.config
                .path_whitelist
                .iter()
                .any(|w| normalized_starts_with(&s, w))
        };
        if !allowed {
            return Err(AppError::Permission(format!("路径不在白名单内: {}", path)));
        }
        Ok(normalized)
    }

    /// 是否需要对该操作进行用户确认（legacy 模式：write/edit/bash 一律需要，
    /// 除非 `auto_confirm = true`）。gate 模式下走 `gate_check_*`，本函数不被调用。
    fn needs_confirmation(&self, op: PrimitiveOperation) -> bool {
        if self.config.auto_confirm {
            return false;
        }
        !matches!(op, PrimitiveOperation::Read)
    }

    // ─────────────────────────────────────────────────────────────────────
    // PermissionGate 桥接（gate 注入后启用）
    // ─────────────────────────────────────────────────────────────────────

    /// 经 gate 决定一个原语对路径的访问，必要时弹 confirm 完成 layer-2。
    ///
    /// 返回 `Ok(Some((path_buf, level, source)))` 表示放行；
    /// `Ok(None)` 不应出现（要么 Allow 要么 Err）；
    /// `Err(AppError::Permission)` 表示被 gate 拒绝或用户拒绝 confirm。
    async fn gate_check_path(
        &self,
        op: PrimitiveOperation,
        path: &str,
        plugin_id: &str,
    ) -> Result<(PathBuf, PermissionLevel, GrantSource), AppError> {
        let gate = self
            .gate
            .as_ref()
            .expect("gate_check_path requires gate to be present");
        let normalized = normalize_path(path)?;
        loop {
            let decision = gate.check(op, &normalized.to_string_lossy())?;
            match decision {
                PermissionDecision::Allow { source, level } => {
                    return Ok((normalized, level, source))
                }
                PermissionDecision::Deny { reason } => {
                    return Err(AppError::Permission(reason));
                }
                PermissionDecision::NeedConfirm {
                    reason,
                    suggested_root,
                } => {
                    let preview = format!(
                        "[{:?}] {}\n路径: {}\n原因: {}",
                        op,
                        op_summary(op),
                        normalized.display(),
                        reason
                    );
                    let dec = self
                        .confirmation
                        .confirm_decision(op, &preview, plugin_id, suggested_root.clone())
                        .await?;
                    match dec {
                        ConfirmDecision::Deny => {
                            return Err(AppError::Permission(format!(
                                "用户拒绝授权: {}",
                                normalized.display()
                            )));
                        }
                        ConfirmDecision::AllowOnce => {
                            // 落 SessionGrants：用 suggested_root（一般是父目录）作为前缀，
                            // 这样同目录下后续访问无需再 confirm。
                            let target = suggested_root.unwrap_or_else(|| normalized.clone());
                            gate.grant_session(target, GrantSource::SessionGrant);
                            // 重新 check：现在应该 Allow。
                            continue;
                        }
                        ConfirmDecision::AllowAndPersistRoot { root } => {
                            // 1) 同时落 SessionGrants（本会话生效）。
                            gate.grant_session(root.clone(), GrantSource::SessionGrant);
                            // 2) 持久化由 caller（CLI confirm 实现）负责调用
                            //    `pi workspace add` 等价的 append_extra_root_to_disk；
                            //    这里只标记会话授权，避免和 disk 写入耦合。
                            //    重新 check 应 Allow。
                            continue;
                        }
                    }
                }
            }
        }
    }

    /// 经 gate 决定一条 bash 命令是否放行；layer-2 命中弹 confirm。
    async fn gate_check_bash(
        &self,
        command: &str,
        plugin_id: &str,
    ) -> Result<(PermissionLevel, GrantSource), AppError> {
        let gate = self
            .gate
            .as_ref()
            .expect("gate_check_bash requires gate to be present");
        let decision = gate.check_bash(command)?;
        match decision {
            PermissionDecision::Allow { source, level } => Ok((level, source)),
            PermissionDecision::Deny { reason } => Err(AppError::Permission(reason)),
            PermissionDecision::NeedConfirm { reason, .. } => {
                let preview = format!(
                    "[Bash] 危险命令命中确认列表\n命令: {}\n原因: {}",
                    command, reason
                );
                let dec = self
                    .confirmation
                    .confirm_decision(PrimitiveOperation::Bash, &preview, plugin_id, None)
                    .await?;
                match dec {
                    ConfirmDecision::AllowOnce | ConfirmDecision::AllowAndPersistRoot { .. } => {
                        Ok((PermissionLevel::BashApproval, GrantSource::SessionGrant))
                    }
                    ConfirmDecision::Deny => {
                        Err(AppError::Permission("用户拒绝 bash 确认".to_string()))
                    }
                }
            }
        }
    }
}

fn op_summary(op: PrimitiveOperation) -> &'static str {
    match op {
        PrimitiveOperation::Read => "读取",
        PrimitiveOperation::Write => "写入",
        PrimitiveOperation::Edit => "编辑",
        PrimitiveOperation::Bash => "执行命令",
    }
}

/// 把 [`PermissionLevel`] 序列化为审计字符串（与 serde rename_all = snake_case 一致）。
fn permission_level_str(l: PermissionLevel) -> String {
    match l {
        PermissionLevel::Read => "read",
        PermissionLevel::Write => "write",
        PermissionLevel::BashWhitelist => "bash_whitelist",
        PermissionLevel::BashApproval => "bash_approval",
        PermissionLevel::Forbidden => "forbidden",
    }
    .to_string()
}

/// 把 [`GrantSource`] 序列化为审计字符串。
fn grant_source_str(s: GrantSource) -> String {
    match s {
        GrantSource::AgentWorkspace => "agent_workspace",
        GrantSource::AgentDataDir => "agent_data_dir",
        GrantSource::ConfigExtraRoot => "config_extra_root",
        GrantSource::SessionGrant => "session_grant",
        GrantSource::DraggedPath => "dragged_path",
        GrantSource::PathRuleReadOnly => "path_rule_read_only",
        GrantSource::BashWhitelist => "bash_whitelist",
        GrantSource::AutoConfirmFlag => "auto_confirm_flag",
    }
    .to_string()
}

/// 由 GrantSource 推断「是否在工作目录内」（与 plan §2 的 `in_working_dir` 字段一致）。
fn in_working_dir_from_source(s: GrantSource) -> bool {
    matches!(
        s,
        GrantSource::AgentWorkspace | GrantSource::ConfigExtraRoot
    )
}

fn normalized_starts_with(path: &str, prefix: &str) -> bool {
    let path = path.trim_end_matches(std::path::MAIN_SEPARATOR);
    let prefix = prefix.trim_end_matches(std::path::MAIN_SEPARATOR);
    path == prefix || path.starts_with(&format!("{}{}", prefix, std::path::MAIN_SEPARATOR))
}

#[async_trait]
impl PrimitiveExecutor for DefaultPrimitiveExecutor {
    async fn read_file(&self, path: &str, plugin_id: &str) -> Result<String, AppError> {
        let (path_buf, level, source) = if self.gate.is_some() {
            let (p, l, s) = self
                .gate_check_path(PrimitiveOperation::Read, path, plugin_id)
                .await?;
            (p, Some(l), Some(s))
        } else {
            (self.check_path(path)?, None, None)
        };
        let meta = std::fs::metadata(&path_buf).map_err(AppError::Io)?;
        if meta.is_dir() {
            return Err(AppError::Primitive(
                "路径是目录，无法读取为文件".to_string(),
            ));
        }
        if meta.len() > MAX_READ_BYTES {
            return Err(AppError::Primitive(format!(
                "文件过大 ({} bytes)，超过限制 {} bytes",
                meta.len(),
                MAX_READ_BYTES
            )));
        }
        let content = read_file_utf8(&path_buf).map_err(|e| match e {
            AppError::Config(msg) if msg.contains("invalid utf-8") => AppError::Primitive(format!(
                "文件存在且权限已通过检查，但它是二进制或非 UTF-8 文本，不能用 read_file 按文本读取：{}",
                path_buf.display()
            )),
            other => other,
        })?;
        self.audit.record_primitive(PrimitiveAuditEntry {
            operation: AuditPrimitiveOp::Read,
            path_or_cmd: path.to_string(),
            plugin_id: plugin_id.to_string(),
            user_approved: true,
            success: true,
            detail: None,
            permission_level: level.map(permission_level_str),
            grant_source: source.map(grant_source_str),
            in_working_dir: source.map(in_working_dir_from_source),
        });
        Ok(content)
    }

    async fn list_dir(&self, path: &str, plugin_id: &str) -> Result<Vec<DirEntry>, AppError> {
        let (path_buf, level, source) = if self.gate.is_some() {
            let (p, l, s) = self
                .gate_check_path(PrimitiveOperation::Read, path, plugin_id)
                .await?;
            (p, Some(l), Some(s))
        } else {
            (self.check_path(path)?, None, None)
        };
        let read = std::fs::read_dir(&path_buf).map_err(AppError::Io)?;
        let mut entries = Vec::new();
        for e in read {
            let e = e.map_err(AppError::Io)?;
            let name = e.file_name().to_string_lossy().into_owned();
            let is_dir = e.file_type().map_err(AppError::Io)?.is_dir();
            entries.push(DirEntry { name, is_dir });
        }
        self.audit.record_primitive(PrimitiveAuditEntry {
            operation: AuditPrimitiveOp::Read,
            path_or_cmd: path.to_string(),
            plugin_id: plugin_id.to_string(),
            user_approved: true,
            success: true,
            detail: Some(format!("list_dir {} entries", entries.len())),
            permission_level: level.map(permission_level_str),
            grant_source: source.map(grant_source_str),
            in_working_dir: source.map(in_working_dir_from_source),
        });
        Ok(entries)
    }

    async fn write_file(
        &self,
        path: &str,
        content: &str,
        overwrite: bool,
        plugin_id: &str,
    ) -> Result<WriteFileResult, AppError> {
        let (path_buf, level, source) = if self.gate.is_some() {
            let (p, l, s) = self
                .gate_check_path(PrimitiveOperation::Write, path, plugin_id)
                .await?;
            (p, Some(l), Some(s))
        } else {
            (self.check_path(path)?, None, None)
        };
        let path_str = path_buf.to_string_lossy().to_string();

        if overwrite && path_buf.exists() {
            let backup = path_buf.with_extension("bak");
            let _ = std::fs::copy(&path_buf, &backup);
        }

        let preview = if path_buf.exists() {
            format!("覆盖文件 {} ({} bytes)", path, content.len())
        } else {
            format!("写入新文件 {} ({} bytes)", path, content.len())
        };
        if self.gate.is_none() && self.needs_confirmation(PrimitiveOperation::Write) {
            debug!("[tool_debug] 请求用户确认写入 path={}", path);
            let ok = self
                .require_user_confirmation(PrimitiveOperation::Write, &preview, plugin_id)
                .await?;
            if !ok {
                debug!("[tool_debug] 用户拒绝写入确认 path={}", path);
                self.audit.record_primitive(PrimitiveAuditEntry {
                    operation: AuditPrimitiveOp::Write,
                    path_or_cmd: path_str.clone(),
                    plugin_id: plugin_id.to_string(),
                    user_approved: false,
                    success: false,
                    detail: Some("用户拒绝确认".to_string()),
                    ..Default::default()
                });
                return Err(AppError::Permission("用户拒绝写入确认".to_string()));
            }
        }
        let _ = preview;

        write_file_atomic(&path_buf, content.as_bytes())?;
        self.audit.record_primitive(PrimitiveAuditEntry {
            operation: AuditPrimitiveOp::Write,
            path_or_cmd: path_str,
            plugin_id: plugin_id.to_string(),
            user_approved: true,
            success: true,
            detail: None,
            permission_level: level.map(permission_level_str),
            grant_source: source.map(grant_source_str),
            in_working_dir: source.map(in_working_dir_from_source),
        });
        Ok(WriteFileResult {
            path: path.to_string(),
            written: true,
        })
    }

    async fn edit_file(
        &self,
        path: &str,
        edits: Vec<EditOperation>,
        plugin_id: &str,
    ) -> Result<EditFileResult, AppError> {
        let (path_buf, level, source) = if self.gate.is_some() {
            let (p, l, s) = self
                .gate_check_path(PrimitiveOperation::Edit, path, plugin_id)
                .await?;
            (p, Some(l), Some(s))
        } else {
            (self.check_path(path)?, None, None)
        };
        let path_str = path_buf.to_string_lossy().to_string();
        let content = read_file_utf8(&path_buf)?;
        let mut lines: Vec<String> = content.lines().map(String::from).collect();

        let backup_path = path_buf.with_extension("bak");
        std::fs::copy(&path_buf, &backup_path).map_err(AppError::Io)?;

        for edit in &edits {
            match edit.operation_type {
                EditOperationType::Replace => {
                    if edit.start_line.is_none() {
                        if let Some(ref old) = edit.old_content {
                            let full_text = lines.join("\n");
                            let count = full_text.matches(old.as_str()).count();
                            if count == 0 {
                                let _ = std::fs::copy(&backup_path, &path_buf);
                                return Err(AppError::Primitive(format!(
                                    "edit_file: 未找到匹配的 old_content（文件 {}）",
                                    path
                                )));
                            }
                            if count > 1 {
                                let _ = std::fs::copy(&backup_path, &path_buf);
                                return Err(AppError::Primitive(format!(
                                    "edit_file: old_content 在文件中出现 {} 次，需要更多上下文使其唯一",
                                    count
                                )));
                            }
                            let new_text = full_text.replacen(old.as_str(), &edit.new_content, 1);
                            lines = new_text.lines().map(String::from).collect();
                        }
                    } else if let Some(start_line_val) = edit.start_line {
                        let start = start_line_val as usize;
                        let end = edit.end_line.unwrap_or(start_line_val) as usize;
                        if start < 1 || end > lines.len() || start > end {
                            let _ = std::fs::copy(&backup_path, &path_buf);
                            return Err(AppError::Primitive(format!(
                                "Replace 行号无效: {}..{}",
                                start, end
                            )));
                        }
                        let idx = start - 1;
                        let new_lines: Vec<String> =
                            edit.new_content.lines().map(String::from).collect();
                        for (i, nl) in new_lines.iter().enumerate() {
                            if idx + i < lines.len() {
                                lines[idx + i] = nl.clone();
                            } else {
                                lines.push(nl.clone());
                            }
                        }
                        for i in (idx + new_lines.len())..end {
                            if i < lines.len() {
                                lines[i] = String::new();
                            }
                        }
                    }
                }
                EditOperationType::Insert => {
                    let at = edit.start_line.unwrap_or(0) as usize;
                    if at > lines.len() {
                        let _ = std::fs::copy(&backup_path, &path_buf);
                        return Err(AppError::Primitive(format!("Insert 行号超出: {}", at)));
                    }
                    lines.insert(at, edit.new_content.clone());
                }
                EditOperationType::Delete => {
                    let start = edit.start_line.unwrap_or(1) as usize;
                    let end = edit.end_line.unwrap_or(start as u64) as usize;
                    if start < 1 || end > lines.len() || start > end {
                        let _ = std::fs::copy(&backup_path, &path_buf);
                        return Err(AppError::Primitive(format!(
                            "Delete 行号无效: {}..{}",
                            start, end
                        )));
                    }
                    for _ in 0..=(end - start) {
                        if start <= lines.len() {
                            lines.remove(start - 1);
                        }
                    }
                }
            }
        }

        let new_content = lines.join("\n");
        let diff_preview = build_simple_diff(content.as_str(), &new_content);
        if self.gate.is_none() && self.needs_confirmation(PrimitiveOperation::Edit) {
            let ok = self
                .require_user_confirmation(
                    PrimitiveOperation::Edit,
                    &format!("编辑 {}:\n{}", path, diff_preview),
                    plugin_id,
                )
                .await?;
            if !ok {
                let _ = std::fs::copy(&backup_path, &path_buf);
                let _ = std::fs::remove_file(&backup_path);
                self.audit.record_primitive(PrimitiveAuditEntry {
                    operation: AuditPrimitiveOp::Edit,
                    path_or_cmd: path_str.clone(),
                    plugin_id: plugin_id.to_string(),
                    user_approved: false,
                    success: false,
                    detail: Some("用户拒绝确认".to_string()),
                    ..Default::default()
                });
                return Err(AppError::Permission("用户拒绝编辑确认".to_string()));
            }
        }

        if let Err(e) = write_file_atomic(&path_buf, new_content.as_bytes()) {
            let _ = std::fs::copy(&backup_path, &path_buf);
            let _ = std::fs::remove_file(&backup_path);
            self.audit.record_primitive(PrimitiveAuditEntry {
                operation: AuditPrimitiveOp::Edit,
                path_or_cmd: path_str,
                plugin_id: plugin_id.to_string(),
                user_approved: true,
                success: false,
                detail: Some(e.to_string()),
                permission_level: level.map(permission_level_str),
                grant_source: source.map(grant_source_str),
                in_working_dir: source.map(in_working_dir_from_source),
            });
            return Err(e);
        }
        let _ = std::fs::remove_file(&backup_path);
        self.audit.record_primitive(PrimitiveAuditEntry {
            operation: AuditPrimitiveOp::Edit,
            path_or_cmd: path_str,
            plugin_id: plugin_id.to_string(),
            user_approved: true,
            success: true,
            detail: None,
            permission_level: level.map(permission_level_str),
            grant_source: source.map(grant_source_str),
            in_working_dir: source.map(in_working_dir_from_source),
        });
        Ok(EditFileResult {
            path: path.to_string(),
            applied: true,
        })
    }

    async fn execute_bash(
        &self,
        command: &str,
        cwd: Option<&str>,
        plugin_id: &str,
        argv: Option<&[String]>,
    ) -> Result<BashResult, AppError> {
        let cwd_path = if let Some(c) = cwd {
            if self.gate.is_some() {
                let (p, _l, _s) = self
                    .gate_check_path(PrimitiveOperation::Read, c, plugin_id)
                    .await?;
                p
            } else {
                self.check_path(c)?
            }
        } else {
            PathBuf::from(".")
        };

        let audit_cmd = match argv {
            None => command.to_string(),
            Some(args) => {
                let mut s = command.to_string();
                for a in args {
                    s.push(' ');
                    s.push_str(a);
                }
                s
            }
        };

        // 用于审计成功路径的 bash 决策来源（whitelist / approval）。
        let mut bash_level: Option<PermissionLevel> = None;
        let mut bash_source: Option<GrantSource> = None;

        if self.gate.is_some() {
            // gate 模式：先 bash 三档 regex 决策（NeedConfirm 弹 confirm）。
            match self.gate_check_bash(&audit_cmd, plugin_id).await {
                Ok((l, s)) => {
                    bash_level = Some(l);
                    bash_source = Some(s);
                }
                Err(e) => {
                    self.audit.record_primitive(PrimitiveAuditEntry {
                        operation: AuditPrimitiveOp::Bash,
                        path_or_cmd: audit_cmd.clone(),
                        plugin_id: plugin_id.to_string(),
                        user_approved: false,
                        success: false,
                        detail: Some(e.to_string()),
                        ..Default::default()
                    });
                    return Err(e);
                }
            }

            // 然后把命令里出现的路径逐一交给 gate.check 处理（layer-1 deny / layer-2 confirm）。
            // 仅作"尽力而为"——shell_words 解析失败的命令，依赖 forbidden regex 兜底。
            for raw in crate::core::permission::bash_parser::extract_paths(&audit_cmd) {
                if let Err(e) = self
                    .gate_check_path(PrimitiveOperation::Bash, &raw, plugin_id)
                    .await
                {
                    self.audit.record_primitive(PrimitiveAuditEntry {
                        operation: AuditPrimitiveOp::Bash,
                        path_or_cmd: audit_cmd.clone(),
                        plugin_id: plugin_id.to_string(),
                        user_approved: false,
                        success: false,
                        detail: Some(format!("路径未授权: {} ({})", raw, e)),
                        ..Default::default()
                    });
                    return Err(e);
                }
            }
        } else {
            // legacy：第一段子串匹配 + require_approval_for_all_bash 流程。
            let (first_token, check_full_cmd): (&str, &str) = match argv {
                None => {
                    let ft = command.split_whitespace().next().unwrap_or("");
                    (ft, command)
                }
                Some(_) => (command, command),
            };

            let in_whitelist = self.config.bash_whitelist.iter().any(|c| {
                c == first_token || check_full_cmd.starts_with(c) || audit_cmd.starts_with(c)
            });
            let in_forbidden = self.config.bash_forbidden.iter().any(|c| {
                c == first_token || check_full_cmd.starts_with(c) || audit_cmd.starts_with(c)
            });
            let needs_approval = self.config.bash_approval_required.iter().any(|c| {
                c == first_token || check_full_cmd.starts_with(c) || audit_cmd.starts_with(c)
            });

            if in_forbidden {
                self.audit.record_primitive(PrimitiveAuditEntry {
                    operation: AuditPrimitiveOp::Bash,
                    path_or_cmd: audit_cmd.clone(),
                    plugin_id: plugin_id.to_string(),
                    user_approved: false,
                    success: false,
                    detail: Some("命令在禁止列表中".to_string()),
                    ..Default::default()
                });
                return Err(AppError::Permission("命令在禁止列表中".to_string()));
            }
            let need_confirm =
                (!in_whitelist && !self.config.bash_whitelist.is_empty()) || needs_approval;
            if need_confirm && self.needs_confirmation(PrimitiveOperation::Bash) {
                let ok = self
                    .require_user_confirmation(
                        PrimitiveOperation::Bash,
                        &format!("执行: {}", audit_cmd),
                        plugin_id,
                    )
                    .await?;
                if !ok {
                    self.audit.record_primitive(PrimitiveAuditEntry {
                        operation: AuditPrimitiveOp::Bash,
                        path_or_cmd: audit_cmd.clone(),
                        plugin_id: plugin_id.to_string(),
                        user_approved: false,
                        success: false,
                        detail: Some("用户拒绝确认".to_string()),
                        ..Default::default()
                    });
                    return Err(AppError::Permission("用户拒绝 bash 确认".to_string()));
                }
            }
        }

        let output = match argv {
            None => {
                #[cfg(unix)]
                let script = {
                    let env_path = self
                        .config
                        .wasmedge_env_path
                        .as_deref()
                        .unwrap_or(r#"$HOME/.wasmedge/env"#);
                    format!(r#"[ -f "{0}" ] && . "{0}"; {1}"#, env_path, command)
                };
                #[cfg(windows)]
                let script = command.to_string();
                #[cfg(unix)]
                let (shell, shell_arg) = ("sh", "-c");
                #[cfg(windows)]
                let (shell, shell_arg) = ("cmd", "/C");
                Command::new(shell)
                    .arg(shell_arg)
                    .arg(&script)
                    .current_dir(&cwd_path)
                    .kill_on_drop(true)
                    .output()
                    .await
            }
            Some(args) => {
                let mut cmd = Command::new(command);
                cmd.args(args)
                    .current_dir(&cwd_path)
                    .kill_on_drop(true)
                    .output()
                    .await
            }
        }
        .map_err(|e| AppError::Primitive(e.to_string()))?;

        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let exit_code = output.status.code().unwrap_or(-1);

        self.audit.record_primitive(PrimitiveAuditEntry {
            operation: AuditPrimitiveOp::Bash,
            path_or_cmd: audit_cmd,
            plugin_id: plugin_id.to_string(),
            user_approved: true,
            success: exit_code == 0,
            detail: Some(format!(
                "exit_code={} stdout_len={} stderr_len={}",
                exit_code,
                stdout.len(),
                stderr.len()
            )),
            permission_level: bash_level.map(permission_level_str),
            grant_source: bash_source.map(grant_source_str),
            in_working_dir: bash_source.map(in_working_dir_from_source),
        });
        Ok(BashResult {
            stdout,
            stderr,
            exit_code,
        })
    }

    async fn require_user_confirmation(
        &self,
        operation: PrimitiveOperation,
        preview: &str,
        plugin_id: &str,
    ) -> Result<bool, AppError> {
        if !self.needs_confirmation(operation) {
            return Ok(true);
        }
        self.confirmation
            .confirm(operation, preview, plugin_id)
            .await
    }
}
