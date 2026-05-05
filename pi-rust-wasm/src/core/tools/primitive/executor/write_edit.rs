//! # `write_file` / `edit_file` 实现
//!
//! 写路径共享 backup → atomic write → 失败回滚 → 审计落库 流程；`edit_file`
//! 在写之前先把整文件按行加载到内存，应用一组 [`EditOperation`] 后整体重写。
//! diff 文本由 [`super::super::diff::build_simple_diff`] 生成（当前为
//! 副作用：日志 / 审计后续可消费，调用结果未消费即可丢弃）。

use super::helpers::{grant_trigger_str, grant_type_str, permission_scope_str};
use super::DefaultPrimitiveExecutor;
use crate::core::tools::primitive::diff::build_simple_diff;
use crate::core::tools::primitive::{
    EditFileResult, EditOperation, EditOperationType, PrimitiveOperation, WriteFileResult,
};
use crate::infra::audit::{AuditPrimitiveOp, PrimitiveAuditEntry};
use crate::infra::error::AppError;
use crate::infra::platform::{read_file_utf8, write_file_atomic};

pub(super) async fn write_file_impl(
    executor: &DefaultPrimitiveExecutor,
    path: &str,
    content: &str,
    overwrite: bool,
    plugin_id: &str,
) -> Result<WriteFileResult, AppError> {
    let (path_buf, scope, grant) = executor
        .gate_check_path(PrimitiveOperation::Write, path, plugin_id)
        .await?;
    let path_str = path_buf.to_string_lossy().to_string();

    if overwrite && path_buf.exists() {
        let backup = path_buf.with_extension("bak");
        let _ = std::fs::copy(&path_buf, &backup);
    }

    write_file_atomic(&path_buf, content.as_bytes())?;
    executor.audit.record_primitive(PrimitiveAuditEntry {
        operation: AuditPrimitiveOp::Write,
        path_or_cmd: path_str,
        plugin_id: plugin_id.to_string(),
        user_approved: true,
        success: true,
        detail: None,
        permission_scope: Some(permission_scope_str(scope)),
        grant_type: Some(grant_type_str(grant.grant_type)),
        grant_trigger: Some(grant_trigger_str(grant.trigger)),
    });
    Ok(WriteFileResult {
        path: path.to_string(),
        written: true,
    })
}

pub(super) async fn edit_file_impl(
    executor: &DefaultPrimitiveExecutor,
    path: &str,
    edits: Vec<EditOperation>,
    plugin_id: &str,
) -> Result<EditFileResult, AppError> {
    let (path_buf, scope, grant) = executor
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
        executor.audit.record_primitive(PrimitiveAuditEntry {
            operation: AuditPrimitiveOp::Edit,
            path_or_cmd: path_str,
            plugin_id: plugin_id.to_string(),
            user_approved: true,
            success: false,
            detail: Some(e.to_string()),
            permission_scope: Some(permission_scope_str(scope)),
            grant_type: Some(grant_type_str(grant.grant_type)),
            grant_trigger: Some(grant_trigger_str(grant.trigger)),
        });
        return Err(e);
    }
    let _ = std::fs::remove_file(&backup_path);
    executor.audit.record_primitive(PrimitiveAuditEntry {
        operation: AuditPrimitiveOp::Edit,
        path_or_cmd: path_str,
        plugin_id: plugin_id.to_string(),
        user_approved: true,
        success: true,
        detail: None,
        permission_scope: Some(permission_scope_str(scope)),
        grant_type: Some(grant_type_str(grant.grant_type)),
        grant_trigger: Some(grant_trigger_str(grant.trigger)),
    });
    Ok(EditFileResult {
        path: path.to_string(),
        applied: true,
    })
}
