use std::sync::Arc;

use super::{
    config_get_impl, config_set_impl, is_array_field, is_readable, is_writable, ConfigToolContext,
};
use crate::core::permission::{
    DefaultPermissionGate, GateConfig, PathRule, PathRuleMode, PermissionDecision, SessionGrants,
};
use crate::core::tools::contract::confirmation::{
    AllowAllConfirmation, DenyAllConfirmation, UserConfirmationProvider,
};
use crate::core::tools::primitive::PrimitiveOperation;
use crate::infra::config::load_config;
use crate::infra::error::AppError;
use tempfile::TempDir;

#[test]
fn read_allowlist_covers_documented_keys() {
    for k in [
        "workspace",
        "workspace.workspace_roots",
        "primitive.path_rules",
        "agent.id",
        "log.level",
    ] {
        assert!(is_readable(k), "{k} should be readable");
    }
}

#[test]
fn read_hardcoded_deny_overrides_allowlist() {
    for k in [
        "llm.api_key",
        "llm.api_key_env",
        "security.audit_log_retention_days",
        "storage.work_dir",
    ] {
        assert!(!is_readable(k), "{k} must be denied");
    }
}

#[test]
fn write_allowlist_subset() {
    for k in [
        "workspace.workspace_roots",
        "primitive.path_rules",
        "primitive.bash_forbidden",
        "log.level",
    ] {
        assert!(is_writable(k), "{k} should be writable");
    }
}

#[test]
fn write_hardcoded_deny_blocks_self_escalation() {
    for k in [
        "primitive.bash_whitelist",
        "primitive.auto_confirm",
        "primitive.path_whitelist",
        "primitive.auto_confirm_whitelist",
        "agent.id",
        "agent.workspace",
        "llm.api_key",
        "security.enable_audit_log",
    ] {
        assert!(!is_writable(k), "{k} must be denied");
    }
}

#[test]
fn array_fields_classification() {
    assert!(is_array_field("workspace.workspace_roots"));
    assert!(is_array_field("primitive.path_rules"));
    assert!(is_array_field("primitive.bash_forbidden"));
    assert!(!is_array_field("log.level"));
    assert!(!is_array_field("llm.default_model"));
}

fn empty_config(dir: &TempDir) -> std::path::PathBuf {
    let p = dir.path().join("pi.config.toml");
    std::fs::write(
        &p,
        "[agent]\nid='main'\nworkspace='/tmp'\n\n[storage]\nwork_dir='/tmp'\n\n[llm]\nprovider='openai'\ndefault_model='gpt-4o'\n\n[workspace]\nworkspace_roots=[]\nentries=[]\n\n[primitive]\npath_rules=[]\nbash_approval_required=[]\nbash_forbidden=[]\nauto_confirm=true",
    )
    .unwrap();
    p
}

#[tokio::test]
async fn config_get_returns_value_for_allowlisted_key() {
    let dir = TempDir::new().unwrap();
    let p = empty_config(&dir);
    let cfg = load_config(Some(&p)).unwrap();
    let v = config_get_impl("llm.default_model", &cfg).unwrap();
    assert_eq!(v.as_str(), Some("gpt-4o"));
}

#[tokio::test]
async fn config_get_denies_sensitive_key() {
    let dir = TempDir::new().unwrap();
    let p = empty_config(&dir);
    let cfg = load_config(Some(&p)).unwrap();
    let err = config_get_impl("llm.api_key", &cfg).unwrap_err();
    assert!(matches!(err, AppError::Permission(_)));
}

#[tokio::test]
async fn config_set_appends_extra_root_with_allow_all_confirm() {
    let dir = TempDir::new().unwrap();
    let p = empty_config(&dir);
    let extra = dir.path().join("proj");
    std::fs::create_dir_all(&extra).unwrap();
    let confirm: Arc<dyn UserConfirmationProvider> = Arc::new(AllowAllConfirmation);
    let ctx = ConfigToolContext::new(p.clone(), confirm);
    let outcome = config_set_impl("workspace.workspace_roots", &extra.to_string_lossy(), &ctx)
        .await
        .unwrap();
    assert!(outcome.applied);
    let cfg = load_config(Some(&p)).unwrap();
    assert_eq!(cfg.workspace.workspace_roots.len(), 1);
}

#[tokio::test]
async fn config_set_extra_root_cannot_override_runtime_deny() {
    let dir = TempDir::new().unwrap();
    let p = empty_config(&dir);
    let extra = dir.path().join("denied");
    std::fs::create_dir_all(&extra).unwrap();
    let gate = DefaultPermissionGate::new(
        GateConfig {
            agent_definition_dir: dir.path().join("workspace-temp"),
            workspace_roots: vec![],
            agent_trail_readonly_dirs: vec![],
            user_path_rules: vec![PathRule::new(
                extra.to_string_lossy().to_string(),
                PathRuleMode::Deny,
            )],
            user_bash_forbidden: vec![],
            user_bash_approval: vec![],
            auto_confirm: false,
        },
        SessionGrants::new(),
    )
    .into_arc();
    let confirm: Arc<dyn UserConfirmationProvider> = Arc::new(AllowAllConfirmation);
    let ctx = ConfigToolContext::new(p.clone(), confirm).with_gate(gate);

    let err = config_set_impl("workspace.workspace_roots", &extra.to_string_lossy(), &ctx)
        .await
        .unwrap_err();
    assert!(matches!(err, AppError::Permission(_)));
    let cfg = load_config(Some(&p)).unwrap();
    assert!(cfg.workspace.workspace_roots.is_empty());
}

#[tokio::test]
async fn config_set_denies_self_escalation_keys() {
    let dir = TempDir::new().unwrap();
    let p = empty_config(&dir);
    let confirm: Arc<dyn UserConfirmationProvider> = Arc::new(AllowAllConfirmation);
    let ctx = ConfigToolContext::new(p, confirm);
    for k in [
        "primitive.bash_whitelist",
        "primitive.path_whitelist",
        "primitive.auto_confirm_whitelist",
        "primitive.auto_confirm",
        "agent.id",
        "llm.api_key",
    ] {
        let err = config_set_impl(k, "anything", &ctx).await.unwrap_err();
        assert!(
            matches!(err, AppError::Permission(_)),
            "{k} must be denied as self-escalation, got {:?}",
            err
        );
    }
}

#[tokio::test]
async fn config_set_user_denied_returns_applied_false() {
    let dir = TempDir::new().unwrap();
    let p = empty_config(&dir);
    let extra = dir.path().join("proj2");
    std::fs::create_dir_all(&extra).unwrap();
    let confirm: Arc<dyn UserConfirmationProvider> = Arc::new(DenyAllConfirmation);
    let ctx = ConfigToolContext::new(p.clone(), confirm);
    let outcome = config_set_impl("workspace.workspace_roots", &extra.to_string_lossy(), &ctx)
        .await
        .unwrap();
    assert!(!outcome.applied);
    assert_eq!(outcome.message, "user_denied");
    let cfg = load_config(Some(&p)).unwrap();
    assert!(cfg.workspace.workspace_roots.is_empty());
}

#[tokio::test]
async fn config_set_array_path_rule_appends_with_json_value() {
    let dir = TempDir::new().unwrap();
    let p = empty_config(&dir);
    let blocked = dir.path().join("blocked");
    std::fs::create_dir_all(&blocked).unwrap();
    let confirm: Arc<dyn UserConfirmationProvider> = Arc::new(AllowAllConfirmation);
    let gate = DefaultPermissionGate::new(
        GateConfig {
            agent_definition_dir: dir.path().join("workspace-temp"),
            workspace_roots: vec![],
            agent_trail_readonly_dirs: vec![],
            user_path_rules: vec![],
            user_bash_forbidden: vec![],
            user_bash_approval: vec![],
            auto_confirm: false,
        },
        SessionGrants::new(),
    )
    .into_arc();
    let ctx = ConfigToolContext::new(p.clone(), confirm).with_gate(gate.clone());
    let rule = format!(
        r#"{{"path":"{}","mode":"deny"}}"#,
        blocked.to_string_lossy()
    );
    let outcome = config_set_impl("primitive.path_rules", &rule, &ctx)
        .await
        .unwrap();
    assert!(outcome.applied);
    let cfg = load_config(Some(&p)).unwrap();
    assert_eq!(cfg.primitive.path_rules.len(), 1);
    assert_eq!(cfg.primitive.path_rules[0].path, blocked.to_string_lossy());

    let decision = gate
        .check(
            PrimitiveOperation::Read,
            blocked.join("secret.txt").to_str().unwrap(),
        )
        .unwrap();
    assert!(
        matches!(decision, PermissionDecision::Deny { .. }),
        "config_set primitive.path_rules 后，同一会话 gate 必须立即 deny，实际: {:?}",
        decision
    );
}

#[tokio::test]
async fn config_set_bash_forbidden_rejects_invalid_regex() {
    let dir = TempDir::new().unwrap();
    let p = empty_config(&dir);
    let confirm: Arc<dyn UserConfirmationProvider> = Arc::new(AllowAllConfirmation);
    let ctx = ConfigToolContext::new(p.clone(), confirm);
    let err = config_set_impl("primitive.bash_forbidden", "(unbalanced", &ctx)
        .await
        .unwrap_err();
    assert!(matches!(err, AppError::Config(_)));
    let cfg = load_config(Some(&p)).unwrap();
    assert!(cfg.primitive.bash_forbidden.is_empty());
}
