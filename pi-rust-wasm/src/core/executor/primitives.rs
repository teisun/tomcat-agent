use crate::core::confirmation::UserConfirmationProvider;
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

/// 4 原语执行引擎默认实现：路径白名单、用户确认、备份、原子化与审计。
pub struct DefaultPrimitiveExecutor {
    config: PrimitiveConfig,
    confirmation: Arc<dyn UserConfirmationProvider>,
    audit: Arc<dyn AuditRecorder>,
    /// 默认工作目录：path_whitelist 为空时作为隐式白名单根目录。
    workspace_dir: PathBuf,
    /// `pi.config.toml` 中 `[workspace] extra_roots` 的额外授权根路径，与 `workspace_dir` 并集（全局，所有 agent 共用）。
    extra_roots: Vec<PathBuf>,
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
        }
    }

    /// 设置配置中解析得到的额外授权根路径（与 `pi workspace` / TOML 同源）。
    pub fn with_extra_roots(mut self, roots: Vec<PathBuf>) -> Self {
        self.extra_roots = roots;
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
        for b in &self.config.path_blacklist {
            if normalized_starts_with(&s, b) {
                return Err(AppError::Permission(format!("路径在黑名单内: {}", path)));
            }
        }
        Ok(normalized)
    }

    /// 是否需要对该操作进行用户确认（根据 config）。
    fn needs_confirmation(&self, op: PrimitiveOperation) -> bool {
        match op {
            PrimitiveOperation::Read => false,
            PrimitiveOperation::Write => {
                if self.config.auto_confirm {
                    return false;
                }
                self.config.require_approval_for_all_write
            }
            PrimitiveOperation::Edit => {
                if self.config.auto_confirm {
                    return false;
                }
                true
            }
            PrimitiveOperation::Bash => {
                if self.config.auto_confirm {
                    return false;
                }
                self.config.require_approval_for_all_bash
            }
        }
    }
}

fn normalized_starts_with(path: &str, prefix: &str) -> bool {
    let path = path.trim_end_matches(std::path::MAIN_SEPARATOR);
    let prefix = prefix.trim_end_matches(std::path::MAIN_SEPARATOR);
    path == prefix || path.starts_with(&format!("{}{}", prefix, std::path::MAIN_SEPARATOR))
}

#[async_trait]
impl PrimitiveExecutor for DefaultPrimitiveExecutor {
    async fn read_file(&self, path: &str, plugin_id: &str) -> Result<String, AppError> {
        let path_buf = self.check_path(path)?;
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
        let content = read_file_utf8(&path_buf)?;
        self.audit.record_primitive(PrimitiveAuditEntry {
            operation: AuditPrimitiveOp::Read,
            path_or_cmd: path.to_string(),
            plugin_id: plugin_id.to_string(),
            user_approved: true,
            success: true,
            detail: None,
        });
        Ok(content)
    }

    async fn list_dir(&self, path: &str, plugin_id: &str) -> Result<Vec<DirEntry>, AppError> {
        let path_buf = self.check_path(path)?;
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
        let path_buf = self.check_path(path)?;
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
        if self.needs_confirmation(PrimitiveOperation::Write) {
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
                });
                return Err(AppError::Permission("用户拒绝写入确认".to_string()));
            }
        }

        write_file_atomic(&path_buf, content.as_bytes())?;
        self.audit.record_primitive(PrimitiveAuditEntry {
            operation: AuditPrimitiveOp::Write,
            path_or_cmd: path_str,
            plugin_id: plugin_id.to_string(),
            user_approved: true,
            success: true,
            detail: None,
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
        let path_buf = self.check_path(path)?;
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
        if self.needs_confirmation(PrimitiveOperation::Edit) {
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
        let cwd_path = cwd
            .map(|c| self.check_path(c))
            .transpose()?
            .unwrap_or_else(|| PathBuf::from("."));

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

        let (first_token, check_full_cmd): (&str, &str) = match argv {
            None => {
                let ft = command.split_whitespace().next().unwrap_or("");
                (ft, command)
            }
            Some(_) => (command, command),
        };

        let in_whitelist =
            self.config.bash_whitelist.iter().any(|c| {
                c == first_token || check_full_cmd.starts_with(c) || audit_cmd.starts_with(c)
            });
        let in_forbidden =
            self.config.bash_forbidden.iter().any(|c| {
                c == first_token || check_full_cmd.starts_with(c) || audit_cmd.starts_with(c)
            });
        let needs_approval =
            self.config.bash_approval_required.iter().any(|c| {
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
                });
                return Err(AppError::Permission("用户拒绝 bash 确认".to_string()));
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
