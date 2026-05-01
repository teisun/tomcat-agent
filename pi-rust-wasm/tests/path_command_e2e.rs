//! Offline contracts for `/path` chat command behavior.
//!
//! The real TTY menu is still best verified manually. These tests pin the
//! stable parsing and permission menu filtering behavior.

use pi_wasm::api::chat::commands::{parse_chat_command, render_path_menu, ChatCommand};
use pi_wasm::core::permission::{
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
    let workspace = tmp.path().join("workspace");
    let denied = tmp.path().join("deny-target");
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
    assert!(!menu.allow_once);
    assert!(!menu.persist_extra_root);
    assert!(!menu.persist_readonly);
    assert!(!menu.persist_deny);
    assert!(menu.note.as_deref().unwrap_or("").contains("禁止读写访问"));
}
