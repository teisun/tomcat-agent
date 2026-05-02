//! Tests for `commands::cmd_path` — `/path` command parsing + menu pre-check.

use std::path::PathBuf;

use super::super::cmd_path::{is_path_token, render_path_menu, PathMenuChoice, PathMenuOptions};
use super::super::{parse_chat_command, ChatCommand};

use crate::core::permission::{
    DefaultPermissionGate, GateConfig, PathRule, PathRuleMode, SessionGrants,
};

fn assert_not_command(input: &str) {
    assert!(matches!(
        parse_chat_command(input),
        ChatCommand::NotACommand(_)
    ));
}

#[test]
fn path_command_accepts_single_path() {
    assert_eq!(
        parse_chat_command("/path /a"),
        ChatCommand::Path {
            path: PathBuf::from("/a"),
            original_line: "/path /a".to_string(),
        }
    );
}

#[test]
fn path_command_accepts_quoted_path_with_space() {
    assert_eq!(
        parse_chat_command("/path '/a b'"),
        ChatCommand::Path {
            path: PathBuf::from("/a b"),
            original_line: "/path '/a b'".to_string(),
        }
    );
}

#[test]
fn path_command_requires_argument() {
    assert!(matches!(
        parse_chat_command("/path"),
        ChatCommand::UsageError { .. }
    ));
}

#[test]
fn path_command_rejects_multiple_arguments() {
    assert!(matches!(
        parse_chat_command("/path /a /b"),
        ChatCommand::UsageError { .. }
    ));
}

#[test]
fn path_command_is_lowercase_only() {
    assert_not_command("/PATH /a");
    assert_not_command("/Path /a");
}

#[test]
fn leading_spaces_do_not_affect_commands() {
    assert_eq!(
        parse_chat_command("  /path /a"),
        ChatCommand::Path {
            path: PathBuf::from("/a"),
            original_line: "/path /a".to_string(),
        }
    );
}

#[test]
fn solo_slash_or_tilde_not_path_token() {
    assert!(!is_path_token("/"));
    assert!(!is_path_token("~"));
}

#[test]
fn quoted_path_with_space_recognized() {
    assert!(is_path_token("/Users/yan/My Documents/foo"));
}

#[test]
fn path_menu_choice_parses_inputs() {
    assert_eq!(
        PathMenuChoice::from_input("a"),
        Some(PathMenuChoice::AllowOnce)
    );
    assert_eq!(
        PathMenuChoice::from_input("W"),
        Some(PathMenuChoice::PersistWorkspaceRoot)
    );
    assert_eq!(
        PathMenuChoice::from_input("r"),
        Some(PathMenuChoice::PersistReadonly)
    );
    assert_eq!(
        PathMenuChoice::from_input("d"),
        Some(PathMenuChoice::PersistDeny)
    );
    assert_eq!(
        PathMenuChoice::from_input("c"),
        Some(PathMenuChoice::Cancel)
    );
    assert_eq!(PathMenuChoice::from_input(""), None);
    assert_eq!(PathMenuChoice::from_input("xyz"), None);
}

#[test]
fn path_menu_options_full_has_all_options() {
    let m = PathMenuOptions::full();
    assert!(m.allow_once && m.persist_extra_root && m.persist_readonly && m.persist_deny);
    assert!(m.note.is_none());
}

#[test]
fn path_menu_options_deny_only_only_cancel() {
    let m = PathMenuOptions::deny_only("note");
    assert!(!m.allow_once && !m.persist_extra_root && !m.persist_readonly);
    assert!(!m.persist_deny && m.cancel);
    assert!(m.note.is_some());
}

#[test]
fn path_menu_with_deny_rule_hides_authorization_choices() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("workspace");
    let denied = tmp.path().join("secret");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(&denied).unwrap();
    let gate = DefaultPermissionGate::new(
        GateConfig {
            agent_definition_dir: workspace,
            workspace_roots: vec![],
            agent_trail_readonly_dirs: vec![],
            user_path_rules: vec![PathRule::new(
                denied.to_string_lossy().to_string(),
                PathRuleMode::Deny,
            )],
            user_bash_forbidden: vec![],
            user_bash_approval: vec![],
            auto_confirm: false,
        },
        SessionGrants::new(),
    );

    let menu = render_path_menu(&denied, &gate);

    assert!(menu.cancel);
    assert!(!menu.allow_once, "deny 命中后不得允许本次授权");
    assert!(!menu.persist_extra_root, "deny 命中后不得允许持久扩权");
    assert!(
        !menu.persist_readonly,
        "deny 命中后不得降级为 readonly 扩权"
    );
    assert!(!menu.persist_deny, "deny 命中后无需再展示重复 deny 选项");
    assert!(menu.note.as_deref().unwrap_or("").contains("禁止读写访问"));
}

#[test]
fn path_menu_options_readonly_allows_session_read_but_not_extra_root() {
    let m = PathMenuOptions::readonly_only("note");
    assert!(m.allow_once);
    assert!(!m.persist_extra_root);
    assert!(m.persist_readonly && m.persist_deny && m.cancel);
}

#[test]
fn nonexistent_ascii_path_is_valid_token() {
    // 全 ASCII 不存在路径仍可作为用户想授权的纯路径。
    assert!(is_path_token("/etc/foo/nonexistent"));
}

#[test]
fn nonascii_token_without_existence_returns_none() {
    assert!(!is_path_token("/abs/path中文"));
}
