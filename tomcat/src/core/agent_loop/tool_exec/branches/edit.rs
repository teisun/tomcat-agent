use super::super::args::{has_real_edits, parse_edit_args, parse_edit_ops};
use super::super::edit_sim::simulate_apply_edits;
use super::super::guard::{check_mutation_stamp, refresh_read_stamp};
use super::super::{ToolDisplay, ToolExecCtx, AGENT_PLUGIN_ID};
use crate::core::tools::primitive::{
    DiffTag, EditOperation, FileDiffLine, EDIT_INSERT_AFTER_MARKER, EDIT_INSERT_BEFORE_MARKER,
    EDIT_REPLACE_ALL_MARKER,
};
use crate::infra::events::{ToolDisplayFileEntry, ToolDisplayFileStatus};
use crate::infra::i18n::tr;

/// 一次批量 edit 最多几个文件。上限存在的理由是可读性：一屏能看完的失败列表才有人会看。
const MAX_BATCH_EDIT_FILES: usize = 10;
/// 每次编辑后给模型的真实磁盘视图保持足够小，避免修复漂移本身制造上下文膨胀。
const EDIT_FEEDBACK_MAX_BYTES: usize = 6 * 1024;
const BATCH_EDIT_FEEDBACK_MAX_BYTES: usize = 8 * 1024;
const EDIT_FEEDBACK_CONTEXT_LINES: usize = 3;
const EDIT_FEEDBACK_MAX_LINES: usize = 36;
const EDIT_FEEDBACK_MAX_LINE_BYTES: usize = 240;

pub(in super::super) async fn handle_edit(
    ctx: &ToolExecCtx<'_>,
    args: &serde_json::Value,
    display_out: &mut Option<ToolDisplay>,
) -> Result<String, String> {
    if let Some(files) = parse_batch_files(args)? {
        return edit_batch(ctx, files, display_out).await;
    }

    let (path, edits) = parse_edit_args(args)?;
    precheck_file(ctx, path)?;
    reviewer_body_guard(ctx, path, &edits)?;
    let heading_notice = heading_replacement_notice(&edits);
    let feedback_edits = edits.clone();

    let baseline = ctx.file_baselines.and_then(|b| b.prepare(path));
    match ctx
        .primitive
        .edit_file_with_cancel(path, edits, ctx.cancel, AGENT_PLUGIN_ID)
        .await
    {
        Ok(result) if result.applied => {
            if let Some(baseline) = baseline {
                baseline.commit();
            }
            if let Some(state) = ctx.read_file_state {
                refresh_read_stamp(state, path, ctx.tool_call_id);
            }
            let feedback = render_post_edit_feedback(
                ctx,
                &result.path,
                result.diff.as_deref(),
                &feedback_edits,
                EDIT_FEEDBACK_MAX_BYTES,
            )
            .await;
            *display_out = Some(ToolDisplay::File {
                file: result.path.clone(),
                added: result.added,
                removed: result.removed,
                diff: result.diff,
                diff_truncated: result.diff_truncated,
                expired: false,
            });
            let mut message = format!("Edited: {}", result.path);
            if let Some(notice) = heading_notice {
                message.push_str(&format!("\nNote: {notice}"));
            }
            if let Some(feedback) = feedback {
                message.push_str(&feedback);
            }
            Ok(message)
        }
        Ok(result) => {
            *display_out = Some(ToolDisplay::Text {
                text: tr("toolEdit.denied", &[("path", &result.path)]),
            });
            Ok(format!("Edit denied: {}", result.path))
        }
        Err(error) => {
            Err(enrich_notfound_error(ctx, path, &feedback_edits, error.to_string()).await)
        }
    }
}

/// 只把刚变动的局部真相回喂给模型。视图从编辑成功后再次经原语读取得到，而不是从
/// 模型输入或 UI diff 猜出：这样行号和文本都对应当前磁盘。读权限若不允许，不影响
/// 已经成功的编辑，只省略这段辅助信息。
async fn render_post_edit_feedback(
    ctx: &ToolExecCtx<'_>,
    path: &str,
    diff: Option<&[FileDiffLine]>,
    edits: &[EditOperation],
    max_bytes: usize,
) -> Option<String> {
    let current = ctx.primitive.read_file(path, AGENT_PLUGIN_ID).await.ok()?;
    let total_lines = current.lines().count().max(1);
    let ranges = changed_line_ranges(diff, total_lines)
        .filter(|ranges| !ranges.is_empty())
        .unwrap_or_else(|| fallback_changed_line_ranges(&current, edits));
    (!ranges.is_empty()).then(|| {
        render_current_line_ranges(
            &current,
            &ranges,
            "Post-edit view (current disk; use this text for the next old_content)",
            max_bytes,
        )
    })
}

/// 成功编辑后的结果已有 pre/post diff；它是定位“本次改动”的最可靠且零猜测来源。
/// 删除行本身没有 `new_line`，就取相邻仍存在的行作为窗口中心。
fn changed_line_ranges(
    diff: Option<&[FileDiffLine]>,
    total_lines: usize,
) -> Option<Vec<(usize, usize)>> {
    let diff = diff?;
    let mut ranges = Vec::new();
    for (index, line) in diff.iter().enumerate() {
        if line.tag == DiffTag::Ctx {
            continue;
        }
        let line_no = line
            .new_line
            .map(|line| line as usize)
            .or_else(|| {
                diff[index + 1..]
                    .iter()
                    .find_map(|line| line.new_line.map(|line| line as usize))
            })
            .or_else(|| {
                diff[..index]
                    .iter()
                    .rev()
                    .find_map(|line| line.new_line.map(|line| line as usize))
            })
            .unwrap_or(total_lines);
        ranges.push(expand_line_range(line_no, line_no, total_lines));
    }
    Some(merge_line_ranges(ranges))
}

/// 超大文件的 diff 为避免内存放大可能缺席；此时仅以刚写入的新文本作保守定位。
/// 不能唯一定位（例如纯删除）时宁可不伪造行号，让模型显式 read。
fn fallback_changed_line_ranges(current: &str, edits: &[EditOperation]) -> Vec<(usize, usize)> {
    let current_lf = current.replace("\r\n", "\n");
    let total_lines = current_lf.lines().count().max(1);
    let mut ranges = Vec::new();
    for edit in edits {
        let inserted = edit.new_content.replace("\r\n", "\n");
        if inserted.is_empty() {
            continue;
        }
        let mut hits = current_lf.match_indices(&inserted);
        let Some((byte_offset, _)) = hits.next() else {
            continue;
        };
        if hits.next().is_some() {
            continue;
        }
        let start = current_lf.as_bytes()[..byte_offset]
            .iter()
            .filter(|&&byte| byte == b'\n')
            .count()
            + 1;
        let end = start + inserted.lines().count().saturating_sub(1);
        ranges.push(expand_line_range(start, end, total_lines));
    }
    merge_line_ranges(ranges)
}

fn expand_line_range(start: usize, end: usize, total_lines: usize) -> (usize, usize) {
    (
        start.saturating_sub(EDIT_FEEDBACK_CONTEXT_LINES).max(1),
        end.saturating_add(EDIT_FEEDBACK_CONTEXT_LINES)
            .min(total_lines),
    )
}

fn merge_line_ranges(mut ranges: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    ranges.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (start, end) in ranges {
        match merged.last_mut() {
            Some((_, previous_end)) if start <= previous_end.saturating_add(1) => {
                *previous_end = (*previous_end).max(end);
            }
            _ => merged.push((start, end)),
        }
    }
    merged
}

fn render_current_line_ranges(
    current: &str,
    ranges: &[(usize, usize)],
    title: &str,
    max_bytes: usize,
) -> String {
    let lines: Vec<&str> = current.lines().collect();
    let total_lines = lines.len().max(1);
    let mut rendered = format!("\n{title}:\n");
    let mut rendered_lines = 0usize;
    let mut truncated = false;

    for (range_index, (start, end)) in ranges.iter().enumerate() {
        if range_index > 0 && !push_feedback_fragment(&mut rendered, "       …\n", max_bytes) {
            truncated = true;
            break;
        }
        for line_no in *start..=*end {
            if rendered_lines >= EDIT_FEEDBACK_MAX_LINES {
                truncated = true;
                break;
            }
            let line = lines.get(line_no.saturating_sub(1)).copied().unwrap_or("");
            let text = truncate_feedback_line(line);
            let fragment = format!("{line_no:>6}\t{text}\n");
            if !push_feedback_fragment(&mut rendered, &fragment, max_bytes) {
                truncated = true;
                break;
            }
            rendered_lines += 1;
        }
        if truncated {
            break;
        }
    }

    if rendered_lines == 0 && total_lines == 1 && current.is_empty() {
        let _ = push_feedback_fragment(&mut rendered, "     1\t<file is now empty>\n", max_bytes);
    }
    if truncated {
        let _ = push_feedback_fragment(
            &mut rendered,
            "       …(post-edit view truncated; use read for more context)\n",
            max_bytes,
        );
    }
    rendered
}

fn push_feedback_fragment(output: &mut String, fragment: &str, max_bytes: usize) -> bool {
    if output.len().saturating_add(fragment.len()) > max_bytes {
        return false;
    }
    output.push_str(fragment);
    true
}

fn truncate_feedback_line(line: &str) -> String {
    if line.len() <= EDIT_FEEDBACK_MAX_LINE_BYTES {
        return line.to_string();
    }
    let mut end = EDIT_FEEDBACK_MAX_LINE_BYTES.saturating_sub('…'.len_utf8());
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &line[..end])
}

async fn enrich_notfound_error(
    ctx: &ToolExecCtx<'_>,
    path: &str,
    edits: &[EditOperation],
    error: String,
) -> String {
    if !error.contains("NotFound:") && !error.contains("NotFound (") {
        return error;
    }
    let Some(edit) = notfound_edit_index(&error)
        .and_then(|index| edits.get(index))
        .or_else(|| edits.first())
    else {
        return error;
    };
    let Ok(current) = ctx.primitive.read_file(path, AGENT_PLUGIN_ID).await else {
        return format!("{error}\nCould not read the current file to locate the target; read it again before editing.");
    };
    let Some(line_no) = nearest_current_line(&current, edit) else {
        return format!("{error}\nCould not reliably locate the current area from old_content; read the file again before editing.");
    };
    let total_lines = current.lines().count().max(1);
    let view = render_current_line_ranges(
        &current,
        &[expand_line_range(line_no, line_no, total_lines)],
        "Nearby current view for old_content (current disk; use this to correct the edit)",
        EDIT_FEEDBACK_MAX_BYTES,
    );
    format!("{error}\n{view}")
}

fn notfound_edit_index(error: &str) -> Option<usize> {
    let start = error.find("edits[")? + "edits[".len();
    let end = start + error[start..].find(']')?;
    error[start..end].parse().ok()
}

/// 从失败的 old_content 中选最长且在当前文件唯一的一行。它只是为回喂真相定位，
/// 不参与写入和匹配判定，因此“找不到就要求 read”比猜一个位置更安全。
fn nearest_current_line(current: &str, edit: &EditOperation) -> Option<usize> {
    let old = edit.old_content.as_deref()?;
    let old = old
        .strip_prefix(EDIT_REPLACE_ALL_MARKER)
        .unwrap_or(old)
        .strip_prefix(EDIT_INSERT_BEFORE_MARKER)
        .or_else(|| old.strip_prefix(EDIT_INSERT_AFTER_MARKER))
        .unwrap_or(old);
    let mut candidates: Vec<String> = old
        .lines()
        .map(crate::core::tools::pipeline::edit_normalize::normalize_for_match)
        .filter(|line| !line.trim().is_empty())
        .collect();
    candidates.sort_unstable_by_key(|line| std::cmp::Reverse(line.len()));

    for candidate in candidates {
        let hits: Vec<usize> = current
            .lines()
            .enumerate()
            .filter_map(|(index, line)| {
                crate::core::tools::pipeline::edit_normalize::normalize_for_match(line)
                    .contains(&candidate)
                    .then_some(index + 1)
            })
            .collect();
        if hits.len() == 1 {
            return hits.first().copied();
        }
    }
    None
}

struct BatchFile {
    path: String,
    edits: Vec<EditOperation>,
}

fn heading_replacement_notice(edits: &[EditOperation]) -> Option<String> {
    edits.iter().find_map(|edit| {
        let raw_old = edit.old_content.as_deref()?;
        let old = raw_old
            .strip_prefix(EDIT_REPLACE_ALL_MARKER)
            .unwrap_or(raw_old);
        if old.starts_with(EDIT_INSERT_BEFORE_MARKER) || old.starts_with(EDIT_INSERT_AFTER_MARKER)
        {
            return None;
        }
        if old.contains(['\n', '\r'])
            || !is_single_line_markdown_heading(old)
            || edit.new_content.contains(old)
        {
            return None;
        }
        Some(format!("This replace removed Markdown heading `{old}`; if insertion was intended, use mode=`insert_before`."))
    })
}

fn is_single_line_markdown_heading(text: &str) -> bool {
    let hashes = text.bytes().take_while(|byte| *byte == b'#').count();
    (1..=6).contains(&hashes)
        && text
            .as_bytes()
            .get(hashes)
            .is_some_and(|byte| byte.is_ascii_whitespace())
}

/// 顶层 `path`/`edits` 里是否还有一段 `files` 中不存在的真实编辑。
///
/// 有才是真歧义。实测模型在批量编辑时会把整批编辑**原样抄一份**放进顶层槽位（`path`
/// 填第一个文件），那是同一个意图写了两遍，不是两套意图 —— 按歧义拒掉只是白白罚一次
/// 往返。反过来，顶层真有一段别处没有的编辑，就必须问清楚：edit 会落盘，猜错要写坏文件。
fn top_level_edit_outside_files(
    args: &serde_json::Value,
    files: &[&serde_json::Value],
) -> Option<String> {
    let known: std::collections::BTreeSet<(String, String, String)> = files
        .iter()
        .flat_map(|file| super::super::args::edit_segments(file))
        .map(super::super::args::segment_identity)
        .collect();

    super::super::args::edit_segments(args)
        .into_iter()
        .map(super::super::args::segment_identity)
        .find(|identity| identity.1 != identity.2 && !known.contains(identity))
        .map(|(_, old, _)| old)
}

/// `files` 里的空壳条目：既没有路径也没有编辑内容。
fn is_placeholder_file(item: &serde_json::Value) -> bool {
    let no_path = item
        .get("path")
        .and_then(|v| v.as_str())
        .is_none_or(|p| p.trim().is_empty());
    no_path && !has_real_edits(item)
}

/// 解析 Shape C `{ files: [{ path, edits }] }`。返回 `None` 表示这是单文件调用。
///
/// 形态由「内容在哪」决定，不由「哪个键出现过」决定：strict schema 下模型会把没用到
/// 的那一套字段也填上空值，两边都按「出现即选中」来判，任何一种形态都过不了互斥校验。
fn parse_batch_files(args: &serde_json::Value) -> Result<Option<Vec<BatchFile>>, String> {
    let Some(raw) = args.get("files") else {
        return Ok(None);
    };
    let raw = raw
        .as_array()
        .ok_or_else(|| "edit: `files` must be an array".to_string())?;
    let raw: Vec<&serde_json::Value> = raw
        .iter()
        .filter(|item| !is_placeholder_file(item))
        .collect();
    if raw.is_empty() {
        // 空壳 `files`：这其实是一次单文件调用，交给下面的 Shape A/B 分支。
        return Ok(None);
    }
    if raw.len() > MAX_BATCH_EDIT_FILES {
        return Err(format!(
            "edit: at most {MAX_BATCH_EDIT_FILES} files may be edited in one batch; received {}",
            raw.len()
        ));
    }
    if let Some(conflict) = top_level_edit_outside_files(args, &raw) {
        return Err(format!("edit: a top-level edit (changing `{}`) is not present in `files`. For multiple files, use only `files` and include that edit there", conflict.chars().take(40).collect::<String>()));
    }

    let mut files = Vec::with_capacity(raw.len());
    let mut seen = std::collections::BTreeSet::new();
    for (idx, item) in raw.iter().enumerate() {
        let path = item
            .get("path")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .ok_or_else(|| format!("edit: files[{idx}] requires a non-empty `path`"))?;
        // 同一文件出现两次时，第二条看到的是第一条改完的内容，old_content 多半对不上；
        // 与其让它以 NotFound 失败，不如直接要求合并。
        if !seen.insert(path.to_string()) {
            return Err(format!("edit: `{path}` appears more than once in files; merge its edit segments into one entry"));
        }
        let edits = parse_edit_ops(item, &format!("files[{idx}]"))?;
        files.push(BatchFile {
            path: path.to_string(),
            edits,
        });
    }
    Ok(Some(files))
}

/// 三段式：先全量预检（不提前退出，把所有问题一次报全），再逐文件落盘（文件内原子），
/// 最后逐文件回报并显式说明磁盘现在是什么状态。
async fn edit_batch(
    ctx: &ToolExecCtx<'_>,
    files: Vec<BatchFile>,
    display_out: &mut Option<ToolDisplay>,
) -> Result<String, String> {
    // 阶段 1：全量预检。一个文件不通过不影响其余文件继续被检查。
    let mut precheck_errors: std::collections::BTreeMap<usize, String> =
        std::collections::BTreeMap::new();
    for (idx, file) in files.iter().enumerate() {
        if let Err(err) = precheck_file(ctx, &file.path)
            .and_then(|()| reviewer_body_guard(ctx, &file.path, &file.edits))
        {
            precheck_errors.insert(idx, err);
        }
    }

    // 阶段 2：只落盘预检通过的文件。primitive 内部对单个文件是先算后写的，
    // 所以某个文件即便在这里失败，它自己也不会留下半截内容。
    let mut entries: Vec<ToolDisplayFileEntry> = Vec::with_capacity(files.len());
    let mut feedback = String::new();
    for (idx, file) in files.into_iter().enumerate() {
        if let Some(err) = precheck_errors.remove(&idx) {
            entries.push(failed_entry(file.path, err));
            continue;
        }
        let heading_notice = heading_replacement_notice(&file.edits);
        let feedback_edits = file.edits.clone();
        let baseline = ctx.file_baselines.and_then(|b| b.prepare(&file.path));
        match ctx
            .primitive
            .edit_file_with_cancel(&file.path, file.edits, ctx.cancel, AGENT_PLUGIN_ID)
            .await
        {
            Ok(result) if result.applied => {
                if let Some(baseline) = baseline {
                    baseline.commit();
                }
                if let Some(state) = ctx.read_file_state {
                    refresh_read_stamp(state, &file.path, ctx.tool_call_id);
                }
                let remaining_feedback =
                    BATCH_EDIT_FEEDBACK_MAX_BYTES.saturating_sub(feedback.len());
                if remaining_feedback > 0 {
                    if let Some(view) = render_post_edit_feedback(
                        ctx,
                        &result.path,
                        result.diff.as_deref(),
                        &feedback_edits,
                        remaining_feedback,
                    )
                    .await
                    {
                        feedback.push_str(&view);
                    }
                }
                entries.push(ToolDisplayFileEntry {
                    file: result.path,
                    added: result.added,
                    removed: result.removed,
                    diff: result.diff,
                    diff_truncated: result.diff_truncated,
                    expired: false,
                    range: None,
                    status: Some(ToolDisplayFileStatus::Applied),
                    note: heading_notice,
                });
            }
            Ok(result) => entries.push(failed_entry(result.path, "Edit denied".to_string())),
            Err(err) => entries.push(failed_entry(
                file.path.clone(),
                enrich_notfound_error(ctx, &file.path, &feedback_edits, err.to_string()).await,
            )),
        }
    }

    // 阶段 3：逐文件回报。磁盘状态必须写死在文案里 —— 批量最容易误解的就是
    // 「报了错，那是不是一个都没改？」
    let applied = entries
        .iter()
        .filter(|e| e.status == Some(ToolDisplayFileStatus::Applied))
        .count();
    let failed = entries.len() - applied;
    let summary = if failed == 0 {
        tr(
            if applied == 1 {
                "toolEdit.allApplied.one"
            } else {
                "toolEdit.allApplied.other"
            },
            &[("count", &applied.to_string())],
        )
    } else {
        tr(
            "toolEdit.partial",
            &[
                ("applied", &applied.to_string()),
                ("failed", &failed.to_string()),
            ],
        )
    };

    // UI status follows the user's locale; model feedback is a fixed English contract.
    let mut text = if failed == 0 {
        format!(
            "Edited {applied} {}; all changes written",
            if applied == 1 { "file" } else { "files" }
        )
    } else {
        format!("Files written: {applied}; failed without writing: {failed}. Failed files retain their original disk contents")
    };
    for entry in &entries {
        if entry.status == Some(ToolDisplayFileStatus::Applied) {
            let mut line = format!(
                "\n- APPLIED {} (+{} -{})",
                entry.file,
                entry.added.unwrap_or(0),
                entry.removed.unwrap_or(0)
            );
            if let Some(notice) = entry.note.as_deref() {
                line.push_str(&format!("\n  Note: {notice}"));
            }
            text.push_str(&line);
        } else {
            text.push_str(&format!(
                "\n- FAILED  {}: {}",
                entry.file,
                entry.note.as_deref().unwrap_or("Unknown error")
            ));
        }
    }
    if !feedback.is_empty() {
        text.push_str(&feedback);
    }

    *display_out = Some(display_for_entries(summary, entries));
    Ok(text)
}

fn display_for_entries(summary: String, mut entries: Vec<ToolDisplayFileEntry>) -> ToolDisplay {
    if entries.len() == 1 {
        let entry = entries.pop().expect("len checked");
        return ToolDisplay::File {
            file: entry.file,
            added: entry.added,
            removed: entry.removed,
            diff: entry.diff,
            diff_truncated: entry.diff_truncated,
            expired: entry.expired,
        };
    }
    ToolDisplay::Files {
        summary,
        files: entries,
        expired: false,
    }
}

fn failed_entry(file: String, error: String) -> ToolDisplayFileEntry {
    ToolDisplayFileEntry {
        file,
        added: None,
        removed: None,
        diff: None,
        diff_truncated: false,
        expired: false,
        range: None,
        status: Some(ToolDisplayFileStatus::Failed),
        note: Some(error),
    }
}

fn precheck_file(ctx: &ToolExecCtx<'_>, path: &str) -> Result<(), String> {
    if crate::core::tools::pipeline::edit_normalize::is_unsupported_structured_file(path) {
        return Err(format!("Notebook: `{path}` is a Jupyter notebook (.ipynb), which edit does not support; use an nbformat tool or export the target cell to .py / .md first"));
    }
    if let Some(state) = ctx.read_file_state {
        check_mutation_stamp(state, path, "edit")?;
    }
    Ok(())
}

fn reviewer_body_guard(
    ctx: &ToolExecCtx<'_>,
    path: &str,
    edits: &[EditOperation],
) -> Result<(), String> {
    if ctx.subagent_type != crate::core::agent_loop::types::SubagentType::PlanReviewer {
        return Ok(());
    }
    let normalized_path = crate::infra::platform::normalize_path(path)
        .map_err(|e| format!("reviewer edit path precheck failed: {e}"))?;
    let old = std::fs::read_to_string(&normalized_path)
        .map_err(|e| format!("reviewer edit source precheck failed: {e}"))?;
    let new = simulate_apply_edits(&old, edits);
    crate::core::plan_runtime::safety::reviewer_body_diff_guard(&old, &new)
        .map_err(|denied| format!("reviewer edit denied: {denied}"))
}
