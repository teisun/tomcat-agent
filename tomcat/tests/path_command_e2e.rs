//! Offline contracts for `/path` chat command behavior.
//!
//! The real TTY menu is still best verified manually. These tests pin the
//! stable parsing and permission menu filtering behavior, and lock in the
//! user-visible `/help` banner so future refactors cannot silently regress
//! `E2E-CLI-026 path_help_command_contract` /
//! `E2E-CLI-019 manual_path_command_denied_shows_cancel_only`.

use tomcat::api::chat::commands::{help_text, parse_chat_command, render_path_menu, ChatCommand};
use tomcat::core::permission::{
    DefaultPermissionGate, GateConfig, PathRule, PathRuleMode, SessionGrants,
};

#[test]
fn path_with_intent_silent_passthrough_contract() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let line = format!("{} 看下里面有什么文件", project.display());

    assert!(matches!(
        parse_chat_command(&line),
        ChatCommand::NotACommand(_)
    ));
}

#[test]
fn deny_path_command_menu_only_allows_cancel_contract() {
    let tmp = tempfile::tempdir().unwrap();
    let agent_def_dir = tmp.path().join("workspace-temp");
    let denied = tmp.path().join("deny-target");
    std::fs::create_dir_all(&agent_def_dir).unwrap();
    std::fs::create_dir_all(&denied).unwrap();

    let gate = DefaultPermissionGate::new(
        GateConfig {
            agent_definition_dir: agent_def_dir,
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
    assert!(!menu.allow_once);
    assert!(!menu.persist_extra_root);
    assert!(!menu.persist_readonly);
    assert!(!menu.persist_deny);
    assert!(menu.note.as_deref().unwrap_or("").contains("禁止读写访问"));
}

/// E2E-CLI-026 — `/help` 解析 + 用户可见文案契约。
#[test]
fn help_command_lists_path_and_help_contract() {
    assert_eq!(parse_chat_command("/help"), ChatCommand::Help);

    let banner = help_text();
    assert!(
        banner.contains("/path"),
        "/help 文案必须列出 /path 本地命令；当前为：\n{banner}"
    );
    assert!(
        banner.contains("/help"),
        "/help 文案必须列出 /help 自身；当前为：\n{banner}"
    );
    assert!(
        banner.contains("绝对路径"),
        "/help 必须说明 /path 需要绝对路径参数；当前为：\n{banner}"
    );
}

/// E2E 契约：`/path` 用法错误（缺参 / 多参 / 大写）应保持稳定的本地处理路径，
/// 不得回落为「按聊天发送给 LLM」。
#[test]
fn path_command_usage_errors_e2e_contract() {
    assert!(
        matches!(parse_chat_command("/path"), ChatCommand::UsageError { .. }),
        "/path 缺参必须返回 UsageError，避免静默把 `/path` 转给 LLM"
    );
    assert!(
        matches!(
            parse_chat_command("/path /a /b"),
            ChatCommand::UsageError { .. }
        ),
        "/path 多参必须返回 UsageError"
    );
    assert!(
        matches!(parse_chat_command("/PATH /a"), ChatCommand::NotACommand(_)),
        "/PATH 大写必须按普通聊天处理（命令为小写 only）"
    );
}
