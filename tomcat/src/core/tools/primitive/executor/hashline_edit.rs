//! # `hashline_edit` 工具实现（T2-P0-017 Phase3 / PR-M）
//!
//! 与 [`super::read::compute_line_hash`] 算法一致；行级强一致编辑。
//!
//! ## 协议
//!
//! ```jsonc
//! {
//!   "path": "src/foo.rs",
//!   "edits": [
//!     { "op": "replace", "pos": "42#Ab",            "lines": "new line\n" },
//!     { "op": "replace", "pos": "55#Cd", "end": "57#Ef", "lines": "x\ny\n" },
//!     { "op": "insert",  "pos": "10#Gh",            "lines": "header\n" },
//!     { "op": "delete",  "pos": "20#Ij", "end": "21#Kl" }
//!   ]
//! }
//! ```
//!
//! ## 校验语义
//!
//! 1. `gate_check_path(Edit)`；
//! 2. `read_file_utf8` → 当前内容（**不**做 normalize，行哈希必须按磁盘字节算）；
//! 3. 解析每条 `pos` / `end` 为 `(line_no, expected_hash)`；
//!    - 行号越界 → `OutOfRange`；
//!    - 实际行哈希 ≠ 期望 → `HashMismatch`（语义同 read 侧 stale）；
//!    - 一趟收集全部无效锚点，返回 `HashlineValidationFailed`，不给模型「修一个再撞一个」；
//! 4. 收集所有 `(start_line, end_line, replacement)` 区间，按行号检查重叠；
//! 5. 自下而上 splice → `new_content` → `write_file_atomic`；写失败回滚 `.bak`。

use super::helpers::{grant_trigger_str, grant_type_str, permission_scope_str, url_like_fs_miss};
use super::read::compute_line_hash;
use super::DefaultPrimitiveExecutor;
use crate::core::tools::primitive::diff::{build_line_diff, line_diff_stat, LineDiff};
use crate::core::tools::primitive::{
    EditFileResult, HashlineOp, HashlineSegment, PrimitiveOperation,
};
use crate::infra::audit::{AuditPrimitiveOp, PrimitiveAuditEntry};
use crate::infra::error::AppError;
use crate::infra::platform::{read_file_utf8, write_file_atomic};
use tokio_util::sync::CancellationToken;

enum HashlineEditOutcome {
    Applied {
        added: u32,
        removed: u32,
        diff: LineDiff,
    },
    Cancelled,
}

#[derive(Debug)]
enum HashlineAnchorValidationKind {
    OutOfRange {
        line_no: u64,
        total_lines: u64,
    },
    Mismatch {
        line_no: u64,
        expected_hash: String,
        actual_hash: String,
        line_excerpt: String,
    },
}

#[derive(Debug)]
struct HashlineAnchorValidationError {
    segment_index: usize,
    field: &'static str,
    kind: HashlineAnchorValidationKind,
}

impl HashlineAnchorValidationError {
    fn render(&self) -> String {
        match &self.kind {
            HashlineAnchorValidationKind::OutOfRange {
                line_no,
                total_lines,
            } => format!(
                "- edits[{}].{}: OutOfRange: 锚点行号 {} 超过文件总行数 {}",
                self.segment_index, self.field, line_no, total_lines
            ),
            HashlineAnchorValidationKind::Mismatch {
                line_no,
                expected_hash,
                actual_hash,
                line_excerpt,
            } => format!(
                "- edits[{}].{}: HashMismatch: 锚点 {}#{} 与当前文件第 {} 行哈希 {} 不一致；最新锚点 `{}#{}`；当前行摘要 {:?}",
                self.segment_index,
                self.field,
                line_no,
                expected_hash,
                line_no,
                actual_hash,
                line_no,
                actual_hash,
                line_excerpt,
            ),
        }
    }
}

/// 主执行入口（被 `DefaultPrimitiveExecutor` 的 trait 实现调用）。
/// `PrimitiveExecutor` trait，避免再次牵动 dispatcher / mock）。
pub async fn hashline_edit_impl(
    executor: &DefaultPrimitiveExecutor,
    path: &str,
    segments: Vec<HashlineSegment>,
    cancel: &CancellationToken,
    plugin_id: &str,
) -> Result<EditFileResult, AppError> {
    if let Some(err) = url_like_fs_miss(path) {
        return Err(err);
    }
    let Some(checked_path) = super::helpers::until_cancelled(
        cancel,
        executor.gate_check_path(PrimitiveOperation::Edit, path, plugin_id),
    )
    .await
    else {
        return Ok(EditFileResult {
            path: path.to_string(),
            applied: false,
            added: None,
            removed: None,
            diff: None,
            diff_truncated: false,
        });
    };
    let (path_buf, scope, grant) = checked_path?;
    let path_str = path_buf.to_string_lossy().to_string();
    let path_for_edit = path_buf.clone();
    let user_path = path.to_string();
    let segments_for_edit = segments.clone();
    let cancel_for_edit = cancel.clone();
    let outcome = tokio::task::spawn_blocking(move || -> Result<HashlineEditOutcome, AppError> {
        let original = read_file_utf8(&path_for_edit).map_err(|e| match e {
            AppError::Io(io) if io.kind() == std::io::ErrorKind::InvalidData => {
                AppError::Primitive(format!(
                    "BinaryFile: `{}` 不是 UTF-8 文本，hashline_edit 拒绝执行",
                    user_path
                ))
            }
            other => other,
        })?;
        let raw_lines: Vec<&str> = original.split_inclusive('\n').collect();
        let total_lines = raw_lines.len() as u64;

        let mut spans: Vec<(u64, u64, String)> = Vec::new();
        let mut validation_errors = Vec::new();
        for (segment_index, seg) in segments_for_edit.iter().enumerate() {
            let start_valid = validate_anchor(
                &mut validation_errors,
                segment_index,
                "pos",
                seg.start_line,
                &seg.start_hash,
                &raw_lines,
                total_lines,
            );
            let end_valid = if seg.end_line == seg.start_line {
                start_valid
            } else {
                validate_anchor(
                    &mut validation_errors,
                    segment_index,
                    "end",
                    seg.end_line,
                    &seg.end_hash,
                    &raw_lines,
                    total_lines,
                )
            };

            if start_valid && end_valid {
                let span = match seg.op {
                    HashlineOp::Replace => (seg.start_line, seg.end_line, seg.lines.clone()),
                    HashlineOp::Insert => (seg.start_line, seg.start_line - 1, seg.lines.clone()),
                    HashlineOp::Delete => (seg.start_line, seg.end_line, String::new()),
                };
                spans.push(span);
            }
        }
        if !validation_errors.is_empty() {
            let details = validation_errors
                .iter()
                .map(HashlineAnchorValidationError::render)
                .collect::<Vec<_>>()
                .join("\n");
            return Err(AppError::Primitive(format!(
                "HashlineValidationFailed: {} 个锚点无效；文件未修改。请重新 `read hashline=true` 获取全部涉及区段的最新锚点：\n{}",
                validation_errors.len(),
                details
            )));
        }
        spans.sort_by_key(|(s, _, _)| *s);
        for w in spans.windows(2) {
            let (_, e1, _) = &w[0];
            let (s2, _, _) = &w[1];
            if *s2 <= *e1 {
                return Err(AppError::Primitive(format!(
                    "Overlap: hashline_edit 段在行 [{}..{}] 与下一段起始行 {} 相交",
                    w[0].0, e1, s2
                )));
            }
        }

        let mut new_content = String::with_capacity(original.len());
        let mut offsets: Vec<usize> = Vec::with_capacity(raw_lines.len() + 1);
        let mut acc = 0usize;
        for line in &raw_lines {
            offsets.push(acc);
            acc += line.len();
        }
        offsets.push(acc);
        new_content.push_str(&original);
        let mut spans_desc = spans.clone();
        spans_desc.sort_by_key(|(s, _, _)| std::cmp::Reverse(*s));
        for (start_line, end_line, replacement) in spans_desc {
            let s_byte = offsets[(start_line - 1) as usize];
            let e_byte = if end_line >= start_line {
                offsets[end_line as usize]
            } else {
                s_byte
            };
            new_content.replace_range(s_byte..e_byte, &replacement);
        }
        let (added, removed) = line_diff_stat(&original, &new_content);
        let diff = build_line_diff(&original, &new_content);

        if cancel_for_edit.is_cancelled() {
            return Ok(HashlineEditOutcome::Cancelled);
        }
        let backup_path = path_for_edit.with_extension("bak");
        std::fs::copy(&path_for_edit, &backup_path).map_err(AppError::Io)?;
        if cancel_for_edit.is_cancelled() {
            let _ = std::fs::remove_file(&backup_path);
            return Ok(HashlineEditOutcome::Cancelled);
        }
        if let Err(e) = write_file_atomic(&path_for_edit, new_content.as_bytes()) {
            let _ = std::fs::copy(&backup_path, &path_for_edit);
            return Err(e);
        }
        let _ = std::fs::remove_file(&backup_path);
        Ok(HashlineEditOutcome::Applied {
            added,
            removed,
            diff,
        })
    })
    .await
    .map_err(|e| AppError::Primitive(format!("hashline_edit join error: {e}")))?;
    match outcome {
        Ok(HashlineEditOutcome::Cancelled) => {
            executor.audit.record_primitive(PrimitiveAuditEntry {
                operation: AuditPrimitiveOp::Edit,
                path_or_cmd: path_str.clone(),
                plugin_id: plugin_id.to_string(),
                user_approved: true,
                success: false,
                detail: Some("cancelled_before_write".to_string()),
                permission_scope: Some(permission_scope_str(scope)),
                grant_type: Some(grant_type_str(grant.grant_type)),
                grant_trigger: Some(grant_trigger_str(grant.trigger)),
            });
            Ok(EditFileResult {
                path: crate::infra::platform::format_home_path(&path_buf),
                applied: false,
                added: None,
                removed: None,
                diff: None,
                diff_truncated: false,
            })
        }
        Err(e) => {
            executor.audit.record_primitive(PrimitiveAuditEntry {
                operation: AuditPrimitiveOp::Edit,
                path_or_cmd: path_str.clone(),
                plugin_id: plugin_id.to_string(),
                user_approved: true,
                success: false,
                detail: Some(e.to_string()),
                permission_scope: Some(permission_scope_str(scope)),
                grant_type: Some(grant_type_str(grant.grant_type)),
                grant_trigger: Some(grant_trigger_str(grant.trigger)),
            });
            Err(e)
        }
        Ok(HashlineEditOutcome::Applied {
            added,
            removed,
            diff,
        }) => {
            executor.audit.record_primitive(PrimitiveAuditEntry {
                operation: AuditPrimitiveOp::Edit,
                path_or_cmd: path_str,
                plugin_id: plugin_id.to_string(),
                user_approved: true,
                success: true,
                detail: Some("hashline_edit".to_string()),
                permission_scope: Some(permission_scope_str(scope)),
                grant_type: Some(grant_type_str(grant.grant_type)),
                grant_trigger: Some(grant_trigger_str(grant.trigger)),
            });
            Ok(EditFileResult {
                path: crate::infra::platform::format_home_path(&path_buf),
                applied: true,
                added: Some(added),
                removed: Some(removed),
                diff: diff.lines,
                diff_truncated: diff.truncated,
            })
        }
    }
}

fn strip_trailing_newline(line: &str) -> &str {
    let line = line.strip_suffix('\n').unwrap_or(line);
    line.strip_suffix('\r').unwrap_or(line)
}

fn validate_anchor(
    validation_errors: &mut Vec<HashlineAnchorValidationError>,
    segment_index: usize,
    field: &'static str,
    line_no: u64,
    expected_hash: &str,
    raw_lines: &[&str],
    total_lines: u64,
) -> bool {
    if line_no == 0 || line_no > total_lines {
        validation_errors.push(HashlineAnchorValidationError {
            segment_index,
            field,
            kind: HashlineAnchorValidationKind::OutOfRange {
                line_no,
                total_lines,
            },
        });
        return false;
    }

    let current_line = strip_trailing_newline(raw_lines[(line_no - 1) as usize]);
    let actual_hash = compute_line_hash(current_line);
    if actual_hash == expected_hash {
        return true;
    }

    validation_errors.push(HashlineAnchorValidationError {
        segment_index,
        field,
        kind: HashlineAnchorValidationKind::Mismatch {
            line_no,
            expected_hash: expected_hash.to_string(),
            actual_hash,
            line_excerpt: line_excerpt(current_line),
        },
    });
    false
}

fn line_excerpt(line: &str) -> String {
    const MAX_CHARS: usize = 80;

    let mut chars = line.trim().chars();
    let excerpt: String = chars.by_ref().take(MAX_CHARS).collect();
    if excerpt.is_empty() {
        return "（空行）".to_string();
    }
    if chars.next().is_some() {
        format!("{excerpt}…")
    } else {
        excerpt
    }
}
