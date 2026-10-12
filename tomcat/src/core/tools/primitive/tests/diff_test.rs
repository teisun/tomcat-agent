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
    let preview = build_line_diff("", "alpha\nbeta\ngamma\n");
    assert!(!preview.truncated);
    let diff = preview
        .lines
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
    let preview = build_line_diff("alpha\nbeta\ngamma\n", "");
    assert!(!preview.truncated);
    let diff = preview
        .lines
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
    let preview = build_line_diff("alpha\nbeta\ngamma\n", "alpha\nbeta-2\ngamma\ndelta\n");
    assert!(!preview.truncated);
    let diff = preview
        .lines
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
    let diff = build_line_diff(old, new)
        .lines
        .expect("repeated lines should diff");
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
        .lines
        .expect("normalized equal content should still succeed");
    assert!(diff.is_empty());
}

#[test]
fn build_line_diff_marks_large_input_as_truncated() {
    let huge = (0..=super::MAX_DIFF_INPUT_LINES)
        .map(|index| format!("line-{index}"))
        .collect::<Vec<_>>()
        .join("\n");
    let preview = build_line_diff("", &huge);
    assert!(preview.truncated);
    assert!(preview.lines.is_none());
}

#[test]
fn build_line_diff_truncates_rendered_output_at_line_budget_and_keeps_prefix() {
    let new = (1..=2_500)
        .map(|line| format!("added-{line}"))
        .collect::<Vec<_>>()
        .join("\n");

    let preview = build_line_diff("", &new);
    let lines = preview
        .lines
        .expect("render budget must retain a useful prefix");
    assert!(preview.truncated);
    assert_eq!(lines.len(), super::MAX_DIFF_RENDERED_LINES);
    assert_eq!(
        lines.first().map(|line| line.text.as_str()),
        Some("added-1")
    );
    assert_eq!(
        lines.last().and_then(|line| line.new_line),
        Some(super::MAX_DIFF_RENDERED_LINES as u32)
    );
}

#[test]
fn build_line_diff_truncates_rendered_output_at_byte_budget_and_keeps_prefix() {
    let oversized_line = "x".repeat(1_024);
    let new = (0..300)
        .map(|_| oversized_line.clone())
        .collect::<Vec<_>>()
        .join("\n");

    let preview = build_line_diff("", &new);
    let lines = preview
        .lines
        .expect("byte budget must retain a useful prefix");
    assert!(preview.truncated);
    assert!(lines.len() < super::MAX_DIFF_RENDERED_LINES);
    assert_eq!(lines.first().and_then(|line| line.new_line), Some(1));
    assert!(lines.iter().all(|line| line.text.len() == 1_024));
}

#[test]
fn build_line_diff_keeps_three_context_lines_and_marks_gaps() {
    let old = (1..=4_800)
        .map(|line| format!("line-{line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut new_lines = old.lines().map(ToOwned::to_owned).collect::<Vec<_>>();
    new_lines.splice(2_000..2_000, (1..=109).map(|line| format!("added-{line}")));
    let preview = build_line_diff(&old, &new_lines.join("\n"));
    let diff = preview.lines.expect("medium input should produce a diff");
    assert!(!preview.truncated);
    assert!(diff.len() < 200);
    assert!(diff.iter().any(|line| line.tag == DiffTag::Gap));
    assert!(diff
        .iter()
        .any(|line| line.tag == DiffTag::Gap && line.text.ends_with("unmodified lines")));
}
