//! E2E-CLI-023 / E2E-CLI-026 的离线契约层。
//!
//! 真正的 TTY 菜单观感仍可人工补验；这里锁定可稳定自动化的拖拽语义和 deny 菜单裁剪。

use pi_wasm::api::chat::dragged_path::{interpret_dragged_paths, render_drag_menu, DragOutcome};
use pi_wasm::core::permission::{
    DefaultPermissionGate, DraggedPaths, GateConfig, PathRule, PathRuleMode, SessionGrants,
};

#[test]
fn path_with_intent_silent_passthrough_contract() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let line = format!("'{}'看下里面有什么文件", project.display());

    assert_eq!(interpret_dragged_paths(&line), DragOutcome::None);
}

#[test]
fn deny_path_drag_menu_only_allows_cancel_contract() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("workspace");
    let denied = tmp.path().join("deny-target");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(&denied).unwrap();

    let gate = DefaultPermissionGate::new(
        GateConfig {
            agent_workspace_dir: workspace,
            extra_roots: vec![],
            agent_data_readonly_dirs: vec![],
            user_path_rules: vec![PathRule::new(
                denied.to_string_lossy().to_string(),
                PathRuleMode::Deny,
            )],
            user_bash_forbidden: vec![],
            user_bash_approval: vec![],
            auto_confirm: false,
        },
        SessionGrants::new(),
        DraggedPaths::new(),
    );

    let menu = render_drag_menu(&denied, &gate);
    assert!(menu.cancel);
    assert!(!menu.allow_once);
    assert!(!menu.persist_extra_root);
    assert!(!menu.persist_readonly);
    assert!(!menu.persist_deny);
    assert!(menu.note.as_deref().unwrap_or("").contains("禁止读写访问"));
}
