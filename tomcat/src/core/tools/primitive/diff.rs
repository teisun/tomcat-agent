use std::borrow::Cow;

use super::{DiffTag, FileDiffLine};

/// Guard the quadratic LCS calculation itself. This is intentionally looser than the
/// render budget below: a medium-sized file still gets a useful compact diff.
pub(crate) const MAX_DIFF_INPUT_LINES: usize = 8_000;
pub(crate) const MAX_DIFF_INPUT_BYTES: usize = 1_500_000;
/// `ToolDisplay` is persisted and sent over stdio, so its budget must be much smaller
/// than the source file. Three context lines on either side are enough to orient a reader.
const DIFF_CONTEXT_LINES: usize = 3;
const MAX_DIFF_RENDERED_LINES: usize = 2_000;
const MAX_DIFF_RENDERED_BYTES: usize = 200 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LineDiff {
    pub lines: Option<Vec<FileDiffLine>>,
    pub truncated: bool,
}

fn normalize_text(text: &str) -> Cow<'_, str> {
    if text.contains("\r\n") {
        Cow::Owned(text.replace("\r\n", "\n"))
    } else {
        Cow::Borrowed(text)
    }
}

fn split_logical_lines(text: &str) -> Vec<String> {
    let normalized = normalize_text(text);
    if normalized.is_empty() {
        return vec![];
    }
    let without_terminal_newline = normalized.strip_suffix('\n').unwrap_or(&normalized);
    if without_terminal_newline.is_empty() {
        return vec![String::new()];
    }
    without_terminal_newline
        .split('\n')
        .map(ToOwned::to_owned)
        .collect()
}

fn lcs_prefix_lengths(left: &[String], right: &[String]) -> Vec<usize> {
    let mut previous = vec![0usize; right.len() + 1];
    let mut current = vec![0usize; right.len() + 1];

    for left_line in left {
        for (right_index, right_line) in right.iter().enumerate() {
            current[right_index + 1] = if left_line == right_line {
                previous[right_index] + 1
            } else {
                previous[right_index + 1].max(current[right_index])
            };
        }
        previous.copy_from_slice(&current);
        current.fill(0);
    }

    previous
}

fn lcs_suffix_lengths(left: &[String], right: &[String]) -> Vec<usize> {
    let mut previous = vec![0usize; right.len() + 1];
    let mut current = vec![0usize; right.len() + 1];

    for left_line in left.iter().rev() {
        for right_index in (0..right.len()).rev() {
            current[right_index] = if left_line == &right[right_index] {
                previous[right_index + 1] + 1
            } else {
                previous[right_index].max(current[right_index + 1])
            };
        }
        previous.copy_from_slice(&current);
        current.fill(0);
    }

    previous
}

fn longest_common_subsequence_length(left: &[String], right: &[String]) -> usize {
    lcs_prefix_lengths(left, right).last().copied().unwrap_or(0)
}

fn to_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

fn push_ctx(out: &mut Vec<FileDiffLine>, old_line: usize, new_line: usize, text: &str) {
    out.push(FileDiffLine {
        tag: DiffTag::Ctx,
        old_line: Some(to_u32(old_line)),
        new_line: Some(to_u32(new_line)),
        skipped_lines: None,
        text: text.to_string(),
    });
}

fn push_add(out: &mut Vec<FileDiffLine>, new_line: usize, text: &str) {
    out.push(FileDiffLine {
        tag: DiffTag::Add,
        old_line: None,
        new_line: Some(to_u32(new_line)),
        skipped_lines: None,
        text: text.to_string(),
    });
}

fn push_del(out: &mut Vec<FileDiffLine>, old_line: usize, text: &str) {
    out.push(FileDiffLine {
        tag: DiffTag::Del,
        old_line: Some(to_u32(old_line)),
        new_line: None,
        skipped_lines: None,
        text: text.to_string(),
    });
}

fn push_gap(out: &mut Vec<FileDiffLine>, skipped_lines: usize) {
    if skipped_lines == 0 {
        return;
    }
    out.push(FileDiffLine {
        tag: DiffTag::Gap,
        old_line: None,
        new_line: None,
        skipped_lines: Some(to_u32(skipped_lines)),
        text: format!("{skipped_lines} unmodified lines"),
    });
}

fn diff_single_left(
    left: &[String],
    right: &[String],
    old_base: usize,
    new_base: usize,
    out: &mut Vec<FileDiffLine>,
) {
    let needle = &left[0];
    if let Some(match_index) = right.iter().position(|line| line == needle) {
        for (index, line) in right.iter().take(match_index).enumerate() {
            push_add(out, new_base + index + 1, line);
        }
        push_ctx(out, old_base + 1, new_base + match_index + 1, needle);
        for (index, line) in right.iter().skip(match_index + 1).enumerate() {
            push_add(out, new_base + match_index + index + 2, line);
        }
        return;
    }

    push_del(out, old_base + 1, needle);
    for (index, line) in right.iter().enumerate() {
        push_add(out, new_base + index + 1, line);
    }
}

fn diff_single_right(
    left: &[String],
    right: &[String],
    old_base: usize,
    new_base: usize,
    out: &mut Vec<FileDiffLine>,
) {
    let needle = &right[0];
    if let Some(match_index) = left.iter().position(|line| line == needle) {
        for (index, line) in left.iter().take(match_index).enumerate() {
            push_del(out, old_base + index + 1, line);
        }
        push_ctx(out, old_base + match_index + 1, new_base + 1, needle);
        for (index, line) in left.iter().skip(match_index + 1).enumerate() {
            push_del(out, old_base + match_index + index + 2, line);
        }
        return;
    }

    for (index, line) in left.iter().enumerate() {
        push_del(out, old_base + index + 1, line);
    }
    push_add(out, new_base + 1, needle);
}

fn build_line_diff_recursive(
    left: &[String],
    right: &[String],
    old_base: usize,
    new_base: usize,
    out: &mut Vec<FileDiffLine>,
) {
    if left.is_empty() {
        for (index, line) in right.iter().enumerate() {
            push_add(out, new_base + index + 1, line);
        }
        return;
    }

    if right.is_empty() {
        for (index, line) in left.iter().enumerate() {
            push_del(out, old_base + index + 1, line);
        }
        return;
    }

    if left.len() == 1 {
        diff_single_left(left, right, old_base, new_base, out);
        return;
    }

    if right.len() == 1 {
        diff_single_right(left, right, old_base, new_base, out);
        return;
    }

    let mid = left.len() / 2;
    let left_prefix = lcs_prefix_lengths(&left[..mid], right);
    let left_suffix = lcs_suffix_lengths(&left[mid..], right);

    let mut split = 0usize;
    let mut best = 0usize;
    for right_index in 0..=right.len() {
        let score = left_prefix[right_index] + left_suffix[right_index];
        if score > best {
            best = score;
            split = right_index;
        }
    }

    build_line_diff_recursive(&left[..mid], &right[..split], old_base, new_base, out);
    build_line_diff_recursive(
        &left[mid..],
        &right[split..],
        old_base + mid,
        new_base + split,
        out,
    );
}

fn compact_context(full: Vec<FileDiffLine>) -> Vec<FileDiffLine> {
    let mut keep = vec![false; full.len()];
    for (index, line) in full.iter().enumerate() {
        if line.tag == DiffTag::Ctx {
            continue;
        }
        let start = index.saturating_sub(DIFF_CONTEXT_LINES);
        let end = (index + DIFF_CONTEXT_LINES + 1).min(full.len());
        for value in &mut keep[start..end] {
            *value = true;
        }
    }

    let mut compact = Vec::with_capacity(full.len().min(128));
    let mut skipped = 0usize;
    for (line, keep_line) in full.into_iter().zip(keep) {
        if keep_line {
            push_gap(&mut compact, skipped);
            skipped = 0;
            compact.push(line);
        } else {
            debug_assert_eq!(line.tag, DiffTag::Ctx);
            skipped += 1;
        }
    }
    push_gap(&mut compact, skipped);
    compact
}

fn line_cost(line: &FileDiffLine) -> usize {
    // JSON punctuation and optional numeric fields. This is a conservative bound and
    // avoids serializing the vector once just to measure it.
    48 + line.text.len()
}

fn bound_rendered_diff(lines: Vec<FileDiffLine>) -> LineDiff {
    let mut bounded = Vec::with_capacity(lines.len().min(MAX_DIFF_RENDERED_LINES));
    let mut bytes = 0usize;
    for line in lines {
        let cost = line_cost(&line);
        if bounded.len() == MAX_DIFF_RENDERED_LINES
            || bytes.saturating_add(cost) > MAX_DIFF_RENDERED_BYTES
        {
            return LineDiff {
                lines: Some(bounded),
                truncated: true,
            };
        }
        bytes += cost;
        bounded.push(line);
    }
    LineDiff {
        lines: Some(bounded),
        truncated: false,
    }
}

pub(crate) fn build_line_diff(old: &str, new: &str) -> LineDiff {
    if old.len().saturating_add(new.len()) > MAX_DIFF_INPUT_BYTES {
        return LineDiff {
            lines: None,
            truncated: true,
        };
    }
    if normalize_text(old) == normalize_text(new) {
        return LineDiff {
            lines: Some(Vec::new()),
            truncated: false,
        };
    }
    let old_lines = split_logical_lines(old);
    let new_lines = split_logical_lines(new);
    if old_lines.len().max(new_lines.len()) > MAX_DIFF_INPUT_LINES {
        return LineDiff {
            lines: None,
            truncated: true,
        };
    }

    let mut diff = Vec::with_capacity(old_lines.len().max(new_lines.len()));
    build_line_diff_recursive(&old_lines, &new_lines, 0, 0, &mut diff);
    bound_rendered_diff(compact_context(diff))
}

pub(crate) fn line_diff_stat(old: &str, new: &str) -> (u32, u32) {
    if normalize_text(old) == normalize_text(new) {
        return (0, 0);
    }

    let old_lines = split_logical_lines(old);
    let new_lines = split_logical_lines(new);
    let shared_line_count = longest_common_subsequence_length(&old_lines, &new_lines);

    let added = new_lines.len().saturating_sub(shared_line_count);
    let removed = old_lines.len().saturating_sub(shared_line_count);
    (
        u32::try_from(added).unwrap_or(u32::MAX),
        u32::try_from(removed).unwrap_or(u32::MAX),
    )
}

pub(super) fn build_simple_diff(old: &str, new: &str) -> String {
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
        out = "(no changes)".to_string();
    }
    out
}

#[cfg(test)]
#[path = "tests/diff_test.rs"]
mod tests;
