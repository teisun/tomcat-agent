use super::*;

fn hit(path: &str, line: u64, col: u64, text: &str) -> String {
    format!("{path}\u{1f}{line}\u{1f}{col}\u{1f}{text}")
}

fn ctx(path: &str, line: u64, text: &str) -> String {
    format!("{path}\u{1e}{line}\u{1e}{text}")
}

#[test]
fn content_output_attaches_context_around_each_hit() {
    let stdout = [
        ctx("a.txt", 1, "before one"),
        ctx("a.txt", 2, "before two"),
        hit("a.txt", 3, 1, "needle"),
        ctx("a.txt", 4, "after one"),
        "--".to_string(),
        ctx("b.txt", 9, "b before"),
        hit("b.txt", 10, 1, "needle again"),
    ]
    .join("\n");

    let matches = parse_rg_content_output(&stdout);

    assert_eq!(matches.len(), 2);
    assert_eq!(matches[0].path, "a.txt");
    assert_eq!(matches[0].line, 3);
    assert_eq!(matches[0].before, vec!["before one", "before two"]);
    assert_eq!(matches[0].after, vec!["after one"]);
    // `--` 是 rg 的组边界：跨组的上下文不能粘到上一条命中行上。
    assert_eq!(matches[1].path, "b.txt");
    assert_eq!(matches[1].before, vec!["b before"]);
    assert!(matches[1].after.is_empty());
}

#[test]
fn content_output_survives_paths_containing_colons_and_dashes() {
    let stdout = [
        ctx("weird:name-1.txt", 4, "ctx"),
        hit("weird:name-1.txt", 5, 2, "needle"),
    ]
    .join("\n");

    let matches = parse_rg_content_output(&stdout);

    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].path, "weird:name-1.txt");
    assert_eq!(matches[0].line, 5);
    assert_eq!(matches[0].before, vec!["ctx"]);
}

#[test]
fn context_rows_with_colons_or_match_separators_are_not_hits() {
    for text in ["clock 12:34:56: context", "fake\u{1f}7\u{1f}2\u{1f}context"] {
        let stdout = [
            ctx("a.rs", 1, text),
            hit("a.rs", 2, 1, "needle"),
            ctx("a.rs", 3, text),
        ]
        .join("\n");
        let matches = parse_rg_content_output(&stdout);
        assert_eq!(
            matches.len(),
            1,
            "context became a fake path/hit: {matches:?}"
        );
        assert_eq!(matches[0].path, "a.rs");
        assert_eq!(matches[0].before, vec![text]);
        assert_eq!(matches[0].after, vec![text]);
    }
    // Preserve the intentional legacy/shim colon format, and native match content.
    let native = hit("a.rs", 2, 1, "text\u{1e}7\u{1e}context-like content");
    let matches = parse_rg_content_output(&format!("legacy.rs:1:2:needle\n{native}"));
    assert_eq!(matches.len(), 2);
    assert_eq!(matches[0].path, "legacy.rs");
    assert_eq!(matches[1].path, "a.rs");
}

#[test]
fn context_rows_cannot_evade_a_file_deny_via_a_fake_path() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let denied = root.join("private.txt");
    std::fs::write(&denied, "needle\nprivate context").unwrap();
    let rules = [PathRule::new(denied.to_string_lossy(), PathRuleMode::Deny)];
    for text in [
        "clock 12:34:56: private context",
        "fake\u{1f}7\u{1f}2\u{1f}private context",
    ] {
        let stdout = [
            hit("private.txt", 1, 1, "needle"),
            ctx("private.txt", 2, text),
        ]
        .join("\n");
        let (visible, skipped) =
            filter_denied_matches(root, parse_rg_content_output(&stdout), &rules);
        assert!(
            visible.is_empty(),
            "a denied file's context escaped as a fake hit: {visible:?}"
        );
        assert_eq!(skipped, 1);
    }
}
