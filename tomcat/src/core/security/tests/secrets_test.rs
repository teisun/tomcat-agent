//! `secrets::scan` / `format_preview` 行为单测。

use crate::core::security::secrets::{format_preview, scan, SecretHit};

#[test]
fn scan_detects_openai_key() {
    let hits = scan("let key = \"sk-ABCDEFGHIJKLMNOPQRSTUV\"");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].rule, "openai_api_key");
}

#[test]
fn scan_detects_aws_key() {
    let hits = scan("AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE\n");
    assert!(hits.iter().any(|h| h.rule == "aws_access_key_id"));
}

#[test]
fn scan_returns_empty_for_plain_code() {
    let hits = scan("fn main() { println!(\"hello\"); }");
    assert!(hits.is_empty(), "普通代码不应触发：{:?}", hits);
}

#[test]
fn scan_orders_hits_by_offset() {
    let body = format!(
        "{}{}",
        "padding ", "AKIAIOSFODNN7EXAMPLE first sk-ABCDEFGHIJKLMNOPQRSTUV second"
    );
    let hits = scan(&body);
    assert!(hits.len() >= 2);
    for w in hits.windows(2) {
        assert!(w[0].byte_offset <= w[1].byte_offset);
    }
}

#[test]
fn format_preview_masks_middle() {
    let hits = vec![SecretHit {
        rule: "openai_api_key",
        byte_offset: 10,
        matched: "sk-ABCDEFGHIJKLMNOPQRSTUV".to_string(),
    }];
    let p = format_preview(&hits);
    assert!(p.contains("openai_api_key"));
    // 应当含掩码省略号，且不含中间字符
    assert!(p.contains("…"));
    assert!(!p.contains("sk-ABCDEFGHIJKLMNOPQRSTUV"));
}
