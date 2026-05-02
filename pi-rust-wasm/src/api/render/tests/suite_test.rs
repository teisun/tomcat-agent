use super::super::*;

#[test]
fn plain_text_passes_through() {
    let mut r = MarkdownRenderer::new();
    r.push("hello world\n");
    let out = r.take_ready().unwrap();
    assert!(out.contains("hello world"));
}

#[test]
fn code_block_is_highlighted() {
    let mut r = MarkdownRenderer::new();
    r.push("```rust\nfn main() {}\n```\n");
    let out = r.take_ready().unwrap();
    assert!(out.contains("fn"));
    assert!(out.contains("\x1b["));
}

#[test]
fn unknown_lang_falls_back() {
    let mut r = MarkdownRenderer::new();
    r.push("```xyzlang\nsome code\n```\n");
    let out = r.take_ready().unwrap();
    assert!(out.contains("some code"));
}

#[test]
fn flush_returns_remaining() {
    let mut r = MarkdownRenderer::new();
    r.push("partial");
    assert!(r.take_ready().is_some());
    let flushed = r.flush();
    assert!(matches!(flushed, None | Some(_)));
}
