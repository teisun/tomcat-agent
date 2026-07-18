use std::borrow::Cow;

use super::{DiffTag, FileDiffLine};

const MAX_DIFF_TOTAL_LINES: usize = 8_000;
const MAX_DIFF_TOTAL_BYTES: usize = 1_500_000;

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
        text: text.to_string(),
    });
}

fn push_add(out: &mut Vec<FileDiffLine>, new_line: usize, text: &str) {
    out.push(FileDiffLine {
        tag: DiffTag::Add,
        old_line: None,
        new_line: Some(to_u32(new_line)),
        text: text.to_string(),
    });
}

fn push_del(out: &mut Vec<FileDiffLine>, old_line: usize, text: &str) {
    out.push(FileDiffLine {
        tag: DiffTag::Del,
        old_line: Some(to_u32(old_line)),
        new_line: None,
        text: text.to_string(),
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

pub(crate) fn build_line_diff(old: &str, new: &str) -> Option<Vec<FileDiffLine>> {
    if old.len().saturating_add(new.len()) > MAX_DIFF_TOTAL_BYTES {
        return None;
    }
    if normalize_text(old) == normalize_text(new) {
        return Some(Vec::new());
    }

    let old_lines = split_logical_lines(old);
    let new_lines = split_logical_lines(new);
    if old_lines.len().max(new_lines.len()) > MAX_DIFF_TOTAL_LINES {
        return None;
    }

    let mut diff = Vec::with_capacity(old_lines.len().max(new_lines.len()));
    build_line_diff_recursive(&old_lines, &new_lines, 0, 0, &mut diff);
    Some(diff)
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
        out = "(无变化)".to_string();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{build_line_diff, line_diff_stat, normalize_text};
    use crate::core::tools::primitive::{DiffTag, FileDiffLine};

    fn logical_text(text: &str) -> String {
        let normalized = normalize_text(text);
        let without_terminal_newline = normalized.strip_suffix('\n').unwrap_or(&normalized);
        without_terminal_newline.to_string()
    }

    fn rebuild_old(diff: &[FileDiffLine]) -> String {
        diff.iter()
            .filter(|line| matches!(line.tag, DiffTag::Ctx | DiffTag::Del))
            .map(|line| line.text.clone())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn rebuild_new(diff: &[FileDiffLine]) -> String {
        diff.iter()
            .filter(|line| matches!(line.tag, DiffTag::Ctx | DiffTag::Add))
            .map(|line| line.text.clone())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn line_diff_stat_handles_empty_input() {
        assert_eq!(line_diff_stat("", ""), (0, 0));
    }

    #[test]
    fn line_diff_stat_handles_new_file() {
        assert_eq!(line_diff_stat("", "alpha\nbeta\ngamma\n"), (3, 0));
    }

    #[test]
    fn line_diff_stat_handles_pure_deletion() {
        assert_eq!(line_diff_stat("alpha\nbeta\ngamma\n", ""), (0, 3));
    }

    #[test]
    fn line_diff_stat_handles_mixed_changes() {
        assert_eq!(
            line_diff_stat("alpha\nbeta\ngamma\n", "alpha\nbeta-2\ngamma\ndelta\n"),
            (2, 1)
        );
    }

    #[test]
    fn line_diff_stat_handles_repeated_lines() {
        assert_eq!(
            line_diff_stat("same\nkeep\nsame\n", "same\nsame\nkeep\nsame\n"),
            (1, 0)
        );
    }

    #[test]
    fn line_diff_stat_ignores_crlf_only_changes() {
        assert_eq!(line_diff_stat("alpha\r\nbeta\r\n", "alpha\nbeta\n"), (0, 0));
    }

    #[test]
    fn line_diff_stat_ignores_terminal_newline_only_changes() {
        assert_eq!(line_diff_stat("alpha\nbeta", "alpha\nbeta\n"), (0, 0));
    }

    #[test]
    fn build_line_diff_handles_new_file() {
        let diff = build_line_diff("", "alpha\nbeta\ngamma\n")
            .expect("small new file should produce structured diff");
        assert_eq!(diff.len(), 3);
        assert!(diff.iter().all(|line| line.tag == DiffTag::Add));
        assert_eq!(diff[0].old_line, None);
        assert_eq!(diff[0].new_line, Some(1));
        assert_eq!(rebuild_old(&diff), "");
        assert_eq!(rebuild_new(&diff), "alpha\nbeta\ngamma");
    }

    #[test]
    fn build_line_diff_handles_pure_deletion() {
        let diff = build_line_diff("alpha\nbeta\ngamma\n", "")
            .expect("small deletion should produce structured diff");
        assert_eq!(diff.len(), 3);
        assert!(diff.iter().all(|line| line.tag == DiffTag::Del));
        assert_eq!(diff[1].old_line, Some(2));
        assert_eq!(diff[1].new_line, None);
        assert_eq!(rebuild_old(&diff), "alpha\nbeta\ngamma");
        assert_eq!(rebuild_new(&diff), "");
    }

    #[test]
    fn build_line_diff_handles_mixed_changes_with_context() {
        let diff = build_line_diff("alpha\nbeta\ngamma\n", "alpha\nbeta-2\ngamma\ndelta\n")
            .expect("mixed change should produce structured diff");
        let tags: Vec<_> = diff.iter().map(|line| line.tag).collect();
        assert_eq!(
            tags,
            vec![
                DiffTag::Ctx,
                DiffTag::Del,
                DiffTag::Add,
                DiffTag::Ctx,
                DiffTag::Add
            ]
        );
        assert_eq!(diff[0].old_line, Some(1));
        assert_eq!(diff[0].new_line, Some(1));
        assert_eq!(diff[1].old_line, Some(2));
        assert_eq!(diff[1].new_line, None);
        assert_eq!(diff[2].old_line, None);
        assert_eq!(diff[2].new_line, Some(2));
        assert_eq!(rebuild_old(&diff), "alpha\nbeta\ngamma");
        assert_eq!(rebuild_new(&diff), "alpha\nbeta-2\ngamma\ndelta");
    }

    #[test]
    fn build_line_diff_handles_repeated_lines() {
        let old = "same\nkeep\nsame\n";
        let new = "same\nsame\nkeep\nsame\n";
        let diff = build_line_diff(old, new).expect("repeated lines should diff");
        assert_eq!(rebuild_old(&diff), logical_text(old));
        assert_eq!(rebuild_new(&diff), logical_text(new));
        assert_eq!(
            diff.iter().filter(|line| line.tag == DiffTag::Add).count(),
            1
        );
    }

    #[test]
    fn build_line_diff_normalizes_crlf_and_terminal_newline() {
        let diff = build_line_diff("alpha\r\nbeta\r\n", "alpha\nbeta\n")
            .expect("normalized equal content should still succeed");
        assert!(diff.is_empty());
    }

    #[test]
    fn build_line_diff_returns_none_when_file_is_too_large() {
        let huge = (0..=super::MAX_DIFF_TOTAL_LINES)
            .map(|index| format!("line-{index}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(build_line_diff("", &huge).is_none());
    }

    #[test]
    fn build_line_diff_reconstructs_original_and_new_content() {
        let old = "one\ntwo\nthree\nfour\n";
        let new = "zero\none\nthree\nfour\nfive\n";
        let diff = build_line_diff(old, new).expect("diff should exist");
        assert_eq!(rebuild_old(&diff), logical_text(old));
        assert_eq!(rebuild_new(&diff), logical_text(new));
    }
}
