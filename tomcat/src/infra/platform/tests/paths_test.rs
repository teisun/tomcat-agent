use std::path::MAIN_SEPARATOR_STR;

use super::{path_starts_with, to_slash};

fn native(path: &str) -> String {
    path.replace('/', MAIN_SEPARATOR_STR)
}

#[test]
fn to_slash_only_rewrites_backslash_where_it_is_a_separator() {
    let expected = if cfg!(windows) { "a/b/c" } else { r"a\b/c" };
    assert_eq!(to_slash(r"a\b/c"), expected);
}

#[test]
fn path_starts_with_requires_a_component_boundary() {
    assert!(path_starts_with(&native("/a/b/c"), &native("/a/b")));
    assert!(path_starts_with(&native("/a/b"), &native("/a/b")));
    assert!(!path_starts_with(&native("/a/bc"), &native("/a/b")));
}

#[test]
fn path_starts_with_ignores_trailing_separators() {
    assert!(path_starts_with(&native("/a/b/"), &native("/a/b")));
    assert!(path_starts_with(&native("/a/b"), &native("/a/b/")));
    assert!(path_starts_with(&native("/a/b/c"), &native("/a/b/")));
}

#[test]
fn path_starts_with_case_follows_platform_rule() {
    assert_eq!(
        path_starts_with(&native("/Users/A/x"), &native("/users/a")),
        super::PATH_COMPARE_IGNORES_CASE
    );
}

#[cfg(windows)]
#[test]
fn path_starts_with_ignores_drive_letter_case_on_windows() {
    assert!(path_starts_with(r"C:\Users\A\.ssh\id", r"c:\users\a"));
}
