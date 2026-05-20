//! # `write_file` / `edit_file` 实现
//!
//! 写路径共享 backup → atomic write → 失败回滚 → 审计落库 流程。
//!
//! ## `edit_file`：T2-P0-017 PR-D 重写（2026-05-05）
//!
//! 替换早期「逐段 `replacen` 链式应用」实现，对齐
//! [docs/architecture/tools/edit.md](../../../../../docs/architecture/tools/edit.md) §2.4.1：
//!
//! 1. **原文快照**：读入磁盘 `original: String` 后，**所有**段的匹配 / 重叠 / 计数都在
//!    `original` 上完成（**字节索引** `match_indices`），禁止链式增量。
//! 2. **`replace_all` 信号**：由 `tool_exec::EDIT_REPLACE_ALL_MARKER` 编码到段的
//!    `old_content` 前缀；本模块在分段解析时识别并剥离（保留 trait 方法签名不动）。
//! 3. **错误集合（本期必落）**：`NotFound` / `Ambiguous` / `Overlap` / `BinaryFile` /
//!    `Io`；`Stale` 由 `tool_exec::check_edit_staleness` 在调 primitive 之前拦截。
//! 4. **`.bak` 写序**：校验阶段**不**触碰磁盘；全部校验通过后 → `copy(path → path.bak)`
//!    → `write_file_atomic`；写成功 `remove .bak`；写失败回滚 `path.bak → path` 并保留
//!    备份供排查（与 [edit.md §2.4.1](.) 第 4 条一致：「校验失败磁盘原样、无 `.bak` 残留」）。
//! 5. **行号 API 保留**：`EditOperation` 的 `start_line` / `Insert` / `Delete` 路径
//!    仍供 dispatcher / extension 内部调用，本期 LLM 主路径只走「字符串 + 无行号」分支。
//!
//! diff 文本由 [`super::super::diff::build_simple_diff`] 生成（副作用日志，调用结果可丢弃）。

use super::helpers::{grant_trigger_str, grant_type_str, permission_scope_str};
use super::DefaultPrimitiveExecutor;
use crate::core::tools::pipeline::edit_normalize::{
    detect_line_ending, fold_curly_quotes, normalize_to_lf, restore_line_endings, strip_bom,
};
use crate::core::tools::primitive::diff::build_simple_diff;
use crate::core::tools::primitive::{
    EditFileResult, EditOperation, EditOperationType, PrimitiveExecutor, PrimitiveOperation,
    WriteFileResult, EDIT_REPLACE_ALL_MARKER,
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

    // T2-P0-016 PR-C 二道防线：tool_exec 已先做 `exists && !overwrite` 早退，
    // 但 dispatcher / extension 可能直接调 trait（绕过编排），这里再挡一次。
    // 与 edit 的 `.ipynb` 在 tool_exec + primitive 双层一致。
    let pre_existed = path_buf.exists();
    if !overwrite && pre_existed {
        return Err(AppError::Primitive(format!(
            "Exists: 路径 `{}` 已存在；如需替换请显式 `overwrite=true`",
            path
        )));
    }

    // T2-P0-016 PR-G：覆盖写时先读旧内容用于 diff hint（顺序：先 metadata 由编排层
    // 校验 stamp → 这里读全文算 diff → 落 .bak → 写盘）。读盘失败不阻断写流程，
    // 只是回执里不带 diff 摘要。
    let original: Option<String> = if overwrite && pre_existed {
        crate::infra::platform::read_file_utf8(&path_buf).ok()
    } else {
        None
    };

    // T2-P0-016 PR-G：可配置 LF 规范化（默认开）。关闭时按字节透传模型给的 `content`。
    let final_bytes: Vec<u8> = if executor.write_normalize_crlf {
        content.replace("\r\n", "\n").into_bytes()
    } else {
        content.as_bytes().to_vec()
    };

    // T2-P0-016 T3-K：secrets 扫描在 .bak / 落盘之前。命中走 require_user_confirmation；
    // 用户拒 → 返回 SecretsRejected，磁盘字节级未变（与 edit `edit_file_impl` L118–L142 对称，
    // 两条路径**仅 op_label 不同**：Edit / Write）。
    let final_text = std::str::from_utf8(&final_bytes).unwrap_or("");
    let original_for_secrets = original.as_deref().unwrap_or("");
    if let Some(hits) = scan_new_content_for_secrets(original_for_secrets, final_text) {
        let preview = crate::core::security::secrets::format_preview(&hits);
        let approved = executor
            .require_user_confirmation(PrimitiveOperation::Write, &preview, plugin_id)
            .await?;
        if !approved {
            executor.audit.record_primitive(PrimitiveAuditEntry {
                operation: AuditPrimitiveOp::Write,
                path_or_cmd: path_str.clone(),
                plugin_id: plugin_id.to_string(),
                user_approved: false,
                success: false,
                detail: Some(format!("SecretsRejected: {} hits", hits.len())),
                permission_scope: Some(permission_scope_str(scope)),
                grant_type: Some(grant_type_str(grant.grant_type)),
                grant_trigger: Some(grant_trigger_str(grant.trigger)),
            });
            return Err(AppError::Primitive(format!(
                "SecretsRejected: write 内容命中 {} 条潜在敏感信息且用户拒绝写入；磁盘未被修改",
                hits.len()
            )));
        }
    }

    if overwrite && pre_existed {
        let backup = path_buf.with_extension("bak");
        let _ = std::fs::copy(&path_buf, &backup);
    }

    write_file_atomic(&path_buf, &final_bytes)?;
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

    let diff_hint = original.as_deref().map(|orig| {
        // 写盘后的字节再以 UTF-8 解码用于 diff；非 UTF-8（理论上 LF 规范化前 content 就已是 &str）回退为空。
        let after = std::str::from_utf8(&final_bytes).unwrap_or("");
        build_simple_diff(orig, after)
    });

    Ok(WriteFileResult {
        path: crate::infra::platform::format_home_path(&path_buf),
        written: true,
        bytes_written: final_bytes.len() as u64,
        diff_hint,
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

    // T2-P0-017 PR-D：行号路径（dispatcher / extension）保留，LLM 主路径走 `apply_string_edits`。
    // 区分依据与历史保持一致：`Replace` 且 `start_line.is_none()` ⇒ 字符串模式；其余 ⇒ 行号模式。
    let is_line_oriented = edits.iter().any(|e| match e.operation_type {
        EditOperationType::Replace => e.start_line.is_some(),
        EditOperationType::Insert | EditOperationType::Delete => true,
    });

    let result = if is_line_oriented {
        apply_line_oriented_edits(&path_buf, &edits)
    } else {
        apply_string_edits(&path_buf, &edits, path)
    };

    let (original, new_content) = match result {
        Ok(v) => v,
        Err(e) => {
            // 校验阶段失败：磁盘未变、无 .bak 残留（apply_* 函数不创建 .bak）。
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
    };

    let _ = build_simple_diff(original.as_str(), &new_content);

    // T2-P0-017 PR-M T3-K：secrets 扫描在 .bak 之前。命中走 require_user_confirmation；
    // 用户拒 → 返回 SecretsRejected，磁盘字节级未变；用户允 → 继续写盘。
    if let Some(hits) = scan_new_content_for_secrets(&original, &new_content) {
        let preview = crate::core::security::secrets::format_preview(&hits);
        let approved = executor
            .require_user_confirmation(PrimitiveOperation::Edit, &preview, plugin_id)
            .await?;
        if !approved {
            executor.audit.record_primitive(PrimitiveAuditEntry {
                operation: AuditPrimitiveOp::Edit,
                path_or_cmd: path_str,
                plugin_id: plugin_id.to_string(),
                user_approved: false,
                success: false,
                detail: Some(format!("SecretsRejected: {} hits", hits.len())),
                permission_scope: Some(permission_scope_str(scope)),
                grant_type: Some(grant_type_str(grant.grant_type)),
                grant_trigger: Some(grant_trigger_str(grant.trigger)),
            });
            return Err(AppError::Primitive(format!(
                "SecretsRejected: edit 内容命中 {} 条潜在敏感信息且用户拒绝写入；磁盘未被修改",
                hits.len()
            )));
        }
    }

    // 校验全通过 → 写盘前 copy .bak（仅作崩溃兜底；写成功删除）。
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
        // 写盘失败：从 .bak 恢复磁盘内容；保留 .bak 供事后排查。
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
        detail: None,
        permission_scope: Some(permission_scope_str(scope)),
        grant_type: Some(grant_type_str(grant.grant_type)),
        grant_trigger: Some(grant_trigger_str(grant.trigger)),
    });
    Ok(EditFileResult {
        path: crate::infra::platform::format_home_path(&path_buf),
        applied: true,
    })
}

/// 单段意图的内部表示（解析 `EditOperation` + 剥离 `EDIT_REPLACE_ALL_MARKER`）。
struct EditSegment<'a> {
    old: &'a str,
    new: &'a str,
    replace_all: bool,
}

fn parse_segment(op: &EditOperation) -> Result<EditSegment<'_>, AppError> {
    let raw_old = op
        .old_content
        .as_deref()
        .ok_or_else(|| AppError::Primitive("edit: old_content 缺失".to_string()))?;
    let (replace_all, old) = match raw_old.strip_prefix(EDIT_REPLACE_ALL_MARKER) {
        Some(rest) => (true, rest),
        None => (false, raw_old),
    };
    if old.is_empty() {
        return Err(AppError::Primitive(
            "edit: old_content 不能为空字符串".to_string(),
        ));
    }
    Ok(EditSegment {
        old,
        new: op.new_content.as_str(),
        replace_all,
    })
}

/// T2-P0-017 PR-D + PR-H 主路径：BOM/换行/curly/desanitize 归一化匹配后，对 working_lf splice。
///
/// **PR-H normalize 接入**（[edit.md §2.4.4](../../../../../docs/architecture/tools/edit.md)）：
/// 1. 读盘 → `disk_text`；
/// 2. `strip_bom` + `detect_line_ending` → `(no_bom, kind, had_bom)`；
/// 3. `normalize_to_lf(no_bom)` → `working_lf`（仅 splice 用，CRLF 折叠为 LF）；
/// 4. `build_normalized_byte_map(working_lf)` → `(n_text, n_to_w_map)`；
/// 5. 段 `old` 走 `normalize_for_match` → `n_old`；在 `n_text` 上 `match_indices` 命中后通过 map 反查回 `working_lf` 字节区间；
/// 6. 在 `working_lf` 上按降序 splice，再 `restore_line_endings(kind, ...)` + 可选 prepend BOM。
///
/// 这条管线让模型可用 `“foo”` 命中磁盘 `"foo"`；同时 splice 区间外的字节在
/// `working_lf` → `restored` → `write_back` 链路上**仅**做了 LE 还原与 BOM prepend，
/// 因此 CRLF/BOM 文件被改后仍是 CRLF/BOM。
///
/// 返回 `(disk_text, write_back)` 供调用方做 diff / 写盘；磁盘**不**被本函数触碰。
fn apply_string_edits(
    path_buf: &std::path::Path,
    edits: &[EditOperation],
    user_path: &str,
) -> Result<(String, String), AppError> {
    let disk_text = read_file_utf8(path_buf).map_err(|e| match e {
        AppError::Io(io) if io.kind() == std::io::ErrorKind::InvalidData => AppError::Primitive(
            format!(
                "BinaryFile: `{}` 不是有效的 UTF-8 文本，edit 仅支持文本文件；请改用 read 查看二进制提示或换用合适工具",
                user_path
            ),
        ),
        AppError::Primitive(msg) if msg.contains("UTF-8") || msg.contains("invalid utf-8") => {
            AppError::Primitive(format!(
                "BinaryFile: `{}` 不是 UTF-8 文本，edit 拒绝执行 ({})",
                user_path, msg
            ))
        }
        other => other,
    })?;

    let (no_bom, had_bom) = strip_bom(&disk_text);
    let kind = detect_line_ending(no_bom);
    let working_lf: String = normalize_to_lf(no_bom).into_owned();
    let (n_text, n_to_w_map) =
        crate::core::tools::pipeline::edit_normalize::build_normalized_byte_map(&working_lf);

    // 收集 (working_lf 起, working_lf 止, replacement) 区间。
    let mut spans: Vec<(usize, usize, String)> = Vec::new();
    for (idx, op) in edits.iter().enumerate() {
        let seg = parse_segment(op)?;
        let n_old = crate::core::tools::pipeline::edit_normalize::normalize_for_match(seg.old);
        if n_old.is_empty() {
            return Err(AppError::Primitive(format!(
                "edit: edits[{}] 的 old_content 归一化后为空（仅含 BOM/零宽字符），无法匹配",
                idx
            )));
        }
        let lf_new: String = normalize_to_lf(seg.new).into_owned();

        let mut n_hits: Vec<usize> = n_text
            .match_indices(n_old.as_str())
            .map(|(b, _)| b)
            .collect();
        if n_hits.is_empty() {
            return Err(AppError::Primitive(format!(
                "NotFound: edits[{}] 的 old_content 在文件 `{}` 中未找到 (已尝试 BOM/换行/引号/不可见字符归一化; 请检查上下文是否唯一或扩大上下文)",
                idx, user_path
            )));
        }
        if !seg.replace_all && n_hits.len() > 1 {
            return Err(AppError::Primitive(format!(
                "Ambiguous: edits[{}] 的 old_content 在文件 `{}` 中出现 {} 次; 请扩大上下文使其唯一, 或设置 replace_all: true",
                idx, user_path, n_hits.len()
            )));
        }
        if !seg.replace_all {
            n_hits.truncate(1);
        }
        for n_start in n_hits {
            let n_end = n_start + n_old.len();
            let w_start = *n_to_w_map.get(n_start).ok_or_else(|| {
                AppError::Primitive("edit: normalize map index out of range (start)".to_string())
            })?;
            let w_end = *n_to_w_map.get(n_end).ok_or_else(|| {
                AppError::Primitive("edit: normalize map index out of range (end)".to_string())
            })?;
            spans.push((w_start, w_end, lf_new.clone()));
        }
    }

    // 重叠检测：按起点排序；s2 < e1 即拒，s2 == e1（边界相邻）允许。
    spans.sort_by_key(|(s, _, _)| *s);
    for w in spans.windows(2) {
        let (s1, e1, _) = w[0];
        let (s2, e2, _) = w[1];
        if s2 < e1 {
            return Err(AppError::Primitive(format!(
                "Overlap: edit 段在文件 `{}` 中存在交叠/嵌套区间 ([{}..{}) 与 [{}..{})); 请合并为单段或拆为两次 edit 调用",
                user_path, s1, e1, s2, e2
            )));
        }
    }

    // 在 working_lf 上按降序 splice。
    let mut new_working_lf = working_lf.clone();
    for (start, end, replacement) in spans.iter().rev() {
        new_working_lf.replace_range(*start..*end, replacement);
    }
    let restored = restore_line_endings(kind, &new_working_lf);
    let write_back = if had_bom {
        let mut s = String::with_capacity(restored.len() + 3);
        s.push('\u{FEFF}');
        s.push_str(&restored);
        s
    } else {
        restored.into_owned()
    };
    // fold_curly_quotes 暴露给上层做诊断，本函数链路通过 build_normalized_byte_map 间接消费。
    let _ = fold_curly_quotes;
    Ok((disk_text, write_back))
}

/// 仅当 `new_content` **新增**了之前不在 `original` 的命中时才触发 confirm。
///
/// 这样「读出含密钥的文件 → 改一行不相关的代码 → 写回」不会反复打扰用户；
/// 只有「edit **引入**了新的敏感片段」才走 confirm。
fn scan_new_content_for_secrets(
    original: &str,
    new_content: &str,
) -> Option<Vec<crate::core::security::secrets::SecretHit>> {
    let new_hits = crate::core::security::secrets::scan(new_content);
    if new_hits.is_empty() {
        return None;
    }
    let original_hits = crate::core::security::secrets::scan(original);
    let original_set: std::collections::HashSet<&str> =
        original_hits.iter().map(|h| h.matched.as_str()).collect();
    let novel: Vec<_> = new_hits
        .into_iter()
        .filter(|h| !original_set.contains(h.matched.as_str()))
        .collect();
    if novel.is_empty() {
        None
    } else {
        Some(novel)
    }
}

/// 行号路径（dispatcher / extension 内部 API 兼容）。沿用旧实现中的逐行修改语义；
/// LLM 主路径**不**走此分支（catalog schema 不暴露 `start_line` / `end_line`）。
fn apply_line_oriented_edits(
    path_buf: &std::path::Path,
    edits: &[EditOperation],
) -> Result<(String, String), AppError> {
    let original = read_file_utf8(path_buf)?;
    let mut lines: Vec<String> = original.lines().map(String::from).collect();
    for edit in edits {
        match edit.operation_type {
            EditOperationType::Replace => {
                if let Some(start_line_val) = edit.start_line {
                    let start = start_line_val as usize;
                    let end = edit.end_line.unwrap_or(start_line_val) as usize;
                    if start < 1 || end > lines.len() || start > end {
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
                    return Err(AppError::Primitive(format!("Insert 行号超出: {}", at)));
                }
                lines.insert(at, edit.new_content.clone());
            }
            EditOperationType::Delete => {
                let start = edit.start_line.unwrap_or(1) as usize;
                let end = edit.end_line.unwrap_or(start as u64) as usize;
                if start < 1 || end > lines.len() || start > end {
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
    Ok((original, new_content))
}
