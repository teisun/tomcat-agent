//! `PathRule::matches` 行为：含/不含 glob、路径前缀、`~` 展开。

use crate::core::permission::path_rule::PathRule;
use crate::core::permission::types::PathRuleMode;
use std::path::PathBuf;

#[test]
fn prefix_rule_matches_subpath() {
    let r = PathRule::new("/etc", PathRuleMode::Deny);
    assert!(r.matches(&PathBuf::from("/etc/passwd")));
    assert!(r.matches(&PathBuf::from("/etc")));
    assert!(!r.matches(&PathBuf::from("/etcd/data")));
}

#[test]
fn glob_rule_matches_globset() {
    let r = PathRule::new("/tmp/agents/*/sessions/**", PathRuleMode::Readonly);
    assert!(r.has_glob());
    assert!(r.matches(&PathBuf::from("/tmp/agents/main/sessions/abc.jsonl")));
    assert!(!r.matches(&PathBuf::from("/tmp/agents/main/audit/abc.jsonl")));
}

#[test]
fn tilde_expansion_no_glob() {
    let _home_lock = crate::test_support::home_env_lock().lock().unwrap();
    let r = PathRule::new("~/some/dir", PathRuleMode::Deny);
    let expanded = r.expanded_path().expect("expand");
    let home = dirs::home_dir().expect("home");
    assert!(expanded.starts_with(&home.to_string_lossy().to_string()));
}
