use super::super::args::parse_optional_u64;
use super::super::guard::validate_read_bounds;
use super::super::{ToolDisplay, ToolExecCtx, AGENT_PLUGIN_ID};
use crate::infra::events::{ToolDisplayFileEntry, ToolDisplayFileStatus};
use crate::infra::i18n::tr;

/// 整批 `paths` 共享的输出预算，与单次 read 的后读护栏同一个数值 —— 批量只是把
/// 若干次 read 摊进一次往返，不该因此获得更大的上下文配额。
const BATCH_READ_BUDGET_BYTES: usize =
    crate::core::tools::primitive::executor::read::READ_POST_OUTPUT_BUDGET_BYTES;

/// 一次批量读里最多几个文件。上限是给模型的提示而非性能约束：
/// 3–5 个文件时整批体量落在 10–40K，既摊薄往返又不至于整批被落盘成一个引用。
const MAX_BATCH_READ_ENTRIES: usize = 10;

/// 一次读请求（单读与批量读共用）。
struct ReadSpec {
    path: String,
    offset: Option<u64>,
    limit: Option<u64>,
}

pub(in super::super) async fn handle_read(
    ctx: &ToolExecCtx<'_>,
    args: &serde_json::Value,
    display_out: &mut Option<ToolDisplay>,
) -> Result<(String, Vec<crate::core::llm::ChatMessageContentPart>), String> {
    let line_numbers = args
        .get("line_numbers")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let hashline = args
        .get("hashline")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if let Some(specs) = parse_batch_specs(args)? {
        return read_batch(ctx, specs, line_numbers, hashline, display_out).await;
    }

    let spec = ReadSpec {
        path: args["path"].as_str().unwrap_or("").to_string(),
        offset: parse_optional_u64(args, "offset"),
        limit: parse_optional_u64(args, "limit"),
    };
    validate_read_bounds(spec.offset, spec.limit)?;
    let outcome = read_one(ctx, &spec, line_numbers, hashline).await?;
    Ok((outcome.text, outcome.parts))
}

/// 解析 `paths` 批量入参。返回 `None` 表示这是一次普通单读。
///
/// 走 strict schema 的模型会把没用到的字段也填上空值，单读时照样带一个
/// `paths: [{path: ""}]`。空壳条目在这里先丢掉，让形态由内容决定。
fn parse_batch_specs(args: &serde_json::Value) -> Result<Option<Vec<ReadSpec>>, String> {
    let Some(raw) = args.get("paths") else {
        return Ok(None);
    };
    let raw = raw
        .as_array()
        .ok_or_else(|| "`paths` must be an array".to_string())?;
    let raw: Vec<&serde_json::Value> = raw
        .iter()
        .filter(|item| {
            item.get("path")
                .and_then(|v| v.as_str())
                .is_some_and(|p| !p.trim().is_empty())
        })
        .collect();
    if raw.is_empty() {
        return Ok(None);
    }
    if raw.len() > MAX_BATCH_READ_ENTRIES {
        return Err(format!(
            "A batch read allows at most {MAX_BATCH_READ_ENTRIES} files; received {}",
            raw.len()
        ));
    }
    let mut specs = Vec::with_capacity(raw.len() + 1);
    for (idx, item) in raw.iter().enumerate() {
        let path = item
            .get("path")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .ok_or_else(|| format!("paths[{idx}] requires a non-empty `path`"))?;
        let offset = parse_optional_u64(item, "offset");
        let limit = parse_optional_u64(item, "limit");
        validate_read_bounds(offset, limit).map_err(|e| format!("paths[{idx}]: {e}"))?;
        specs.push(ReadSpec {
            path: path.to_string(),
            offset,
            limit,
        });
    }

    // 顶层 `path` 与 `paths` 同时出现时不报错：模型常把批量的第一个文件顺手也填进
    // 单文件槽位。读是无副作用的，重复的就忽略、多出来的就补进去，代价是零；为此
    // 罚一次往返才是真的浪费。edit 不这么做 —— 它会落盘，两套编辑意图必须问清楚。
    if let Some(single) = args
        .get("path")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        if !specs.iter().any(|spec| spec.path == single) {
            specs.insert(
                0,
                ReadSpec {
                    path: single.to_string(),
                    offset: parse_optional_u64(args, "offset"),
                    limit: parse_optional_u64(args, "limit"),
                },
            );
        }
    }
    Ok(Some(specs))
}

/// 顺序执行整批读，装不下的条目显式标注 SKIPPED 并给出可直接复制的续读调用。
async fn read_batch(
    ctx: &ToolExecCtx<'_>,
    specs: Vec<ReadSpec>,
    line_numbers: bool,
    hashline: bool,
    display_out: &mut Option<ToolDisplay>,
) -> Result<(String, Vec<crate::core::llm::ChatMessageContentPart>), String> {
    let total = specs.len();
    let mut sections: Vec<String> = Vec::with_capacity(total);
    let mut entries: Vec<ToolDisplayFileEntry> = Vec::with_capacity(total);
    let mut parts = Vec::new();
    let mut used = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;

    for (idx, spec) in specs.iter().enumerate() {
        let position = format!("[{}/{}]", idx + 1, total);
        if used >= BATCH_READ_BUDGET_BYTES {
            skipped += 1;
            let hint = resume_hint(spec);
            sections.push(format!(
                "=== {position} {} SKIPPED: output budget exhausted\n          (resume: {hint}) ===",
                spec.path,
            ));
            entries.push(ToolDisplayFileEntry {
                file: spec.path.clone(),
                added: None,
                removed: None,
                diff: None,
                diff_truncated: false,
                expired: false,
                range: None,
                status: Some(ToolDisplayFileStatus::Skipped),
                note: Some(tr("toolRead.budget", &[("hint", &hint)])),
            });
            continue;
        }
        match read_one(ctx, spec, line_numbers, hashline).await {
            Ok(outcome) => {
                used += outcome.text.len();
                parts.extend(outcome.parts);
                sections.push(format!(
                    "=== {position} {} ===\n{}",
                    outcome.header, outcome.text
                ));
                entries.push(ToolDisplayFileEntry {
                    file: spec.path.clone(),
                    added: None,
                    removed: None,
                    diff: None,
                    diff_truncated: false,
                    expired: false,
                    range: outcome.range,
                    status: None,
                    note: outcome.note,
                });
            }
            // 单个文件读失败不该让整批失败：标出来，其余照常返回。
            Err(err) => {
                failed += 1;
                sections.push(format!("=== {position} {} FAILED: {err} ===", spec.path));
                entries.push(ToolDisplayFileEntry {
                    file: spec.path.clone(),
                    added: None,
                    removed: None,
                    diff: None,
                    diff_truncated: false,
                    expired: false,
                    range: None,
                    status: Some(ToolDisplayFileStatus::Failed),
                    note: Some(err),
                });
            }
        }
    }

    let read = total - skipped - failed;
    let key = match (skipped > 0, failed > 0) {
        (true, true) => "toolRead.withBoth",
        (true, false) => "toolRead.withSkipped",
        (false, true) => "toolRead.withFailed",
        (false, false) if read == 1 => "toolRead.readCount.one",
        (false, false) => "toolRead.readCount.other",
    };
    let summary = tr(
        key,
        &[
            ("read", &read.to_string()),
            ("skipped", &skipped.to_string()),
            ("failed", &failed.to_string()),
        ],
    );
    *display_out = Some(ToolDisplay::Files {
        summary,
        files: entries,
        expired: false,
    });

    Ok((sections.join("\n\n"), parts))
}

fn resume_hint(spec: &ReadSpec) -> String {
    let mut call = format!("read(path=\"{}\"", spec.path);
    if let Some(offset) = spec.offset {
        call.push_str(&format!(", offset={offset}"));
    }
    if let Some(limit) = spec.limit {
        call.push_str(&format!(", limit={limit}"));
    }
    call.push(')');
    call
}

struct ReadOutcome {
    /// `path L1-900 (900 lines)` 之类的定位信息，批量读用它做分段标题。
    header: String,
    /// 行号区间（仅文本），批量读卡片右侧显示这个。
    range: Option<String>,
    /// 非文本或需要额外说明时的一行注解（图片类型、截断续读、文件未变）。
    note: Option<String>,
    text: String,
    parts: Vec<crate::core::llm::ChatMessageContentPart>,
}

async fn read_one(
    ctx: &ToolExecCtx<'_>,
    spec: &ReadSpec,
    line_numbers: bool,
    hashline: bool,
) -> Result<ReadOutcome, String> {
    let path = spec.path.as_str();
    let offset = spec.offset;
    let limit = spec.limit;
    let render_mode =
        crate::core::tools::pipeline::read_state::ReadRenderMode::resolve(line_numbers, hashline);

    let resolved = crate::infra::platform::normalize_path(path).unwrap_or_else(|_| path.into());
    let stub_short_circuit = ctx.read_file_state.and_then(|state| {
        let stamp = state.get(&resolved)?;
        let meta = std::fs::metadata(&resolved).ok()?;
        if meta.is_dir() {
            return None;
        }
        let mtime = crate::core::tools::pipeline::read_state::metadata_mtime_ms(&meta);
        stamp.covers(mtime, meta.len(), offset, limit, render_mode)
    });
    if let Some((covered_start, covered_end)) = stub_short_circuit {
        // 写清覆盖关系，否则「和上次一样」对着一个更窄的窗口说，读的人无从确认。
        let coverage = format!("L{covered_start}-{covered_end}");
        return Ok(ReadOutcome {
            header: format!("{path} already covered by your earlier read of {coverage}"),
            range: Some(coverage.clone()),
            note: Some(tr("toolRead.covered", &[("coverage", &coverage)])),
            text: format!(
                "{} (earlier read covered {coverage}; to request another rendering, call read with line_numbers or hashline — do not modify the file)",
                crate::core::tools::pipeline::read_state::FILE_UNCHANGED_STUB
            ),
            parts: Vec::new(),
        });
    }

    let exec_result = ctx
        .primitive
        .read(path, offset, limit, line_numbers, hashline, AGENT_PLUGIN_ID)
        .await;

    if let (Ok(result), Some(state)) = (exec_result.as_ref(), ctx.read_file_state) {
        if let Ok(meta) = std::fs::metadata(&resolved) {
            if !meta.is_dir() {
                let hash_input: Vec<u8> = match result {
                    // `Text::content` 是准备给模型显示的文本，可能带行号等展示格式；
                    // mutation guard 的 content_hash 必须是原始文件字节，才能在 mtime
                    // 变动时可靠判断内容是否仍相同。
                    crate::core::tools::primitive::ReadResult::Text(t) => {
                        std::fs::read(&resolved).unwrap_or_else(|_| t.content.as_bytes().to_vec())
                    }
                    crate::core::tools::primitive::ReadResult::Image(b)
                    | crate::core::tools::primitive::ReadResult::Pdf(b) => {
                        b.path.as_os_str().as_encoded_bytes().to_vec()
                    }
                    crate::core::tools::primitive::ReadResult::FileUnchanged { .. } => Vec::new(),
                };
                let (covered_lines, reached_eof) = match result {
                    crate::core::tools::primitive::ReadResult::Text(t) => (
                        Some((t.start_line, t.start_line + t.num_lines.saturating_sub(1))),
                        !t.truncated,
                    ),
                    _ => (None, false),
                };
                let stamp = crate::core::tools::pipeline::read_state::ReadStamp {
                    mtime_ms: crate::core::tools::pipeline::read_state::metadata_mtime_ms(&meta),
                    size: meta.len(),
                    content_hash: crate::core::tools::pipeline::read_state::hash_content(
                        &hash_input,
                    ),
                    offset,
                    limit,
                    is_partial_view: offset.is_some() || limit.is_some(),
                    render_mode,
                    covered_lines,
                    reached_eof,
                    tool_call_id: Some(ctx.tool_call_id.to_string()),
                };
                state.put(resolved.clone(), stamp);
            }
        }
    }

    match exec_result {
        Ok(result) => {
            let mut follow_up_parts = Vec::new();
            match &result {
                crate::core::tools::primitive::ReadResult::Image(b) => {
                    follow_up_parts.push(
                        crate::core::llm::ChatMessageContentPart::image_b64(
                            b.mime.clone(),
                            &b.path,
                        )
                        .map_err(|error| {
                            format!("Read image cannot be attached inline: {error}")
                        })?,
                    );
                }
                crate::core::tools::primitive::ReadResult::Pdf(b) => {
                    follow_up_parts.push(
                        crate::core::llm::ChatMessageContentPart::file_b64(
                            b.filename.clone(),
                            b.mime.clone(),
                            &b.path,
                        )
                        .map_err(|error| format!("Read file cannot be attached inline: {error}"))?,
                    );
                }
                crate::core::tools::primitive::ReadResult::Text(_)
                | crate::core::tools::primitive::ReadResult::FileUnchanged { .. } => {}
            }
            let (range, note) = render_locator(path, &result);
            Ok(ReadOutcome {
                header: render_header(path, &result),
                range,
                note,
                text: result.to_tool_text(),
                parts: follow_up_parts,
            })
        }
        Err(e) => Err(e.to_string()),
    }
}

/// 卡片上这一行的定位信息：`(行号区间, 注解)`。文本文件给区间，图片/PDF 给类型注解。
fn render_locator(
    path: &str,
    result: &crate::core::tools::primitive::ReadResult,
) -> (Option<String>, Option<String>) {
    match result {
        crate::core::tools::primitive::ReadResult::Text(t) => {
            let end = t.start_line + t.num_lines.saturating_sub(1);
            let range = tr(
                if t.num_lines == 1 {
                    "toolRead.range.one"
                } else {
                    "toolRead.range.other"
                },
                &[
                    ("start", &t.start_line.to_string()),
                    ("end", &end.to_string()),
                    ("count", &t.num_lines.to_string()),
                ],
            );
            let note = t.truncated.then(|| {
                tr(
                    "toolRead.resume",
                    &[("path", path), ("offset", &(end + 1).to_string())],
                )
            });
            (Some(range), note)
        }
        crate::core::tools::primitive::ReadResult::Image(b) => {
            (None, Some(tr("toolRead.image", &[("mime", &b.mime)])))
        }
        crate::core::tools::primitive::ReadResult::Pdf(b) => {
            (None, Some(format!("pdf {}", b.mime)))
        }
        crate::core::tools::primitive::ReadResult::FileUnchanged { .. } => {
            (None, Some(tr("toolRead.unchanged", &[])))
        }
    }
}

/// 批量读的分段标题：告诉模型这一段来自哪个文件的哪一段，以及后面还剩多少没读。
fn render_header(path: &str, result: &crate::core::tools::primitive::ReadResult) -> String {
    match result {
        crate::core::tools::primitive::ReadResult::Text(t) => {
            let end = t.start_line + t.num_lines.saturating_sub(1);
            let mut header = format!("{path}  L{}-{end} ({} lines)", t.start_line, t.num_lines);
            if t.truncated {
                header.push_str(&format!(
                    " truncated, resume: read(path=\"{path}\", offset={})",
                    end + 1
                ));
            }
            header
        }
        crate::core::tools::primitive::ReadResult::Image(b) => format!("{path}  image {}", b.mime),
        crate::core::tools::primitive::ReadResult::Pdf(b) => format!("{path}  pdf {}", b.mime),
        crate::core::tools::primitive::ReadResult::FileUnchanged { .. } => {
            format!("{path} UNCHANGED since your last read")
        }
    }
}
