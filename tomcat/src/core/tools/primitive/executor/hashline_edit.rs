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
//! 4. 收集所有 `(start_line, end_line, replacement)` 区间，按行号检查重叠；
//! 5. 自下而上 splice → `new_content` → `write_file_atomic`；写失败回滚 `.bak`。

use super::helpers::{grant_trigger_str, grant_type_str, permission_scope_str, url_like_fs_miss};
use super::read::compute_line_hash;
use super::DefaultPrimitiveExecutor;
use crate::core::tools::primitive::{
    EditFileResult, HashlineOp, HashlineSegment, PrimitiveOperation,
};
use crate::infra::audit::{AuditPrimitiveOp, PrimitiveAuditEntry};
use crate::infra::error::AppError;
use crate::infra::platform::{read_file_utf8, write_file_atomic};

/// 主执行入口（被 `DefaultPrimitiveExecutor` 的 trait 实现调用）。
/// `PrimitiveExecutor` trait，避免再次牵动 dispatcher / mock）。
pub async fn hashline_edit_impl(
    executor: &DefaultPrimitiveExecutor,
    path: &str,
    segments: Vec<HashlineSegment>,
    plugin_id: &str,
) -> Result<EditFileResult, AppError> {
    if let Some(err) = url_like_fs_miss(path) {
        return Err(err);
    }
    let (path_buf, scope, grant) = executor
        .gate_check_path(PrimitiveOperation::Edit, path, plugin_id)
        .await?;
    let path_str = path_buf.to_string_lossy().to_string();
    let original = read_file_utf8(&path_buf).map_err(|e| match e {
        AppError::Io(io) if io.kind() == std::io::ErrorKind::InvalidData => {
            AppError::Primitive(format!(
                "BinaryFile: `{}` 不是 UTF-8 文本，hashline_edit 拒绝执行",
                path
            ))
        }
        other => other,
    })?;
    // 切行（保留尾换行风格：`split_inclusive('\n')` 与 read 侧一致）。
    let raw_lines: Vec<&str> = original.split_inclusive('\n').collect();
    let total_lines = raw_lines.len() as u64;

    // 校验所有锚点 + 收集 (start_idx, end_idx_inclusive_exclusive_byte_range)。
    // 我们用「按行字节区间」统一表示；splice 时一次性按降序替换。
    let mut spans: Vec<(u64, u64, String)> = Vec::new(); // (start_line_1b, end_line_1b_inclusive, replacement)
    for seg in &segments {
        if seg.start_line > total_lines {
            return Err(AppError::Primitive(format!(
                "OutOfRange: hashline_edit 锚点行号 {} 超过文件总行数 {}",
                seg.start_line, total_lines
            )));
        }
        let actual_start_line = strip_trailing_newline(raw_lines[(seg.start_line - 1) as usize]);
        let actual_start_hash = compute_line_hash(actual_start_line, seg.start_line);
        if actual_start_hash != seg.start_hash {
            return Err(AppError::Primitive(format!(
                "HashMismatch: 锚点 {}#{} 与当前文件第 {} 行哈希 {} 不一致；请重新 `read hashline=true` 拿到最新锚点",
                seg.start_line, seg.start_hash, seg.start_line, actual_start_hash
            )));
        }
        if seg.end_line != seg.start_line {
            if seg.end_line > total_lines {
                return Err(AppError::Primitive(format!(
                    "OutOfRange: hashline_edit end 行号 {} 超过文件总行数 {}",
                    seg.end_line, total_lines
                )));
            }
            let actual_end = strip_trailing_newline(raw_lines[(seg.end_line - 1) as usize]);
            let actual_end_hash = compute_line_hash(actual_end, seg.end_line);
            if actual_end_hash != seg.end_hash {
                return Err(AppError::Primitive(format!(
                    "HashMismatch: end 锚点 {}#{} 与当前文件第 {} 行哈希 {} 不一致",
                    seg.end_line, seg.end_hash, seg.end_line, actual_end_hash
                )));
            }
        }
        let span = match seg.op {
            HashlineOp::Replace => (seg.start_line, seg.end_line, seg.lines.clone()),
            HashlineOp::Insert => (seg.start_line, seg.start_line - 1, seg.lines.clone()),
            HashlineOp::Delete => (seg.start_line, seg.end_line, String::new()),
        };
        spans.push(span);
    }
    // 重叠检测（按 start 排序后比邻；end 用 inclusive，所以 next.start <= cur.end + 1 时算邻接）。
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

    // 自下而上 splice：按行号降序处理，按每行字节区间替换。
    let mut new_content = String::with_capacity(original.len());
    // 行起始字节偏移表（offset[i] = 第 i+1 行的起始字节；offset[total] = 文件末尾字节）。
    let mut offsets: Vec<usize> = Vec::with_capacity(raw_lines.len() + 1);
    let mut acc = 0usize;
    for line in &raw_lines {
        offsets.push(acc);
        acc += line.len();
    }
    offsets.push(acc);
    new_content.push_str(&original);
    // 按 start 降序应用
    let mut spans_desc = spans.clone();
    spans_desc.sort_by_key(|(s, _, _)| std::cmp::Reverse(*s));
    for (start_line, end_line, replacement) in spans_desc {
        // Insert：end_line == start_line - 1，替换字节区间为 `[offsets[start-1] .. offsets[start-1])`，零长。
        let s_byte = offsets[(start_line - 1) as usize];
        let e_byte = if end_line >= start_line {
            offsets[end_line as usize]
        } else {
            s_byte
        };
        new_content.replace_range(s_byte..e_byte, &replacement);
    }

    // 写盘：复用 .bak 兜底。
    let backup_path = path_buf.with_extension("bak");
    if let Err(e) = std::fs::copy(&path_buf, &backup_path) {
        executor.audit.record_primitive(PrimitiveAuditEntry {
            operation: AuditPrimitiveOp::Edit,
            path_or_cmd: path_str,
            plugin_id: plugin_id.to_string(),
            user_approved: true,
            success: false,
            detail: Some(format!(".bak copy failed: {}", e)),
            permission_scope: Some(permission_scope_str(scope)),
            grant_type: Some(grant_type_str(grant.grant_type)),
            grant_trigger: Some(grant_trigger_str(grant.trigger)),
        });
        return Err(AppError::Io(e));
    }
    if let Err(e) = write_file_atomic(&path_buf, new_content.as_bytes()) {
        let _ = std::fs::copy(&backup_path, &path_buf);
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
        detail: Some("hashline_edit".to_string()),
        permission_scope: Some(permission_scope_str(scope)),
        grant_type: Some(grant_type_str(grant.grant_type)),
        grant_trigger: Some(grant_trigger_str(grant.trigger)),
    });
    Ok(EditFileResult {
        path: crate::infra::platform::format_home_path(&path_buf),
        applied: true,
    })
}

fn strip_trailing_newline(line: &str) -> &str {
    let line = line.strip_suffix('\n').unwrap_or(line);
    line.strip_suffix('\r').unwrap_or(line)
}
