//! # `read` / `read_file` / `list_dir` 实现
//!
//! 集中承载 PR-RA/RB/RF/RJ/RM 加强后的 `read` 工具：分块流式抽窗、cat-n
//! 行号、PR-RM hashline（xxh32）、PR-RJ T3 image/PDF 多模态路由，以及历史
//! `read_file` 公共 API 与最小 `list_dir`。
//!
//! 子模块 `pub(crate)` 暴露的 helper（`detect_inline_mime` /
//! `compute_line_hash` / `format_with_hashlines` / `format_with_line_numbers`）
//! 由 [`super`]（`executor/mod.rs`）通过 `pub(crate) use` 重新对外，保持
//! `primitive::executor::xxx` 引用路径在子模块化前后完全等价。

use super::helpers::{grant_trigger_str, grant_type_str, permission_scope_str, url_like_fs_miss};
use super::DefaultPrimitiveExecutor;
use crate::core::tools::primitive::{
    DirEntry, PrimitiveOperation, ReadBinaryResult, ReadResult, ReadTextResult,
};
use crate::infra::audit::{AuditPrimitiveOp, PrimitiveAuditEntry};
use crate::infra::error::AppError;
use crate::infra::platform::read_file_utf8;
use std::path::Path;

/// PR-RB（T1）流式分块读的固定 buffer 大小。
///
/// 64 KiB 是 wasm 友好的小块（堆压力低），同时与典型 page cache 命中粒度对齐；
/// 加大没有明显收益，加小会让 syscall 数量过多。
const READ_CHUNK_BYTES: usize = 64 * 1024;

/// PR-RB（T1）默认 limit（行数），与 cc-fork `MAX_LINES_TO_READ` 对齐。
const READ_DEFAULT_LIMIT_LINES: u64 = 2000;

/// 空响应整改阶段一：`read` 文本路径的后读预算护栏。
///
/// 与 metadata 阶段的 `read_max_bytes` 不同，这一条限制的是**最终渲染回模型的文本体量**：
/// 在分块读 + 行级拼装过程中累计输出字节，达到 128 KiB 后就在完整行边界停下，
/// 并返回 `offset=<next>` 续读提示，避免单个 `read` 窗口直接把上下文顶爆。
const READ_POST_OUTPUT_BUDGET_BYTES: usize = 128 * 1024;

fn rendered_prefix_len(line_no: u64, line_numbers: bool, hashline: bool) -> usize {
    let width = std::cmp::max(6, line_no.to_string().len());
    if hashline {
        width + 4 // "#{2-char}:"
    } else if line_numbers {
        width + 1 // "\t"
    } else {
        0
    }
}

fn rendered_line_len(
    line_no: u64,
    raw_line_bytes: usize,
    line_numbers: bool,
    hashline: bool,
) -> usize {
    rendered_prefix_len(line_no, line_numbers, hashline) + raw_line_bytes
}

/// PR-RJ（T3-b）`read` 工具的 mime 路由：扩展名 + 头几字节 magic 双重校验。
///
/// 仅返回 image / PDF 两类「需要走 inline content part 通道」的 mime；
/// 其他（包括 `.txt` 之外的扩展名 + 任何二进制 fallback）都返回 `None`,
/// 走文本 / 二进制 hint 路径（与 PR-RB §2.3 一致）。
///
/// **设计权衡**（详见 `read.md` §4.1 的「不引解码 / 缩放依赖」论述）：
/// - **不**引 `image` / `infer` 等 crate，`Cargo.lock` 零增长；
/// - 扩展名先行，magic 兜底——避免 `.png` 后缀挂着 PDF 字节这类小概率的误路由；
/// - PDF 的 magic 是 `%PDF-`（5 字节），PNG 是 `89 50 4E 47`，JPEG 是 `FF D8 FF`，
///   GIF 是 `47 49 46 38`，WebP 需要在 RIFF 头里看 `WEBP`（`52 49 46 46 .. .. .. .. 57 45 42 50`）。
pub(crate) fn detect_inline_mime(path: &Path) -> Option<DetectedInlineMime> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase());
    let ext = ext.as_deref()?;
    let candidate = match ext {
        "png" => Some(("image/png", InlineKind::Image)),
        "jpg" | "jpeg" => Some(("image/jpeg", InlineKind::Image)),
        "gif" => Some(("image/gif", InlineKind::Image)),
        "webp" => Some(("image/webp", InlineKind::Image)),
        "pdf" => Some(("application/pdf", InlineKind::Pdf)),
        _ => None,
    }?;
    // 头 12 字节足够覆盖以上所有 magic（最长是 WebP 的 RIFF + WEBP = 12 字节）。
    let head = read_head_bytes(path, 12).ok()?;
    if !magic_matches(candidate.0, &head) {
        return None;
    }
    Some(DetectedInlineMime {
        mime: candidate.0.to_string(),
        kind: candidate.1,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InlineKind {
    Image,
    Pdf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DetectedInlineMime {
    pub(crate) mime: String,
    pub(crate) kind: InlineKind,
}

fn read_head_bytes(path: &Path, n: usize) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut buf = vec![0u8; n];
    let read = f.read(&mut buf)?;
    buf.truncate(read);
    Ok(buf)
}

fn magic_matches(mime: &str, head: &[u8]) -> bool {
    match mime {
        "image/png" => head.starts_with(&[0x89, 0x50, 0x4E, 0x47]),
        "image/jpeg" => head.starts_with(&[0xFF, 0xD8, 0xFF]),
        "image/gif" => head.starts_with(b"GIF8"),
        "image/webp" => head.len() >= 12 && &head[0..4] == b"RIFF" && &head[8..12] == b"WEBP",
        "application/pdf" => head.starts_with(b"%PDF-"),
        _ => false,
    }
}

/// PR-RM（T3 hashline）`pi_agent_rust::compute_line_hash` 25 行实现的等价 Rust 版。
///
/// 算法（与 `pi_agent_rust/src/tools.rs` 5451–5466 一字对齐）：
/// 1. `strip_suffix('\r')`：去 Windows 换行残留；
/// 2. 移除所有空白（`char::is_whitespace`）得到 `significant`——「缩进改动**不影响 hash**」；
/// 3. seed：含字母数字字符 → 0；纯标点 / 空行 → 行号（让空行也有唯一 hash）；
/// 4. `xxh32(significant_bytes, seed) & 0xFF`；
/// 5. 取低字节按 4-bit nibble 拆 → 字典 `b"ZPMQVRWSNKTXJBYH"` 映射为 2 字符
///    （字典刻意避开 `O / I / 0 / 1` 等易混字符，便于人眼粘贴 / 比对）。
///
/// hashline 与 cat-n 行号互斥：spec §3.1 规定「hashline 优先」，
/// 调用方在 [`crate::core::agent_loop::tool_exec`] 入口已做去抖。
pub(crate) fn compute_line_hash(line: &str, line_no: u64) -> String {
    let trimmed = line.strip_suffix('\r').unwrap_or(line);
    let significant: String = trimmed.chars().filter(|c| !c.is_whitespace()).collect();
    let seed: u32 = if trimmed.chars().any(|c| c.is_ascii_alphanumeric()) {
        0
    } else {
        // pi_agent_rust 用行号低 32 位作为 seed；这里同样 cast，避免大文件 wrap。
        line_no as u32
    };
    let raw = xxhash_rust::xxh32::xxh32(significant.as_bytes(), seed);
    let low = (raw & 0xFF) as u8;
    const ALPHABET: &[u8; 16] = b"ZPMQVRWSNKTXJBYH";
    let high_nibble = ALPHABET[((low >> 4) & 0x0F) as usize] as char;
    let low_nibble = ALPHABET[(low & 0x0F) as usize] as char;
    let mut s = String::with_capacity(2);
    s.push(high_nibble);
    s.push(low_nibble);
    s
}

/// PR-RM（T3 hashline）`{1-based 行号}#{2 字符 hash}:{原行内容}` 渲染。
///
/// 与 [`format_with_line_numbers`] 互斥：本函数被调用时**必然** `hashline=true`，
/// 此时 cat-n 行号被忽略（避免双重行号噪音）。
///
/// 行尾保留：`split_inclusive('\n')` 让 trailing newline 落到原行结尾，
/// 与上游 `pi_agent_rust` 输出一致；空 body 直接返回空串。
pub(crate) fn format_with_hashlines(start_line: u64, body: &str) -> String {
    if body.is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity(body.len() + body.len() / 16);
    let mut line_no = start_line;
    for line in body.split_inclusive('\n') {
        let bare = line.strip_suffix('\n').unwrap_or(line);
        let tag = compute_line_hash(bare, line_no);
        out.push_str(&format!("{:>6}#{}:{}", line_no, tag, line));
        line_no = line_no.saturating_add(1);
    }
    out
}

/// PR-RF（T2-a）`cat -n` 风格行号渲染：每行前缀 `{:>6}\t`（6 格右对齐 + Tab）。
///
/// - **行号语义**：`start_line` 是该 body **第一行的绝对行号**（1-based）；
///   后续行依次递增。截断尾注由调用方追加，**不**进入本函数。
/// - **格式来源**：与 `cc-fork-01` `addLineNumbers` 一致，便于 IDE / diff 工具
///   横向比对（详见 `docs/architecture/tools/read.md` §3.1）。
/// - **行尾处理**：`split_inclusive('\n')` 保留每行末尾换行；最后一行若无换行
///   也按裸行渲染（与原始内容一致，不补 `\n`）。
/// - **空 body**：返回空字符串（不强行打印 `1\t`），与 cat -n 行为一致。
pub(crate) fn format_with_line_numbers(start_line: u64, body: &str) -> String {
    if body.is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity(body.len() + body.len() / 32);
    let mut line_no = start_line;
    for line in body.split_inclusive('\n') {
        out.push_str(&format!("{:>6}\t{}", line_no, line));
        line_no = line_no.saturating_add(1);
    }
    out
}

/// PR-RB（T1）`read` 工具流式抽窗的返回值。
///
/// `Binary` 用于二进制 / 非 UTF-8 文件的早期检测：
/// 第一块（最多 [`READ_CHUNK_BYTES`]）若含 `\x00` → 立即判定，**不**继续扫描。
/// 这与 grep/cat 行业惯例一致，避免把超大二进制读到一半才发现要拒。
enum ReadWindowOutcome {
    Text {
        /// 窗口字节（已包含每行尾部 `\n`，最后一行若无换行也保留原样）。
        window: Vec<u8>,
        /// 窗口内实际写入的文本行数（不含尾注）。
        num_lines: u64,
        /// 截断信息：可能来自 `limit`，也可能来自 128 KiB 后读预算。
        truncation: Option<ReadWindowTruncation>,
    },
    Binary {
        /// 触发判定的字节十六进制（如 `"89"` 提示 PNG，`"25"` 提示 PDF）。
        first_byte_hex: String,
    },
    FirstLineTooLong {
        /// 触发护栏的首个返回行号（即 `offset` 对应的首行）。
        line_no: u64,
        /// 该行渲染后的字节数（已计入行号 / hashline 前缀）。
        rendered_bytes: usize,
        /// 允许的后读预算上限。
        budget_bytes: usize,
    },
}

enum ReadWindowTruncation {
    /// 命中 `limit`：继续扫到 EOF，仅计数剩余行数。
    Limit {
        remaining_lines: u64,
        next_offset: u64,
    },
    /// 命中 128 KiB 后读预算：在完整行边界提前停止，不再读完整个请求窗口。
    OutputBudget { next_offset: u64 },
}

/// PR-RB（T1）阻塞式分块读 + memchr 单循环抽窗。
///
/// 在 [`tokio::task::spawn_blocking`] 里跑，避免阻塞 reactor。
///
/// 算法（与 `read.md` §2.4 对齐）：
/// 1. 按 [`READ_CHUNK_BYTES`] 反复 `read`；
/// 2. 用 `memchr::memchr_iter(b'\n', chunk)` 数换行；
/// 3. 维护 `current_line`（1-based）：
///    - `current_line < start_line` → **跳过**（指针 + 计数，不分配 String）；
///    - `start_line ≤ current_line < start_line + limit_lines` → 收到 `window`；
///    - `current_line ≥ start_line + limit_lines` → 进入「仅计数尾部」阶段；
/// 4. 第一块若含 `\x00` → `Binary` 早返；
/// 5. EOF 后若仍有 leftover（无换行结尾） → 按是否在窗口内补齐。
fn read_window_blocking(
    path: &Path,
    start_line: u64,
    limit_lines: u64,
    line_numbers: bool,
    hashline: bool,
    output_budget_bytes: usize,
) -> Result<ReadWindowOutcome, AppError> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(AppError::Io)?;
    let mut buf = vec![0u8; READ_CHUNK_BYTES];
    let mut window: Vec<u8> = Vec::new();
    let mut leftover: Vec<u8> = Vec::new();
    let mut current_line: u64 = 1;
    let mut window_lines: u64 = 0;
    let mut rendered_output_bytes: usize = 0;
    let end_line_exclusive = start_line.saturating_add(limit_lines);
    let mut limit_truncated = false;
    let mut remaining_lines: u64 = 0;
    let mut budget_next_offset: Option<u64> = None;
    let mut first_chunk = true;

    loop {
        let n = file.read(&mut buf).map_err(AppError::Io)?;
        if let Some(next_offset) = budget_next_offset {
            if n == 0 {
                break;
            }
            return Ok(ReadWindowOutcome::Text {
                window,
                num_lines: window_lines,
                truncation: Some(ReadWindowTruncation::OutputBudget { next_offset }),
            });
        }
        if n == 0 {
            break;
        }
        let chunk = &buf[..n];

        if first_chunk {
            first_chunk = false;
            if memchr::memchr(0, chunk).is_some() {
                let first_byte_hex = format!("{:02X}", chunk[0]);
                return Ok(ReadWindowOutcome::Binary { first_byte_hex });
            }
        }

        let mut last_consumed = 0usize;
        for nl in memchr::memchr_iter(b'\n', chunk) {
            let line_slice = &chunk[last_consumed..=nl];
            last_consumed = nl + 1;

            if limit_truncated {
                remaining_lines = remaining_lines.saturating_add(1);
                current_line = current_line.saturating_add(1);
                continue;
            }

            let within_window = current_line >= start_line && current_line < end_line_exclusive;
            if within_window {
                let raw_line_bytes = leftover.len() + line_slice.len();
                let rendered_line_bytes =
                    rendered_line_len(current_line, raw_line_bytes, line_numbers, hashline);
                if window_lines == 0 && rendered_line_bytes > output_budget_bytes {
                    return Ok(ReadWindowOutcome::FirstLineTooLong {
                        line_no: current_line,
                        rendered_bytes: rendered_line_bytes,
                        budget_bytes: output_budget_bytes,
                    });
                }
                if !leftover.is_empty() {
                    window.extend_from_slice(&leftover);
                    leftover.clear();
                }
                window.extend_from_slice(line_slice);
                window_lines = window_lines.saturating_add(1);
                rendered_output_bytes = rendered_output_bytes.saturating_add(rendered_line_bytes);
            } else if !leftover.is_empty() {
                leftover.clear();
            }

            current_line = current_line.saturating_add(1);

            if within_window && rendered_output_bytes >= output_budget_bytes {
                if last_consumed < chunk.len() {
                    return Ok(ReadWindowOutcome::Text {
                        window,
                        num_lines: window_lines,
                        truncation: Some(ReadWindowTruncation::OutputBudget {
                            next_offset: current_line,
                        }),
                    });
                }
                budget_next_offset = Some(current_line);
                break;
            }

            if window_lines >= limit_lines && !limit_truncated {
                limit_truncated = true;
            }
        }

        if budget_next_offset.is_some() {
            continue;
        }

        let tail = &chunk[last_consumed..];
        if !tail.is_empty()
            && current_line >= start_line
            && current_line < end_line_exclusive
            && !limit_truncated
        {
            leftover.extend_from_slice(tail);
            if window_lines == 0 {
                let rendered_line_bytes =
                    rendered_line_len(current_line, leftover.len(), line_numbers, hashline);
                if rendered_line_bytes > output_budget_bytes {
                    return Ok(ReadWindowOutcome::FirstLineTooLong {
                        line_no: current_line,
                        rendered_bytes: rendered_line_bytes,
                        budget_bytes: output_budget_bytes,
                    });
                }
            }
        }
    }

    if !leftover.is_empty()
        && !limit_truncated
        && current_line >= start_line
        && current_line < end_line_exclusive
    {
        let rendered_line_bytes =
            rendered_line_len(current_line, leftover.len(), line_numbers, hashline);
        if window_lines == 0 && rendered_line_bytes > output_budget_bytes {
            return Ok(ReadWindowOutcome::FirstLineTooLong {
                line_no: current_line,
                rendered_bytes: rendered_line_bytes,
                budget_bytes: output_budget_bytes,
            });
        }
        window.extend_from_slice(&leftover);
        window_lines = window_lines.saturating_add(1);
    }

    let truncation = if limit_truncated {
        Some(ReadWindowTruncation::Limit {
            remaining_lines,
            next_offset: start_line.saturating_add(limit_lines),
        })
    } else {
        None
    };

    Ok(ReadWindowOutcome::Text {
        window,
        num_lines: window_lines,
        truncation,
    })
}

pub(super) async fn read_file_impl(
    executor: &DefaultPrimitiveExecutor,
    path: &str,
    plugin_id: &str,
) -> Result<String, AppError> {
    if let Some(err) = url_like_fs_miss(path) {
        return Err(err);
    }
    let (path_buf, scope, grant) = executor
        .gate_check_path(PrimitiveOperation::Read, path, plugin_id)
        .await?;
    let meta = std::fs::metadata(&path_buf).map_err(AppError::Io)?;
    if meta.is_dir() {
        return Err(AppError::Primitive(
            "路径是目录，无法读取为文件".to_string(),
        ));
    }
    if meta.len() > executor.read_max_bytes {
        return Err(AppError::Primitive(format!(
            "文件过大 ({} bytes)，超过限制 {} bytes",
            meta.len(),
            executor.read_max_bytes
        )));
    }
    let content = read_file_utf8(&path_buf).map_err(|e| match e {
        AppError::Config(msg) if msg.contains("invalid utf-8") => AppError::Primitive(format!(
            "文件存在且权限已通过检查，但它是二进制或非 UTF-8 文本，不能用 read_file 按文本读取：{}",
            path_buf.display()
        )),
        other => other,
    })?;
    executor.audit.record_primitive(PrimitiveAuditEntry {
        operation: AuditPrimitiveOp::Read,
        path_or_cmd: path.to_string(),
        plugin_id: plugin_id.to_string(),
        user_approved: true,
        success: true,
        detail: None,
        permission_scope: Some(permission_scope_str(scope)),
        grant_type: Some(grant_type_str(grant.grant_type)),
        grant_trigger: Some(grant_trigger_str(grant.trigger)),
    });
    Ok(content)
}

/// PR-RB（T1）`read` 工具入口：metadata 阶段大小预检 + 分块流式 + memchr 单循环抽窗。
///
/// 详见 `docs/architecture/tools/read.md` §2.1–§2.5。
/// `offset`/`limit` 的边界（`offset >= 1` / `1 ≤ limit ≤ 10000`）已在
/// [`crate::core::agent_loop::tool_exec`] 入口（§2.6）兜底，本方法仅
/// `clamp` 防御。
pub(super) async fn read_impl(
    executor: &DefaultPrimitiveExecutor,
    path: &str,
    offset: Option<u64>,
    limit: Option<u64>,
    line_numbers: bool,
    hashline: bool,
    plugin_id: &str,
) -> Result<ReadResult, AppError> {
    if let Some(err) = url_like_fs_miss(path) {
        return Err(err);
    }
    let (path_buf, scope, grant) = executor
        .gate_check_path(PrimitiveOperation::Read, path, plugin_id)
        .await?;
    let meta = std::fs::metadata(&path_buf).map_err(AppError::Io)?;
    if meta.is_dir() {
        return Err(AppError::Primitive(
            "路径是目录，无法读取为文件".to_string(),
        ));
    }

    // PR-RJ T3-b：image / PDF 路由。`offset`/`limit` 对二进制无意义——
    // 命中即按 inline 通道走，metadata 阶段判大小（不读字节、不 base64）。
    if let Some(detected) = detect_inline_mime(&path_buf) {
        let (max_bytes, label) = match detected.kind {
            InlineKind::Image => (
                crate::core::llm::IMAGE_MAX_BYTES as u64,
                "IMAGE_MAX_BYTES (4.5 MiB)",
            ),
            InlineKind::Pdf => (
                crate::core::llm::FILE_MAX_BYTES as u64,
                "FILE_MAX_BYTES (25 MiB)",
            ),
        };
        if meta.len() > max_bytes {
            return Err(AppError::Primitive(format!(
                "File ({} bytes, mime={}) exceeds {} for inline content parts. Either trim the asset, host it externally, or upload via the Files API once the upload manager lands (T2-P0-015).",
                meta.len(),
                detected.mime,
                label
            )));
        }
        let filename = path_buf
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path_buf.display().to_string());
        let binary = ReadBinaryResult {
            mime: detected.mime,
            original_size: meta.len(),
            path: path_buf.clone(),
            filename,
        };
        executor.audit.record_primitive(PrimitiveAuditEntry {
            operation: AuditPrimitiveOp::Read,
            path_or_cmd: path.to_string(),
            plugin_id: plugin_id.to_string(),
            user_approved: true,
            success: true,
            detail: Some(format!(
                "read inline kind={:?} mime={} bytes={}",
                detected.kind, binary.mime, binary.original_size
            )),
            permission_scope: Some(permission_scope_str(scope)),
            grant_type: Some(grant_type_str(grant.grant_type)),
            grant_trigger: Some(grant_trigger_str(grant.trigger)),
        });
        return Ok(match detected.kind {
            InlineKind::Image => ReadResult::Image(binary),
            InlineKind::Pdf => ReadResult::Pdf(binary),
        });
    }

    let has_window = offset.is_some() || limit.is_some();
    if !has_window && meta.len() > executor.read_max_bytes {
        return Err(AppError::Primitive(format!(
            "File is large ({} bytes > {} bytes). Pass `offset` and `limit` to read a specific window, e.g. `read(path, offset=1, limit=2000)`. (decision: docs/architecture/tools/read.md §2.5)",
            meta.len(),
            executor.read_max_bytes
        )));
    }

    let start_line = offset.unwrap_or(1).max(1);
    let limit_lines = limit.unwrap_or(READ_DEFAULT_LIMIT_LINES).max(1);

    let path_clone = path_buf.clone();
    let read_outcome = tokio::task::spawn_blocking(move || {
        read_window_blocking(
            &path_clone,
            start_line,
            limit_lines,
            line_numbers,
            hashline,
            READ_POST_OUTPUT_BUDGET_BYTES,
        )
    })
    .await
    .map_err(|e| AppError::Primitive(format!("read join error: {}", e)))??;

    let text = match read_outcome {
        ReadWindowOutcome::Text {
            window,
            num_lines,
            truncation,
        } => {
            let body = String::from_utf8(window).map_err(|e| {
                AppError::Primitive(format!(
                    "File contains invalid UTF-8 mid-stream (byte {} not a valid sequence start): {}",
                    e.utf8_error().valid_up_to(),
                    path_buf.display()
                ))
            })?;
            // PR-RM：hashline 优先于 line_numbers（与 spec §3.1 一致）。
            let mut s = if hashline {
                format_with_hashlines(start_line, &body)
            } else if line_numbers {
                format_with_line_numbers(start_line, &body)
            } else {
                body
            };
            let (truncated, remaining_lines) = match truncation {
                Some(ReadWindowTruncation::Limit {
                    remaining_lines,
                    next_offset,
                }) => {
                    if !s.ends_with('\n') {
                        s.push('\n');
                    }
                    if remaining_lines > 0 {
                        s.push_str(&format!(
                            "... [{} more lines truncated; resume with offset={}, limit={}]\n",
                            remaining_lines, next_offset, limit_lines
                        ));
                    } else {
                        s.push_str(&format!(
                            "... [more lines truncated; resume with offset={}, limit={}]\n",
                            next_offset, limit_lines
                        ));
                    }
                    (true, remaining_lines)
                }
                Some(ReadWindowTruncation::OutputBudget { next_offset }) => {
                    if !s.ends_with('\n') {
                        s.push('\n');
                    }
                    s.push_str(&format!(
                        "... [output truncated at {} bytes post-read budget; resume with offset={}, limit={}]\n",
                        READ_POST_OUTPUT_BUDGET_BYTES, next_offset, limit_lines
                    ));
                    (true, 0)
                }
                None => (false, 0),
            };
            ReadTextResult {
                content: s,
                start_line,
                num_lines,
                truncated,
                remaining_lines,
            }
        }
        ReadWindowOutcome::Binary { first_byte_hex } => {
            return Err(AppError::Primitive(format!(
                "File is binary or non-UTF-8 (detected: 0x{first}). • try `bash file <path>` to inspect the type; • multimodal image/PDF will be supported in a later read upgrade (T3, docs/architecture/tools/read.md §4.1).",
                first = first_byte_hex
            )));
        }
        ReadWindowOutcome::FirstLineTooLong {
            line_no,
            rendered_bytes,
            budget_bytes,
        } => {
            return Err(AppError::Primitive(format!(
                "The first returned line (line {}) exceeds the post-read budget ({} bytes > {} bytes = 128KiB). Narrow the window with a smaller `offset`/`limit` so the first returned line is shorter.",
                line_no, rendered_bytes, budget_bytes
            )));
        }
    };

    executor.audit.record_primitive(PrimitiveAuditEntry {
        operation: AuditPrimitiveOp::Read,
        path_or_cmd: path.to_string(),
        plugin_id: plugin_id.to_string(),
        user_approved: true,
        success: true,
        detail: Some(format!(
            "read offset={} limit={} bytes_returned={} num_lines={}",
            start_line,
            limit_lines,
            text.content.len(),
            text.num_lines
        )),
        permission_scope: Some(permission_scope_str(scope)),
        grant_type: Some(grant_type_str(grant.grant_type)),
        grant_trigger: Some(grant_trigger_str(grant.trigger)),
    });
    Ok(ReadResult::Text(text))
}

pub(super) async fn list_dir_impl(
    executor: &DefaultPrimitiveExecutor,
    path: &str,
    plugin_id: &str,
) -> Result<Vec<DirEntry>, AppError> {
    if let Some(err) = url_like_fs_miss(path) {
        return Err(err);
    }
    let (path_buf, scope, grant) = executor
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
    executor.audit.record_primitive(PrimitiveAuditEntry {
        operation: AuditPrimitiveOp::Read,
        path_or_cmd: path.to_string(),
        plugin_id: plugin_id.to_string(),
        user_approved: true,
        success: true,
        detail: Some(format!("list_dir {} entries", entries.len())),
        permission_scope: Some(permission_scope_str(scope)),
        grant_type: Some(grant_type_str(grant.grant_type)),
        grant_trigger: Some(grant_trigger_str(grant.trigger)),
    });
    Ok(entries)
}
