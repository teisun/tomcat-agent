use std::sync::Arc;

#[cfg(unix)]
use std::os::unix::fs::symlink;

use super::{
    config_get_impl, config_set_impl, is_array_field, is_readable, is_writable, ConfigToolContext,
};
use crate::core::agent_loop::{PackageInstallBackend, PackageInstallRequest, PackageInstallStatus};
use crate::core::permission::{
    DefaultPermissionGate, GateConfig, PathRule, PathRuleMode, PermissionDecision, SessionGrants,
};
use crate::core::tools::contract::confirmation::{
    AllowAllConfirmation, DenyAllConfirmation, UserConfirmationProvider,
};
use crate::core::tools::package_install::{ChatPackageInstallBackend, PackageInstallContext};
use crate::core::tools::primitive::PrimitiveOperation;
use crate::infra::config::{load_config, load_config_toml_file};
use crate::infra::error::AppError;
use crate::infra::AuditStore;
use serial_test::serial;
use tempfile::TempDir;

#[test]
fn read_allowlist_covers_documented_keys() {
    for k in [
        "workspace",
        "workspace.workspace_roots",
        "workspace.project_resource_dir",
        "primitive.path_rules",
        "agent.id",
        "log.level",
        "session.default_mode",
        "context.context_window_fallback",
        "context.output_reserve_tokens",
        "preflight.show_search_tools_ui",
        "preflight.show_git_ui",
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
        "workspace.project_resource_dir",
        "primitive.path_rules",
        "primitive.bash_forbidden",
        "log.level",
        "session.default_mode",
        "context.context_window_fallback",
        "context.output_reserve_tokens",
        "preflight.show_search_tools_ui",
        "preflight.show_git_ui",
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
    let work_dir = dir.path();
    let p = work_dir.join("tomcat.config.toml");
    let work_dir_s = work_dir.to_string_lossy();
    std::fs::write(
        &p,
        format!(
            "[agent]\nid='main'\nworkspace='{work_dir_s}'\n\n[storage]\nwork_dir='{work_dir_s}'\n\n[llm]\ndefault_model='gpt-5.4'\n\n[workspace]\nworkspace_roots=[]\nentries=[]\n\n[primitive]\npath_rules=[]\nbash_approval_required=[]\nbash_forbidden=[]\nauto_confirm=true"
        ),
    )
    .unwrap();
    crate::test_support::write_models_override(
        work_dir,
        &[crate::test_support::TestModelOverride::gpt54_openai_responses("OPENAI_API_KEY")],
    );
    p
}

#[tokio::test]
async fn config_get_returns_value_for_allowlisted_key() {
    let dir = TempDir::new().unwrap();
    let p = empty_config(&dir);
    let cfg = load_config(Some(&p)).unwrap();
    let v = config_get_impl("llm.default_model", &cfg).unwrap();
    assert_eq!(v.as_str(), Some("gpt-5.4"));
    let resource_dir = config_get_impl("workspace.project_resource_dir", &cfg).unwrap();
    assert_eq!(resource_dir.as_str(), Some(".agents"));
}

#[tokio::test]
async fn config_get_returns_preflight_ui_value_for_allowlisted_key() {
    let dir = TempDir::new().unwrap();
    let p = empty_config(&dir);
    std::fs::write(
        &p,
        "[agent]\nid='main'\nworkspace='/tmp'\n\n[storage]\nwork_dir='/tmp'\n\n[llm]\ndefault_model='gpt-5.4'\n\n[preflight]\nshow_git_ui=true\n\n[workspace]\nworkspace_roots=[]\nentries=[]\n\n[primitive]\npath_rules=[]\nbash_approval_required=[]\nbash_forbidden=[]\nauto_confirm=true",
    )
    .unwrap();
    let cfg = load_config(Some(&p)).unwrap();
    let v = config_get_impl("preflight.show_git_ui", &cfg).unwrap();
    assert_eq!(v.as_bool(), Some(true));
}

#[tokio::test]
async fn config_set_updates_and_validates_project_resource_dir() {
    let dir = TempDir::new().unwrap();
    let path = empty_config(&dir);
    let ctx = ConfigToolContext::new(path.clone(), Arc::new(AllowAllConfirmation));

    let updated = config_set_impl("workspace.project_resource_dir", ".team-agents", &ctx)
        .await
        .unwrap();
    assert!(updated.applied);
    assert_eq!(
        load_config(Some(&path))
            .unwrap()
            .workspace
            .project_resource_dir,
        ".team-agents"
    );

    let err = config_set_impl("workspace.project_resource_dir", ".tomcat", &ctx)
        .await
        .unwrap_err();
    assert!(matches!(err, AppError::Config(_)));
    assert_eq!(
        load_config(Some(&path))
            .unwrap()
            .workspace
            .project_resource_dir,
        ".team-agents",
        "invalid write must not replace the last valid resource directory"
    );
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
async fn config_set_updates_preflight_ui_scalar_bool() {
    let dir = TempDir::new().unwrap();
    let p = empty_config(&dir);
    let confirm: Arc<dyn UserConfirmationProvider> = Arc::new(AllowAllConfirmation);
    let ctx = ConfigToolContext::new(p.clone(), confirm);
    let outcome = config_set_impl("preflight.show_search_tools_ui", "true", &ctx)
        .await
        .unwrap();
    assert!(outcome.applied);
    let cfg = load_config(Some(&p)).unwrap();
    assert!(cfg.preflight.show_search_tools_ui);
    assert!(!cfg.preflight.show_git_ui);
}

#[tokio::test]
async fn config_set_writes_only_new_context_limit_keys() {
    let dir = TempDir::new().unwrap();
    let p = empty_config(&dir);
    let confirm: Arc<dyn UserConfirmationProvider> = Arc::new(AllowAllConfirmation);
    let ctx = ConfigToolContext::new(p.clone(), confirm);

    assert!(
        config_set_impl("context.context_window_fallback", "500000", &ctx)
            .await
            .unwrap()
            .applied
    );
    assert!(
        config_set_impl("context.output_reserve_tokens", "130000", &ctx)
            .await
            .unwrap()
            .applied
    );

    let raw = std::fs::read_to_string(&p).unwrap();
    assert!(raw.contains("context_window_fallback = 500000"));
    assert!(raw.contains("output_reserve_tokens = 130000"));
    assert!(!raw.contains("\ncontext_window ="));
    assert!(!raw.contains("\nmax_output_tokens ="));
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

#[tokio::test]
#[serial(env_lock)]
async fn config_set_bash_forbidden_does_not_persist_env_merged_values() {
    struct EnvGuard(Option<String>);
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.0 {
                Some(v) => std::env::set_var("TOMCAT__LOG__LEVEL", v),
                None => std::env::remove_var("TOMCAT__LOG__LEVEL"),
            }
        }
    }

    let dir = TempDir::new().unwrap();
    let p = empty_config(&dir);
    let confirm: Arc<dyn UserConfirmationProvider> = Arc::new(AllowAllConfirmation);
    let ctx = ConfigToolContext::new(p.clone(), confirm);
    let _guard = EnvGuard(std::env::var("TOMCAT__LOG__LEVEL").ok());
    std::env::set_var("TOMCAT__LOG__LEVEL", "trace");

    let outcome = config_set_impl("primitive.bash_forbidden", "^rm -rf /$", &ctx)
        .await
        .unwrap();
    assert!(outcome.applied);

    let cfg = load_config_toml_file(&p).unwrap();
    assert_eq!(cfg.log.level, "warn");
    assert_eq!(cfg.primitive.bash_forbidden, vec!["^rm -rf /$".to_string()]);
}

#[test]
fn package_install_request_rejects_force_unknown_fields_and_relative_sources() {
    let force = PackageInstallRequest::parse(&serde_json::json!({
        "source": "/tmp/source",
        "force": true,
    }));
    assert!(matches!(force, Err(AppError::Config(_))));

    let relative = PackageInstallRequest::parse(&serde_json::json!({
        "source": "./source",
    }));
    assert!(matches!(relative, Err(AppError::Config(_))));
}

#[tokio::test]
async fn package_install_scope_requires_a_persisted_project_root() {
    let dir = TempDir::new().unwrap();
    let config_path = empty_config(&dir);
    let source = dir.path().join("source-skill");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(
        source.join("SKILL.md"),
        "---\nname: no-project-root\ndescription: must not install\n---\n# Skill\n",
    )
    .unwrap();
    let backend = ChatPackageInstallBackend {
        ctx: PackageInstallContext::new(
            load_config(Some(&config_path)).unwrap(),
            dir.path().to_path_buf(),
            None,
            Arc::new(AllowAllConfirmation),
        ),
    };

    let error = backend
        .install(
            PackageInstallRequest::parse(&serde_json::json!({
                "source": source.to_string_lossy(),
                "scope": "scope",
            }))
            .unwrap(),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, AppError::Config(_)));
    assert!(!dir.path().join(".agents/skills/no-project-root").exists());
}

#[tokio::test]
async fn package_install_rechecks_plan_mode_before_any_filesystem_write() {
    let dir = TempDir::new().unwrap();
    let config_path = empty_config(&dir);
    let source = dir.path().join("source-skill");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(
        source.join("SKILL.md"),
        "---\nname: plan-blocked\ndescription: must not install\n---\n# Skill\n",
    )
    .unwrap();
    let runtime = crate::core::plan_runtime::PlanRuntime::new("install-plan-mode");
    runtime.enter_plan().unwrap();
    let backend = ChatPackageInstallBackend {
        ctx: PackageInstallContext::new(
            load_config(Some(&config_path)).unwrap(),
            dir.path().to_path_buf(),
            None,
            Arc::new(AllowAllConfirmation),
        )
        .with_plan_runtime(&runtime),
    };

    let error = backend
        .install(
            PackageInstallRequest::parse(&serde_json::json!({
                "source": source.to_string_lossy(),
                "scope": "agent",
            }))
            .unwrap(),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, AppError::Permission(_)));
    assert!(!dir.path().join("agents/main/skills/plan-blocked").exists());
}

#[tokio::test]
async fn package_install_confirms_then_installs_an_agent_skill() {
    let dir = TempDir::new().unwrap();
    let config_path = empty_config(&dir);
    let source = dir.path().join("source-skill");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(
        source.join("SKILL.md"),
        "---\nname: release-notes\ndescription: write release notes\n---\n# Release notes\n",
    )
    .unwrap();
    let mut config = load_config(Some(&config_path)).unwrap();
    config.security.enable_audit_log = true;
    let audit_store = Arc::new(AuditStore::open_if_enabled(&config).unwrap().unwrap());
    let backend = ChatPackageInstallBackend {
        ctx: PackageInstallContext::new(
            config,
            dir.path().to_path_buf(),
            None,
            Arc::new(AllowAllConfirmation),
        )
        .with_audit_store(Arc::clone(&audit_store)),
    };

    let result = backend
        .install(
            PackageInstallRequest::parse(&serde_json::json!({
                "source": source.to_string_lossy(),
                "scope": "agent"
            }))
            .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(result.status, PackageInstallStatus::Installed);
    assert_eq!(result.resources[0].kind, "skill");
    assert!(result.inventory_dirty);
    assert!(dir
        .path()
        .join("agents/main/skills/release-notes/SKILL.md")
        .is_file());
    let statuses = audit_store
        .query(&Default::default())
        .unwrap()
        .into_iter()
        .filter_map(|entry| match entry.payload {
            crate::infra::audit_store::AuditKindPayload::ToolCall {
                tool_name, detail, ..
            } if tool_name == "package_install" => detail,
            _ => None,
        })
        .map(|detail| serde_json::from_str::<serde_json::Value>(&detail).unwrap()["status"].clone())
        .collect::<Vec<_>>();
    assert_eq!(statuses, vec!["installed", "approved"]);
}

#[tokio::test]
async fn package_install_denial_has_no_side_effects() {
    let dir = TempDir::new().unwrap();
    let config_path = empty_config(&dir);
    let source = dir.path().join("source-skill");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(
        source.join("SKILL.md"),
        "---\nname: cancelled-skill\ndescription: must not install\n---\n# Cancelled\n",
    )
    .unwrap();
    let mut config = load_config(Some(&config_path)).unwrap();
    config.security.enable_audit_log = true;
    let audit_store = Arc::new(AuditStore::open_if_enabled(&config).unwrap().unwrap());
    let backend = ChatPackageInstallBackend {
        ctx: PackageInstallContext::new(
            config,
            dir.path().to_path_buf(),
            None,
            Arc::new(DenyAllConfirmation),
        )
        .with_audit_store(Arc::clone(&audit_store)),
    };

    let error = backend
        .install(
            PackageInstallRequest::parse(&serde_json::json!({
                "source": source.to_string_lossy(),
                "scope": "agent"
            }))
            .unwrap(),
        )
        .await
        .unwrap_err();

    assert!(matches!(error, AppError::Permission(_)));
    assert!(!dir
        .path()
        .join("agents/main/skills/cancelled-skill")
        .exists());
    let statuses = audit_store
        .query(&Default::default())
        .unwrap()
        .into_iter()
        .filter_map(|entry| match entry.payload {
            crate::infra::audit_store::AuditKindPayload::ToolCall {
                tool_name, detail, ..
            } if tool_name == "package_install" => detail,
            _ => None,
        })
        .map(|detail| serde_json::from_str::<serde_json::Value>(&detail).unwrap()["status"].clone())
        .collect::<Vec<_>>();
    assert_eq!(statuses, vec!["cancelled"]);
}

#[tokio::test]
async fn package_install_honors_a_denied_source_child_path_rule_before_confirmation() {
    let dir = TempDir::new().unwrap();
    let config_path = empty_config(&dir);
    let source = dir.path().join("blocked-source");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(
        source.join("SKILL.md"),
        "---\nname: blocked-skill\ndescription: must stay blocked\n---\n# Blocked\n",
    )
    .unwrap();
    let gate = DefaultPermissionGate::new(
        GateConfig {
            agent_definition_dir: dir.path().join("definition"),
            workspace_roots: vec![],
            agent_trail_readonly_dirs: vec![],
            user_path_rules: vec![PathRule::new(
                source.join("SKILL.md").to_string_lossy().to_string(),
                PathRuleMode::Deny,
            )],
            user_bash_forbidden: vec![],
            user_bash_approval: vec![],
            auto_confirm: false,
        },
        SessionGrants::new(),
    )
    .into_arc();
    let backend = ChatPackageInstallBackend {
        ctx: PackageInstallContext::new(
            load_config(Some(&config_path)).unwrap(),
            dir.path().to_path_buf(),
            None,
            Arc::new(AllowAllConfirmation),
        )
        .with_gate(gate),
    };

    let error = backend
        .install(
            PackageInstallRequest::parse(&serde_json::json!({
                "source": source.to_string_lossy(),
                "scope": "agent"
            }))
            .unwrap(),
        )
        .await
        .unwrap_err();

    assert!(matches!(error, AppError::Permission(_)));
    assert!(!dir.path().join("agents/main/skills/blocked-skill").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn package_install_rejects_symlink_and_special_files_in_source_tree() {
    let dir = TempDir::new().unwrap();
    let config_path = empty_config(&dir);
    let source = dir.path().join("source-skill");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(
        source.join("SKILL.md"),
        "---\nname: checked-source\ndescription: source tree must be ordinary files\n---\n# Skill\n",
    )
    .unwrap();
    symlink("/tmp", source.join("linked-child")).unwrap();
    let backend = ChatPackageInstallBackend {
        ctx: PackageInstallContext::new(
            load_config(Some(&config_path)).unwrap(),
            dir.path().to_path_buf(),
            Some(dir.path().to_path_buf()),
            Arc::new(AllowAllConfirmation),
        ),
    };
    let request = || {
        PackageInstallRequest::parse(&serde_json::json!({
            "source": source.to_string_lossy(),
            "scope": "scope",
        }))
        .unwrap()
    };

    let error = backend.install(request()).await.unwrap_err();
    assert!(matches!(error, AppError::Config(_)));
    assert!(error.to_string().contains("symbolic links"));

    std::fs::remove_file(source.join("linked-child")).unwrap();
    let fifo = source.join("special-child");
    let status = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap();
    assert!(status.success());

    let error = backend.install(request()).await.unwrap_err();
    assert!(matches!(error, AppError::Config(_)));
    assert!(error.to_string().contains("special file"));
    assert!(!dir.path().join(".agents/skills/checked-source").exists());
}

#[tokio::test]
async fn package_install_requires_ordinary_source_read_authorization() {
    let dir = TempDir::new().unwrap();
    let config_path = empty_config(&dir);
    let source = dir.path().join("foreign-source");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(
        source.join("SKILL.md"),
        "---\nname: source-needs-read\ndescription: must stay uninstalled\n---\n# Skill\n",
    )
    .unwrap();
    let gate = DefaultPermissionGate::new(
        GateConfig {
            agent_definition_dir: dir.path().join("definition"),
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
    let backend = ChatPackageInstallBackend {
        ctx: PackageInstallContext::new(
            load_config(Some(&config_path)).unwrap(),
            dir.path().to_path_buf(),
            None,
            Arc::new(DenyAllConfirmation),
        )
        .with_gate(gate),
    };

    let error = backend
        .install(
            PackageInstallRequest::parse(&serde_json::json!({
                "source": source.to_string_lossy(),
                "scope": "agent",
            }))
            .unwrap(),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, AppError::Permission(_)));
    assert!(error.to_string().contains("拒绝读取"));
    assert!(!dir
        .path()
        .join("agents/main/skills/source-needs-read")
        .exists());
}

#[tokio::test]
async fn package_install_reads_a_readonly_source_but_rejects_a_readonly_target() {
    let dir = TempDir::new().unwrap();
    let config_path = empty_config(&dir);
    let source = dir.path().join("readonly-source");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(
        source.join("SKILL.md"),
        "---\nname: readonly-source\ndescription: source is readable\n---\n# Skill\n",
    )
    .unwrap();
    let gate = DefaultPermissionGate::new(
        GateConfig {
            agent_definition_dir: dir.path().join("definition"),
            workspace_roots: vec![],
            agent_trail_readonly_dirs: vec![],
            user_path_rules: vec![
                PathRule::new(source.to_string_lossy().to_string(), PathRuleMode::Readonly),
                PathRule::new(
                    dir.path().join("agents").to_string_lossy().to_string(),
                    PathRuleMode::Readonly,
                ),
            ],
            user_bash_forbidden: vec![],
            user_bash_approval: vec![],
            auto_confirm: false,
        },
        SessionGrants::new(),
    )
    .into_arc();
    let backend = ChatPackageInstallBackend {
        ctx: PackageInstallContext::new(
            load_config(Some(&config_path)).unwrap(),
            dir.path().to_path_buf(),
            None,
            Arc::new(AllowAllConfirmation),
        )
        .with_gate(gate),
    };

    let error = backend
        .install(
            PackageInstallRequest::parse(&serde_json::json!({
                "source": source.to_string_lossy(),
                "scope": "agent",
            }))
            .unwrap(),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, AppError::Permission(_)));
    assert!(error.to_string().contains("目标被路径策略拒绝"));
    assert!(!dir
        .path()
        .join("agents/main/skills/readonly-source")
        .exists());
}
