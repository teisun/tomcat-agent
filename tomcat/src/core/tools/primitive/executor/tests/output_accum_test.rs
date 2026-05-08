//! `output_accum::accumulate_with_persist` 单测：头尾保留 / 落盘 / 多字节安全。

use crate::core::tools::primitive::executor::output_accum::accumulate_with_persist;
use std::fs;

#[test]
fn empty_input_returns_unchanged() {
    let out = accumulate_with_persist("", 100, None, "bash-stdout");
    assert_eq!(out.text, "");
    assert!(!out.truncated);
    assert!(out.persisted_path.is_none());
}

#[test]
fn short_input_returns_unchanged() {
    let s = "hello world";
    let out = accumulate_with_persist(s, 100, None, "bash-stdout");
    assert_eq!(out.text, s);
    assert!(!out.truncated);
}

#[test]
fn boundary_at_exactly_max_chars_no_truncate() {
    let s: String = "a".repeat(50);
    let out = accumulate_with_persist(&s, 50, None, "bash-stdout");
    assert!(!out.truncated, "char_count == max_chars 不应触发截断");
    assert_eq!(out.text, s);
}

#[test]
fn skips_truncation_when_max_chars_too_small() {
    let s = "0123456789abcdef";
    let out = accumulate_with_persist(s, 8, None, "bash-stdout");
    assert!(!out.truncated, "max_chars < 16 直接返回原文（兜底）");
    assert_eq!(out.text, s);
}

#[test]
fn long_input_keeps_head_and_tail() {
    let s: String = (0..200).map(|i| (b'a' + (i % 26) as u8) as char).collect();
    let out = accumulate_with_persist(&s, 32, None, "bash-stdout");
    assert!(out.truncated);
    assert!(out.text.contains("[truncated"));
    assert!(out.text.starts_with(&s[..16]));
    assert!(out.text.ends_with(&s[s.len() - 16..]));
    assert!(out.persisted_path.is_none(), "无 persist_dir 时应不落盘");
}

#[test]
fn multibyte_input_does_not_panic() {
    let s = "中文a".repeat(100);
    let out = accumulate_with_persist(&s, 32, None, "bash-stdout");
    assert!(out.truncated);
}

#[test]
fn persists_full_output_when_truncated() {
    let dir = tempfile::tempdir().expect("tempdir");
    let s: String = (0..200).map(|i| (b'a' + (i % 26) as u8) as char).collect();
    let out = accumulate_with_persist(&s, 32, Some(dir.path()), "bash-stdout");
    assert!(out.truncated);
    let path = out.persisted_path.expect("应落盘");
    let on_disk = fs::read_to_string(&path).expect("读盘");
    assert_eq!(on_disk, s, "落盘文件应是完整原文（不是截断后文本）");
    assert!(path.starts_with(dir.path()));
    let fname = path.file_name().and_then(|n| n.to_str()).unwrap();
    assert!(fname.starts_with("bash-stdout-"));
    assert!(fname.ends_with(".txt"));
}

#[test]
fn persist_dir_auto_create() {
    let base = tempfile::tempdir().expect("tempdir");
    let nested = base.path().join("nonexistent").join("tool-results");
    let s: String = (0..200).map(|i| (b'a' + (i % 26) as u8) as char).collect();
    let out = accumulate_with_persist(&s, 32, Some(&nested), "bash-stderr");
    assert!(out.truncated);
    assert!(out.persisted_path.is_some(), "落盘前应自动 mkdir -p");
    assert!(nested.exists());
}
