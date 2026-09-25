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

    let outcome = run(&ctx, ConnectorCommand::List).await;
    assert!(matches!(outcome, ChatCommandOutcome::Handled));

    let root = ctx.scope_services.session_project_root.as_deref().unwrap();
    let store = crate::core::security::project_trust::ProjectTrustStore::open(&cfg).unwrap();
    assert!(!store.is_trusted(root).unwrap());
    let outcome = run(&ctx, ConnectorCommand::TrustProject).await;
    assert!(matches!(outcome, ChatCommandOutcome::Handled));
    assert!(store.is_trusted(root).unwrap());
}
