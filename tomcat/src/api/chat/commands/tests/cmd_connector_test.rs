use std::ffi::OsString;

use super::super::cmd_connector::run;
use crate::api::chat::commands::{
    parse_chat_command, ChatCommand, ChatCommandOutcome, ConnectorCommand,
};
use crate::api::chat::{ChatContext, ChatContextOverrides};
use crate::infra::config::get_work_dir;
use crate::AppConfig;
use serial_test::serial;

struct EnvGuard {
    key: &'static str,
    old: Option<OsString>,
}

impl EnvGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let old = std::env::var_os(key);
        unsafe { std::env::set_var(key, value) };
        Self { key, old }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.old {
            Some(value) => unsafe { std::env::set_var(self.key, value) },
            None => unsafe { std::env::remove_var(self.key) },
        }
    }
}

#[test]
fn parses_connector_add_with_command_and_args() {
    assert_eq!(
        parse_chat_command(r#"/connector add playwright npx -y @playwright/mcp@0.0.79"#),
        ChatCommand::Connector(ConnectorCommand::Add {
            name: "playwright".to_string(),
            command: "npx".to_string(),
            args: vec!["-y".to_string(), "@playwright/mcp@0.0.79".to_string()],
        })
    );
}

#[test]
fn parses_connector_tool_filter_flags() {
    assert_eq!(
        parse_chat_command(
            r#"/connector tools playwright --include "browser_*" --exclude "browser_save_as_pdf""#
        ),
        ChatCommand::Connector(ConnectorCommand::Tools {
            name: "playwright".to_string(),
            include: vec!["browser_*".to_string()],
            exclude: vec!["browser_save_as_pdf".to_string()],
        })
    );
}

#[test]
fn connector_rejects_invalid_subcommands() {
    assert!(matches!(
        parse_chat_command("/connector add"),
        ChatCommand::UsageError { .. }
    ));
}

#[test]
fn project_trust_is_one_command_without_server_identity() {
    assert_eq!(
        parse_chat_command("/connector trust-project"),
        ChatCommand::Connector(ConnectorCommand::TrustProject)
    );
    for command in [
        "/connector trust fake",
        "/connector deny fake",
        "/connector trust-project fake",
    ] {
        assert!(matches!(
            parse_chat_command(command),
            ChatCommand::UsageError { .. }
        ));
    }
}

#[tokio::test]
#[serial(env_lock)]
async fn cmd_connector_add_and_list_round_trips_filtered_tools() {
    const API_KEY_ENV: &str = "TOMCAT_CMD_CONNECTOR_TEST_KEY";

    let temp = tempfile::tempdir().expect("temporary directory");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let _api_key = EnvGuard::set(API_KEY_ENV, "test-key");
    let mut cfg = AppConfig::default();
    cfg.connector.enabled = true;
    cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
    cfg.llm.api_key_env = Some(API_KEY_ENV.to_string());
    let ctx = ChatContext::from_config_with_overrides(
        cfg.clone(),
        ChatContextOverrides::default().with_session_cwd_override(workspace),
    )
    .expect("chat context");
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/mcp/fake_stdio_server.mjs");

    let outcome = run(
        &ctx,
        ConnectorCommand::Add {
            name: "fake".to_string(),
            command: "node".to_string(),
            args: vec![fixture.to_string_lossy().into_owned()],
        },
    )
    .await;
    assert!(matches!(outcome, ChatCommandOutcome::Handled));
    let mcp_file = get_work_dir(&cfg).expect("work dir").join("mcp.json");
    let config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(mcp_file).expect("read MCP config"))
            .expect("parse MCP config");
    assert_eq!(config["mcpServers"]["fake"]["command"], "node");

    let outcome = run(
        &ctx,
        ConnectorCommand::Tools {
            name: "fake".to_string(),
            include: vec!["capture".to_string()],
            exclude: vec![],
        },
    )
    .await;
    assert!(matches!(outcome, ChatCommandOutcome::Handled));
    let tools = ctx
        .global_services
        .connector_registry
        .as_ref()
        .expect("enabled connector registry")
        .mcp_manager()
        .tool_defs("fake");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].raw_name, "capture");
    let own_manager = ctx
        .global_services
        .connector_registry
        .as_ref()
        .unwrap()
        .mcp_manager();
    let healthy_generation = own_manager.statuses()[0].generation.clone();
    run(&ctx, ConnectorCommand::Reload).await;
    assert_eq!(
        own_manager.statuses()[0].generation,
        healthy_generation,
        "explicit reload must retain an unchanged healthy connection"
    );

    let outcome = run(&ctx, ConnectorCommand::List).await;
    assert!(matches!(outcome, ChatCommandOutcome::Handled));

    let root = ctx.scope_services.session_project_root.as_deref().unwrap();
    let store = crate::core::security::project_trust::ProjectTrustStore::open(&cfg).unwrap();
    assert!(!store.is_trusted(root).unwrap());
    let outcome = run(&ctx, ConnectorCommand::TrustProject).await;
    assert!(matches!(outcome, ChatCommandOutcome::Handled));
    assert!(store.is_trusted(root).unwrap());

    let other_workspace = temp.path().join("other-workspace");
    std::fs::create_dir_all(&other_workspace).unwrap();
    let other =
        crate::core::connector::ConnectorRegistry::new(&cfg, Some(&other_workspace)).unwrap();
    let manager = other.mcp_manager();
    manager.connect_server("fake").await.unwrap();
    let key = manager.configured_server("fake").unwrap().config_key;
    let credentials = crate::core::connector::mcp::oauth::OAuthTokenStore::open(&cfg).unwrap();
    credentials
        .save_static_bearer(&key, "fixture-token".into(), None)
        .unwrap();
    assert!(!manager.tool_defs(&key).is_empty());
    let outcome = run(
        &ctx,
        ConnectorCommand::Logout {
            name: "fake".into(),
        },
    )
    .await;
    let other_retired = manager.statuses()[0].state
        == crate::core::connector::mcp::manager::ServerState::Disconnected;
    let removed = credentials.load(&key).unwrap().is_none();
    other.logout_server(&key).unwrap();
    assert!(matches!(outcome, ChatCommandOutcome::Handled));
    assert!(
        removed && other_retired,
        "CLI logout must use the shared registry workflow"
    );

    // Explicit reload is not the same as post-save reconciliation: it must
    // resume an unchanged stopped source. Removal must revoke, not just edit JSON.
    manager.reconnect_server(&key).await.unwrap();
    let model_name = manager.tool_defs(&key)[0].model_name.clone();
    run(&ctx, ConnectorCommand::Reload).await;
    let current_registry = ctx.global_services.connector_registry.as_ref().unwrap();
    let explicitly_reconnected = current_registry.mcp_manager().statuses()[0].state
        == crate::core::connector::mcp::manager::ServerState::Ready;
    credentials
        .save_static_bearer(&key, "remove-fixture-token".into(), None)
        .unwrap();
    run(
        &ctx,
        ConnectorCommand::Remove {
            name: "fake".into(),
        },
    )
    .await;
    let credentials_removed = credentials.load(&key).unwrap().is_none();
    let other_call_rejected = manager
        .call_tool(&key, &model_name, serde_json::json!({}))
        .await
        .is_err();
    run(
        &ctx,
        ConnectorCommand::Add {
            name: "fake".into(),
            command: "node".into(),
            args: vec![fixture.to_string_lossy().into_owned()],
        },
    )
    .await;
    let readd_has_no_old_token = credentials.load(&key).unwrap().is_none();
    // Also clean both contexts on the pre-fix path before reporting failures.
    current_registry.logout_server("fake").unwrap();
    assert!(explicitly_reconnected && credentials_removed && other_call_rejected && readd_has_no_old_token,
        "reload_connected={explicitly_reconnected}, token_removed={credentials_removed}, removed_source_rejected={other_call_rejected}, readd_token_absent={readd_has_no_old_token}");

    current_registry
        .mcp_manager()
        .reconnect_server("fake")
        .await
        .unwrap();
    manager.reload_configuration(&cfg).unwrap();
    manager.reconnect_server("fake").await.unwrap();
    credentials
        .save_static_bearer(&key, "partial-removal-fixture".into(), None)
        .unwrap();
    let credential_path = get_work_dir(&cfg).unwrap().join("connector-oauth.json");
    let original = std::fs::read(&credential_path).unwrap();
    std::fs::write(&credential_path, "invalid-json").unwrap();
    run(
        &ctx,
        ConnectorCommand::Remove {
            name: "fake".into(),
        },
    )
    .await;
    assert!(current_registry
        .mcp_manager()
        .configured_server("fake")
        .is_none());
    assert!(
        manager.configured_server("fake").is_none(),
        "a credential error after the config commit must still reconcile other clients"
    );
    std::fs::write(&credential_path, original).unwrap();
    run(
        &ctx,
        ConnectorCommand::Remove {
            name: "fake".into(),
        },
    )
    .await;
    assert!(
        credentials.load(&key).unwrap().is_none(),
        "retry must finish credential cleanup"
    );
}
