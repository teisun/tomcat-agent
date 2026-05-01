//! # 4+1 原语执行引擎（DefaultPrimitiveExecutor）
//!
//! 实现 [`crate::core::primitives::PrimitiveExecutor`] trait，是 Agent 与文件系统 / Shell
//! 的 **唯一受信通道**：任何 LLM 工具调用最终都要落到这 5 个方法上，安全策略
//! （`PermissionGate` 三层决策 / 用户确认 / 备份 / 原子写 / 审计）全部在此横切。
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
//!    │ ① 路径规范化（infra::platform::normalize_path）：~ 展开、symlink 还原、
//!    │   Windows 反斜杠归一。
//!    │
//!    │ ② 路径授权（统一走 PermissionGate）
//!    │   gate.check(op, normalized_path)
//!    │     ├─ Allow            ► 继续
//!    │     ├─ Deny             ► AppError::Permission(原因)
//!    │     └─ NeedConfirm      ► confirmation.confirm_decision(...)
//!    │                            ├─ AllowOnce → grant_session → 重试
//!    │                            ├─ AllowAndPersistRoot → grant_session → 重试
//!    │                            └─ Deny → AppError::Permission(用户拒绝)
//!    │
//!    │ ③ Bash 命令决策（execute_bash 专属）
//!    │   gate.check_bash(audit_cmd) 同样三态；命中后再用 `bash_parser::extract_paths`
//!    │   把命令里出现的路径逐一交给 gate.check(Bash) 做路径级预检
//!    │   （layer-1 deny / layer-2 confirm）。
//!    │
//!    │ ④ 业务执行（按方法分支）
//!    │   read_file    ► 大小预检（≤ MAX_READ_BYTES=10MB）► read_file_utf8
//!    │   list_dir     ► std::fs::read_dir → DirEntry 列表
//!    │   write_file   ► （可选）备份原文件 ► write_file_atomic（写临时 + rename）
//!    │   edit_file    ► load → apply EditOperation* → diff → 备份 ► atomic 写
//!    │   execute_bash ► tokio::process::Command spawn
//!    │
//!    │ ⑤ 审计落库（无论 Ok / Err）
//!    │   audit.record_primitive(PrimitiveAuditEntry {
//!    │     plugin_id, op: AuditPrimitiveOp::*, success, detail,
//!    │     permission_level, grant_type, grant_trigger, ...
//!    │   })
//!    ▼
//!   Result<T, AppError>
//! ```
//!
//! ## 横切配置
//!
//! - `PrimitiveConfig`（来自 [`crate::infra::PrimitiveConfig`]）：bash 禁止 / 审批
//!   regex 与备份 / `auto_confirm` 策略；路径规则已迁入 `PermissionGate`。
//! - `MAX_READ_BYTES = 10 MiB`：read_file 单次读上限，防 OOM。
//! - `BASH_TIMEOUT_SECS = 30`：bash 默认超时（当前预留，后续接 `tokio::time::timeout`）。
//! - `gate: Arc<dyn PermissionGate>`：构造期强制注入，承担路径 / bash / 审计来源
//!   的全部决策；不存在「未注入 gate」的执行路径。
//!
//! ## 与同族子模块的边界
//!
//! - `super::diff::build_simple_diff`：edit_file 的 diff 文本生成。
//! - `super::primitives` 与 `super::confirmation`：trait + 用户确认 trait。
//! - 调用方：`agent_loop::tool_exec::execute_tool` 是唯一直接调用方，所有
//!   LLM 工具调用都从那里 dispatch 进来。

use crate::core::confirmation::{ConfirmDecision, UserConfirmationProvider};
use crate::core::permission::{
    GrantTrace, GrantTrigger, GrantType, PermissionDecision, PermissionGate, PermissionLevel,
};
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

use super::diff::build_simple_diff;

/// 单次读取文件最大字节数，避免 OOM。
const MAX_READ_BYTES: u64 = 10 * 1024 * 1024; // 10 MiB
/// bash 执行默认超时（预留，后续可配合 tokio::time::timeout 使用）。
#[allow(dead_code)]
const BASH_TIMEOUT_SECS: u64 = 30;

/// 4 原语执行引擎默认实现：路径权限、用户确认、备份、原子化与审计。
///
/// **权限模型**：构造期强制注入 [`PermissionGate`]；路径 / bash / 审计来源
/// 全部走 gate 三层决策。无 legacy fallback 通道。
pub struct DefaultPrimitiveExecutor {
    config: PrimitiveConfig,
    confirmation: Arc<dyn UserConfirmationProvider>,
    audit: Arc<dyn AuditRecorder>,
    /// 路径与 bash 权限决策入口；由 [`crate::api::chat::ChatContext`] 注入并与
    /// `cwd_lazy_prompt` / `config_tool` 共享同一份 `SessionGrants` 视图。
    gate: Arc<dyn PermissionGate>,
}

impl DefaultPrimitiveExecutor {
    pub fn new(
        config: PrimitiveConfig,
        confirmation: Arc<dyn UserConfirmationProvider>,
        audit: Arc<dyn AuditRecorder>,
        gate: Arc<dyn PermissionGate>,
    ) -> Self {
        Self {
            config,
            confirmation,
            audit,
            gate,
        }
    }

    // ─────────────────────────────────────────────────────────────────────
    // PermissionGate 桥接（gate 始终存在）
    // ─────────────────────────────────────────────────────────────────────

    /// 经 gate 决定一个原语对路径的访问，必要时弹 confirm 完成 layer-2。
    ///
    /// 返回 `Ok((path_buf, level, grant))` 表示放行；
    /// `Err(AppError::Permission)` 表示被 gate 拒绝或用户拒绝 confirm。
    async fn gate_check_path(
        &self,
        op: PrimitiveOperation,
        path: &str,
        plugin_id: &str,
    ) -> Result<(PathBuf, PermissionLevel, GrantTrace), AppError> {
        let gate = &self.gate;
        let normalized = normalize_path(path)?;
        loop {
            let decision = gate.check(op, &normalized.to_string_lossy())?;
            match decision {
                PermissionDecision::Allow { grant, level } => {
                    return Ok((normalized, level, grant))
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
                            // 落 SessionGrants：AllowOnce 授权当前目标路径本身。
                            gate.grant_session(normalized.clone(), GrantTrigger::UserConfirm);
                            // 重新 check：现在应该 Allow。
                            continue;
                        }
                        ConfirmDecision::AllowAndPersistRoot { root } => {
                            // 1) 同时落 SessionGrants（本会话生效）。
                            gate.grant_session(root.clone(), GrantTrigger::UserConfirm);
                            // 2) 持久化由 caller（CLI confirm 实现）负责调用
                            //    `pi workspace add` 等价的 append_workspace_root_to_disk；
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
    ) -> Result<(PermissionLevel, GrantTrace), AppError> {
        let gate = &self.gate;
        let decision = gate.check_bash(command)?;
        match decision {
            PermissionDecision::Allow { grant, level } => Ok((level, grant)),
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
                        Ok((
                            PermissionLevel::BashApproval,
                            GrantTrace::new(GrantType::BashPolicy, GrantTrigger::UserConfirm),
                        ))
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
        PermissionLevel::Bash => "bash",
        PermissionLevel::BashApproval => "bash_approval",
        PermissionLevel::Forbidden => "forbidden",
    }
    .to_string()
}

/// 把 [`GrantType`] 序列化为审计字符串。
fn grant_type_str(s: GrantType) -> String {
    match s {
        GrantType::AgentDefinitionDir => "agent_definition_dir",
        GrantType::AgentWorkspaceRoot => "agent_workspace_root",
        GrantType::SessionScope => "session_scope",
        GrantType::PathRuleReadOnly => "path_rule_read_only",
        GrantType::AgentTrailDir => "agent_trail_dir",
        GrantType::BashPolicy => "bash_policy",
    }
    .to_string()
}

/// 把 [`GrantTrigger`] 序列化为审计字符串。
fn grant_trigger_str(s: GrantTrigger) -> String {
    match s {
        GrantTrigger::BuiltinDefault => "builtin_default",
        GrantTrigger::WorkspaceRootsConfig => "workspace_roots_config",
        GrantTrigger::PathRulesConfig => "path_rules_config",
        GrantTrigger::BashRegexConfig => "bash_regex_config",
        GrantTrigger::UserConfirm => "user_confirm",
        GrantTrigger::CwdLazyPrompt => "cwd_lazy_prompt",
        GrantTrigger::DraggedPathMenu => "dragged_path_menu",
        GrantTrigger::AutoConfirmFlag => "auto_confirm_flag",
    }
    .to_string()
}

#[async_trait]
impl PrimitiveExecutor for DefaultPrimitiveExecutor {
    async fn read_file(&self, path: &str, plugin_id: &str) -> Result<String, AppError> {
        let (path_buf, level, grant) = self
            .gate_check_path(PrimitiveOperation::Read, path, plugin_id)
            .await?;
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
            permission_level: Some(permission_level_str(level)),
            grant_type: Some(grant_type_str(grant.grant_type)),
            grant_trigger: Some(grant_trigger_str(grant.trigger)),
        });
        Ok(content)
    }

    async fn list_dir(&self, path: &str, plugin_id: &str) -> Result<Vec<DirEntry>, AppError> {
        let (path_buf, level, grant) = self
            .gate_check_path(PrimitiveOperation::Read, path, plugin_id)
            .await?;
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
            permission_level: Some(permission_level_str(level)),
            grant_type: Some(grant_type_str(grant.grant_type)),
            grant_trigger: Some(grant_trigger_str(grant.trigger)),
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
        let (path_buf, level, grant) = self
            .gate_check_path(PrimitiveOperation::Write, path, plugin_id)
            .await?;
        let path_str = path_buf.to_string_lossy().to_string();

        if overwrite && path_buf.exists() {
            let backup = path_buf.with_extension("bak");
            let _ = std::fs::copy(&path_buf, &backup);
        }

        write_file_atomic(&path_buf, content.as_bytes())?;
        self.audit.record_primitive(PrimitiveAuditEntry {
            operation: AuditPrimitiveOp::Write,
            path_or_cmd: path_str,
            plugin_id: plugin_id.to_string(),
            user_approved: true,
            success: true,
            detail: None,
            permission_level: Some(permission_level_str(level)),
            grant_type: Some(grant_type_str(grant.grant_type)),
            grant_trigger: Some(grant_trigger_str(grant.trigger)),
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
        let (path_buf, level, grant) = self
            .gate_check_path(PrimitiveOperation::Edit, path, plugin_id)
            .await?;
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
        let _ = build_simple_diff(content.as_str(), &new_content);

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
                permission_level: Some(permission_level_str(level)),
                grant_type: Some(grant_type_str(grant.grant_type)),
                grant_trigger: Some(grant_trigger_str(grant.trigger)),
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
            permission_level: Some(permission_level_str(level)),
            grant_type: Some(grant_type_str(grant.grant_type)),
            grant_trigger: Some(grant_trigger_str(grant.trigger)),
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
            let (p, _l, _s) = self
                .gate_check_path(PrimitiveOperation::Read, c, plugin_id)
                .await?;
            p
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

        // bash 决策来源（whitelist / approval）—— 走 gate 三层。
        let (bash_level, bash_grant) = match self.gate_check_bash(&audit_cmd, plugin_id).await {
            Ok((l, s)) => (l, s),
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
        };

        // 然后把命令里出现的路径逐一交给 gate.check 处理（layer-1 deny / layer-2 confirm）。
        // 仅作"尽力而为"——shell_words 解析失败的命令，依赖 forbidden regex 兜底。
        // 见 docs/TODOS.md `T-147`：动态路径访问的静态预检盲区由后续提示词注入防御方向覆盖。
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
            permission_level: Some(permission_level_str(bash_level)),
            grant_type: Some(grant_type_str(bash_grant.grant_type)),
            grant_trigger: Some(grant_trigger_str(bash_grant.trigger)),
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
        // 路径授权统一走 `gate_check_path` / `gate_check_bash`，
        // 此 trait 方法仅保留供外部直接调用 `confirmation.confirm` 的兼容入口：
        // - Read 操作不需要 confirm；
        // - `auto_confirm = true` 时直接放行；
        // - 其他情况转发给底层 `UserConfirmationProvider::confirm`。
        if matches!(operation, PrimitiveOperation::Read) || self.config.auto_confirm {
            return Ok(true);
        }
        self.confirmation
            .confirm(operation, preview, plugin_id)
            .await
    }
}
