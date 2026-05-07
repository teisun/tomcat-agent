//! `edit_normalize` 字节级 normalize 流水的单元测试。

use crate::core::tools::pipeline::edit_normalize::{
    desanitize, detect_line_ending, fold_curly_quotes, is_unsupported_structured_file,
    normalize_for_match, normalize_to_lf, restore_line_endings, strip_bom, LineEndingKind,
};
use std::borrow::Cow;

#[test]
fn strip_bom_detects_and_removes() {
    let (s, had) = strip_bom("\u{FEFF}hello");
    assert!(had);
    assert_eq!(s, "hello");

    let (s, had) = strip_bom("hello");
    assert!(!had);
    assert_eq!(s, "hello");
}

#[test]
fn detect_line_ending_recognizes_pure_styles() {
    assert_eq!(detect_line_ending("a\nb\n"), LineEndingKind::Lf);
    assert_eq!(detect_line_ending("a\r\nb\r\n"), LineEndingKind::CrLf);
    assert_eq!(detect_line_ending("a\rb\r"), LineEndingKind::Cr);
    assert_eq!(detect_line_ending("noeol"), LineEndingKind::Lf);
}

#[test]
fn detect_line_ending_recognizes_mixed() {
    assert_eq!(detect_line_ending("a\r\nb\nc"), LineEndingKind::Mixed);
}

#[test]
fn normalize_to_lf_borrows_when_no_cr() {
    let cow = normalize_to_lf("plain\nlf\n");
    assert!(matches!(cow, Cow::Borrowed(_)));
}

#[test]
fn normalize_to_lf_collapses_crlf_and_cr() {
    let cow = normalize_to_lf("a\r\nb\rc\nd");
    assert_eq!(cow.as_ref(), "a\nb\nc\nd");
}

#[test]
fn restore_line_endings_roundtrip() {
    let original = "a\r\nb\r\nc";
    let kind = detect_line_ending(original);
    let working = normalize_to_lf(original);
    // 假设我们在 working 上不做编辑
    let restored = restore_line_endings(kind, &working);
    assert_eq!(restored.as_ref(), original);
}

#[test]
fn restore_line_endings_mixed_does_nothing() {
    let cow = restore_line_endings(LineEndingKind::Mixed, "a\nb");
    assert!(matches!(cow, Cow::Borrowed(_)));
}

#[test]
fn fold_curly_quotes_handles_double_and_single() {
    let s = fold_curly_quotes("“double” and ‘single’");
    assert_eq!(s.as_ref(), "\"double\" and 'single'");
}

#[test]
fn fold_curly_quotes_borrows_when_no_change_needed() {
    let s = fold_curly_quotes("\"plain\"");
    assert!(matches!(s, Cow::Borrowed(_)));
}

#[test]
fn desanitize_replaces_invisible_and_nbsp() {
    let s = desanitize("a\u{00A0}b\u{200B}c\u{2060}d\u{3000}e");
    assert_eq!(s.as_ref(), "a bcd e");
}

#[test]
fn desanitize_borrows_when_no_change_needed() {
    let s = desanitize("plain text");
    assert!(matches!(s, Cow::Borrowed(_)));
}

#[test]
fn normalize_for_match_runs_full_pipeline() {
    let s = normalize_for_match("\u{FEFF}“hi”\r\n\u{00A0}world");
    assert_eq!(s, "\"hi\"\n world");
}

#[test]
fn is_unsupported_structured_file_recognizes_ipynb() {
    assert!(is_unsupported_structured_file("notebook.ipynb"));
    assert!(is_unsupported_structured_file("/abs/path/x.IPYNB"));
    assert!(!is_unsupported_structured_file("file.py"));
    assert!(!is_unsupported_structured_file("ipynb.txt"));
}
