//! # 4 原语执行引擎默认实现
//!
//! 路径白名单、用户确认、备份、原子写入与审计；与 design CODE_BLOCK_P1_006 一致。

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
}

impl DefaultPrimitiveExecutor {
    pub fn new(
        config: PrimitiveConfig,
        confirmation: Arc<dyn UserConfirmationProvider>,
        audit: Arc<dyn AuditRecorder>,
    ) -> Self {
        Self {
            config,
            confirmation,
            audit,
        }
    }

    /// 路径白名单/黑名单校验；通过则返回规范化后的 PathBuf。
    fn check_path(&self, path: &str) -> Result<PathBuf, AppError> {
        let normalized = normalize_path(path)?;
        let s = normalized.to_string_lossy();
        let allowed = self
            .config
            .path_whitelist
            .iter()
            .any(|w| normalized_starts_with(&s, w));
        if !allowed {
            return Err(AppError::Permission(format!(
                "路径不在白名单内: {}",
                path
            )));
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
            return Err(AppError::Primitive("路径是目录，无法读取为文件".to_string()));
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
            let ok = self
                .require_user_confirmation(PrimitiveOperation::Write, &preview, plugin_id)
                .await?;
            if !ok {
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
                    let start = edit.start_line.unwrap_or(1) as usize;
                    let end = edit.end_line.unwrap_or(start as u64) as usize;
                    if start < 1 || end > lines.len() || start > end {
                        let _ = std::fs::copy(&backup_path, &path_buf);
                        return Err(AppError::Primitive(format!(
                            "Replace 行号无效: {}..{}",
                            start, end
                        )));
                    }
                    let idx = start - 1;
                    let new_lines: Vec<String> = edit.new_content.lines().map(String::from).collect();
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
    ) -> Result<BashResult, AppError> {
        let cwd_path = cwd
            .map(|c| self.check_path(c))
            .transpose()?
            .unwrap_or_else(|| PathBuf::from("."));
        let first_token = command.split_whitespace().next().unwrap_or("");
        let in_whitelist = self.config.bash_whitelist.iter().any(|c| c == first_token || command.starts_with(c));
        let in_forbidden = self
            .config
            .bash_forbidden
            .iter()
            .any(|c| c == first_token || command.starts_with(c));
        let needs_approval = self
            .config
            .bash_approval_required
            .iter()
            .any(|c| c == first_token || command.starts_with(c));

        if in_forbidden {
            self.audit.record_primitive(PrimitiveAuditEntry {
                operation: AuditPrimitiveOp::Bash,
                path_or_cmd: command.to_string(),
                plugin_id: plugin_id.to_string(),
                user_approved: false,
                success: false,
                detail: Some("命令在禁止列表中".to_string()),
            });
            return Err(AppError::Permission("命令在禁止列表中".to_string()));
        }
        let need_confirm = (!in_whitelist && !self.config.bash_whitelist.is_empty())
            || needs_approval;
        if need_confirm && self.needs_confirmation(PrimitiveOperation::Bash) {
            let ok = self
                .require_user_confirmation(
                    PrimitiveOperation::Bash,
                    &format!("执行: {}", command),
                    plugin_id,
                )
                .await?;
            if !ok {
                self.audit.record_primitive(PrimitiveAuditEntry {
                    operation: AuditPrimitiveOp::Bash,
                    path_or_cmd: command.to_string(),
                    plugin_id: plugin_id.to_string(),
                    user_approved: false,
                    success: false,
                    detail: Some("用户拒绝确认".to_string()),
                });
                return Err(AppError::Permission("用户拒绝 bash 确认".to_string()));
            }
        }

        #[cfg(unix)]
        let (shell, arg) = ("sh", "-c");
        #[cfg(windows)]
        let (shell, arg) = ("cmd", "/C");

        let output = Command::new(shell)
            .arg(arg)
            .arg(command)
            .current_dir(&cwd_path)
            .kill_on_drop(true)
            .output()
            .await
            .map_err(|e| AppError::Primitive(e.to_string()))?;

        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let exit_code = output.status.code().unwrap_or(-1);

        self.audit.record_primitive(PrimitiveAuditEntry {
            operation: AuditPrimitiveOp::Bash,
            path_or_cmd: command.to_string(),
            plugin_id: plugin_id.to_string(),
            user_approved: true,
            success: exit_code == 0,
            detail: Some(format!("exit_code={} stdout_len={} stderr_len={}", exit_code, stdout.len(), stderr.len())),
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

fn build_simple_diff(old: &str, new: &str) -> String {
    let o: Vec<&str> = old.lines().collect();
    let n: Vec<&str> = new.lines().collect();
    let mut out = String::new();
    for (i, (a, b)) in o.iter().zip(n.iter()).enumerate() {
        if a != b {
            out.push_str(&format!("  {} -{}\n  {} +{}\n", i + 1, a, i + 1, b));
        }
    }
    if o.len() != n.len() {
        out.push_str(&format!("  ... ({} -> {} lines)\n", o.len(), n.len()));
    }
    if out.is_empty() {
        out = "(无变化)".to_string();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{AllowAllConfirmation, DenyAllConfirmation};
    use crate::infra::{AuditRecorder, PrimitiveAuditEntry, ToolAuditEntry, TracingAuditRecorder};
    use std::sync::Mutex;

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
        let dir = std::env::temp_dir().join("pi_awsm_exec_read");
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
        );
        let r = exec.read_file("/tmp/any", "p1").await;
        assert!(r.is_err());
        assert!(matches!(r.unwrap_err(), AppError::Permission(_)));
    }

    #[tokio::test]
    async fn list_dir_success() {
        let dir = std::env::temp_dir().join("pi_awsm_exec_list");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("f.txt"), "").unwrap();
        let dir = dir.canonicalize().unwrap();
        let config = temp_whitelist_config(&dir);
        let exec = DefaultPrimitiveExecutor::new(
            config,
            Arc::new(AllowAllConfirmation),
            Arc::new(TracingAuditRecorder),
        );
        let path_str = dir.to_string_lossy().to_string();
        let entries = exec.list_dir(&path_str, "p1").await.unwrap();
        assert!(!entries.is_empty());
        let _ = std::fs::remove_file(dir.join("f.txt"));
        let _ = std::fs::remove_dir(&dir);
    }

    #[tokio::test]
    async fn write_file_success() {
        let dir = std::env::temp_dir().join("pi_awsm_exec_write");
        std::fs::create_dir_all(&dir).unwrap();
        let dir = dir.canonicalize().unwrap();
        let f = dir.join("w.txt");
        let path_str = f.to_string_lossy().to_string();
        let config = temp_whitelist_config(&dir);
        let exec = DefaultPrimitiveExecutor::new(
            config,
            Arc::new(AllowAllConfirmation),
            Arc::new(TracingAuditRecorder),
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
        let dir = std::env::temp_dir().join("pi_awsm_exec_deny");
        std::fs::create_dir_all(&dir).unwrap();
        let dir = dir.canonicalize().unwrap();
        let f = dir.join("d.txt");
        std::fs::write(&f, "old").unwrap();
        let path_str = f.to_string_lossy().to_string();
        let mut c = temp_whitelist_config(&dir);
        c.require_approval_for_all_write = true;
        let audit_entries: Arc<Mutex<Vec<PrimitiveAuditEntry>>> = Arc::new(Mutex::new(Vec::new()));
        let audit = Arc::new(DenyAuditRecorder(audit_entries.clone()));
        let exec = DefaultPrimitiveExecutor::new(
            c,
            Arc::new(DenyAllConfirmation),
            audit,
        );
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
    }

    #[tokio::test]
    async fn edit_file_success() {
        let dir = std::env::temp_dir().join("pi_awsm_exec_edit");
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
        let dir = std::env::temp_dir().join("pi_awsm_exec_bash");
        std::fs::create_dir_all(&dir).unwrap();
        let dir = dir.canonicalize().unwrap();
        let path_str = dir.to_string_lossy().to_string();
        let config = temp_whitelist_config(&dir);
        let exec = DefaultPrimitiveExecutor::new(
            config,
            Arc::new(AllowAllConfirmation),
            Arc::new(TracingAuditRecorder),
        );
        let res = exec
            .execute_bash("echo ok", Some(&path_str), "p1")
            .await
            .unwrap();
        assert_eq!(res.exit_code, 0);
        assert!(res.stdout.trim().contains("ok"));
        let _ = std::fs::remove_dir(&dir);
    }

    #[tokio::test]
    async fn execute_bash_forbidden() {
        let dir = std::env::temp_dir().join("pi_awsm_exec_forbid");
        std::fs::create_dir_all(&dir).unwrap();
        let dir = dir.canonicalize().unwrap();
        let path_str = dir.to_string_lossy().to_string();
        let mut c = temp_whitelist_config(&dir);
        c.bash_forbidden = vec!["rm".to_string()];
        let exec = DefaultPrimitiveExecutor::new(
            c,
            Arc::new(AllowAllConfirmation),
            Arc::new(TracingAuditRecorder),
        );
        let r = exec.execute_bash("rm -rf /", Some(&path_str), "p1").await;
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
        );
        let ok = exec
            .require_user_confirmation(PrimitiveOperation::Write, "preview", "p1")
            .await
            .unwrap();
        assert!(!ok);
    }
}
