use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use super::{CompositeToolExecutor, ConnectorRegistry};
use crate::core::tools::contract::registry::{
    DefaultToolRegistry, Tool, ToolExecutor, ToolRegistry,
};
use crate::infra::{config::get_work_dir, error::AppError, TracingAuditRecorder};
use crate::AppConfig;

struct MarkerExecutor(&'static str);

#[async_trait]
impl ToolExecutor for MarkerExecutor {
    async fn execute(
        &self,
        _tool: &Tool,
        _params: serde_json::Value,
        _caller_plugin_id: &str,
        _session_id: Option<&str>,
    ) -> Result<serde_json::Value, AppError> {
        Ok(serde_json::json!({"marker": self.0}))
    }
}

fn tool(plugin_id: &str) -> Tool {
    Tool {
        name: "tool".to_string(),
        label: "tool".to_string(),
        description: String::new(),
        parameters: serde_json::json!({}),
        plugin_id: plugin_id.to_string(),
        is_enabled: true,
        created_at: 0,
    }
}

fn fake_mcp_config(fixture_args: Vec<String>) -> serde_json::Value {
    serde_json::json!({
        "mcpServers": {
            "fake": { "command": "node", "args": fixture_args }
        }
    })
}

fn fake_fixture_args(extra: &[&str]) -> Vec<String> {
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/mcp/fake_stdio_server.mjs");
    std::iter::once(fixture.to_string_lossy().into_owned())
        .chain(extra.iter().map(|argument| (*argument).to_string()))
        .collect()
}

#[tokio::test]
async fn composite_routes_by_plugin_id() {
    let executor = CompositeToolExecutor::new(
        Arc::new(MarkerExecutor("plugin")),
        Arc::new(MarkerExecutor("mcp")),
    );
    let plugin = executor
        .execute(&tool("plugin:sample"), serde_json::json!({}), "agent", None)
        .await
        .expect("plugin call");
    let mcp = executor
        .execute(&tool("mcp:sample"), serde_json::json!({}), "agent", None)
        .await
        .expect("MCP call");
    assert_eq!(plugin["marker"], "plugin");
    assert_eq!(mcp["marker"], "mcp");
}

#[tokio::test]
async fn ready_mcp_tools_stay_in_manager_and_never_reach_shared_tool_registry() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let mut cfg = AppConfig::default();
    cfg.connector.enabled = true;
    cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
    let config_path = get_work_dir(&cfg).expect("work dir").join("mcp.json");
    std::fs::create_dir_all(config_path.parent().expect("config parent"))
        .expect("config directory");
    std::fs::write(
        config_path,
        fake_mcp_config(fake_fixture_args(&[])).to_string(),
    )
    .expect("write MCP config");

    let connectors = ConnectorRegistry::new(&cfg, Some(&workspace)).expect("connector registry");
    let registry_impl = Arc::new(DefaultToolRegistry::new(
        Arc::new(MarkerExecutor("plugin")),
        Arc::new(TracingAuditRecorder),
    ));
    connectors.spawn_connect_all().await;
    for _ in 0..50 {
        if !connectors.mcp_manager().tool_defs("fake").is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        !connectors.mcp_manager().tool_defs("fake").is_empty(),
        "the deferred catalog should be populated after MCP connects"
    );
    assert!(
        registry_impl.get_tool("mcp__fake__capture").await.is_err(),
        "MCP tools must not enter the prompt-facing registry"
    );

    connectors
        .mcp_manager()
        .remove_configured_server("fake", &cfg)
        .expect("remove MCP server");
    assert!(connectors.mcp_manager().tool_defs("fake").is_empty());
}

#[tokio::test]
async fn connector_disabled_master_switch_does_not_connect_or_register_tools() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let mut cfg = AppConfig::default();
    cfg.connector.enabled = false;
    cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
    let config_path = get_work_dir(&cfg).expect("work dir").join("mcp.json");
    std::fs::create_dir_all(config_path.parent().expect("config parent"))
        .expect("config directory");
    std::fs::write(
        config_path,
        fake_mcp_config(fake_fixture_args(&[])).to_string(),
    )
    .expect("write MCP config");

    let connectors = ConnectorRegistry::new(&cfg, Some(&workspace)).expect("connector registry");
    let registry_impl = Arc::new(DefaultToolRegistry::new(
        connectors.mcp_executor(),
        Arc::new(TracingAuditRecorder),
    ));
    connectors.spawn_connect_all().await;

    tokio::time::sleep(Duration::from_millis(75)).await;
    assert!(registry_impl.get_tool("mcp__fake__capture").await.is_err());
    assert!(matches!(
        connectors
            .mcp_manager()
            .statuses()
            .pop()
            .expect("configured status")
            .state,
        crate::core::connector::mcp::manager::ServerState::Pending
    ));
}

#[test]
fn cache_distinguishes_each_runtime_parameter_but_reuses_identical_values() {
    let temp = tempfile::tempdir().unwrap();
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
    let baseline = ConnectorRegistry::new(&cfg, None).unwrap();
    assert!(Arc::ptr_eq(
        &baseline,
        &ConnectorRegistry::new(&cfg, None).unwrap()
    ));
    for change in [
        (30_001, 120_000, 16),
        (30_000, 120_001, 16),
        (30_000, 120_000, 1),
    ] {
        let mut modified = cfg.clone();
        modified.connector.mcp.startup_timeout_ms = change.0;
        modified.connector.mcp.call_timeout_ms = change.1;
        modified.connector.mcp.max_concurrent_calls = change.2;
        let distinct = ConnectorRegistry::new(&modified, None).unwrap();
        assert!(!Arc::ptr_eq(&baseline, &distinct));
        assert!(Arc::ptr_eq(
            &distinct,
            &ConnectorRegistry::new(&modified, None).unwrap()
        ));
    }
}

#[test]
fn global_config_changes_reconcile_every_cached_workspace_registry() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let first_workspace = temp.path().join("first-workspace");
    let second_workspace = temp.path().join("second-workspace");
    std::fs::create_dir_all(&first_workspace).expect("first workspace");
    std::fs::create_dir_all(&second_workspace).expect("second workspace");
    let mut cfg = AppConfig::default();
    cfg.connector.enabled = true;
    cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
    let config_path = get_work_dir(&cfg).expect("work dir").join("mcp.json");
    std::fs::create_dir_all(config_path.parent().expect("config parent"))
        .expect("config directory");
    std::fs::write(
        &config_path,
        fake_mcp_config(fake_fixture_args(&[])).to_string(),
    )
    .expect("write global config");

    let first = ConnectorRegistry::new(&cfg, Some(&first_workspace)).expect("first registry");
    let second = ConnectorRegistry::new(&cfg, Some(&second_workspace)).expect("second registry");
    let key = first
        .mcp_manager()
        .configured_server("fake")
        .expect("first global connector")
        .config_key;
    assert_eq!(second.mcp_manager().statuses().len(), 1);

    assert!(first
        .mcp_manager()
        .remove_configured_server(&key, &cfg)
        .expect("remove global connector"));
    ConnectorRegistry::synchronize_cached_config_change(
        &cfg,
        crate::core::connector::mcp::config::McpConfigSource::Global,
        None,
    )
    .expect("synchronize global change");

    assert!(first.mcp_manager().statuses().is_empty());
    assert!(second.mcp_manager().statuses().is_empty());
}

#[test]
fn project_config_changes_only_reload_caches_using_the_changed_file() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let mut sender_cfg = AppConfig::default();
    sender_cfg.connector.enabled = true;
    sender_cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
    sender_cfg.workspace.project_resource_dir = ".sender".to_string();
    let mut receiver_cfg = sender_cfg.clone();
    receiver_cfg.workspace.project_resource_dir = ".receiver".to_string();
    let global = get_work_dir(&sender_cfg)
        .expect("work dir")
        .join("mcp.json");
    std::fs::create_dir_all(global.parent().expect("global parent")).expect("global parent");
    std::fs::write(global, fake_mcp_config(fake_fixture_args(&[])).to_string())
        .expect("write global config");
    for (cfg, name) in [
        (&sender_cfg, "sender-only"),
        (&receiver_cfg, "receiver-only"),
    ] {
        let project = crate::core::connector::mcp::config::project_mcp_path(cfg, &workspace)
            .expect("project config path");
        std::fs::create_dir_all(project.parent().expect("project parent")).expect("project parent");
        std::fs::write(
            project,
            serde_json::json!({ "mcpServers": { name: { "command": "node", "args": [] } } })
                .to_string(),
        )
        .expect("write project config");
    }
    let sender = ConnectorRegistry::new(&sender_cfg, Some(&workspace)).expect("sender registry");
    let receiver =
        ConnectorRegistry::new(&receiver_cfg, Some(&workspace)).expect("receiver registry");

    ConnectorRegistry::synchronize_cached_config_change(
        &sender_cfg,
        crate::core::connector::mcp::config::McpConfigSource::Project,
        Some(&workspace),
    )
    .expect("synchronize sender project config");

    let receiver_names = receiver
        .mcp_manager()
        .statuses()
        .into_iter()
        .map(|status| status.name)
        .collect::<Vec<_>>();
    assert!(receiver_names.contains(&"receiver-only".to_string()));
    assert!(!receiver_names.contains(&"sender-only".to_string()));
    assert!(sender
        .mcp_manager()
        .statuses()
        .into_iter()
        .any(|status| status.name == "sender-only"));
}

#[test]
fn global_config_changes_keep_each_receiver_disabled_set() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let mut sender_cfg = AppConfig::default();
    sender_cfg.connector.enabled = true;
    sender_cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
    let mut receiver_cfg = sender_cfg.clone();
    receiver_cfg.connector.disabled = vec!["fake".to_string()];
    let global = get_work_dir(&sender_cfg)
        .expect("work dir")
        .join("mcp.json");
    std::fs::create_dir_all(global.parent().expect("global parent")).expect("global parent");
    std::fs::write(global, fake_mcp_config(fake_fixture_args(&[])).to_string())
        .expect("write global config");
    let _sender = ConnectorRegistry::new(&sender_cfg, Some(&workspace)).expect("sender registry");
    let receiver =
        ConnectorRegistry::new(&receiver_cfg, Some(&workspace)).expect("receiver registry");
    assert!(receiver.mcp_manager().statuses().is_empty());

    ConnectorRegistry::synchronize_cached_config_change(
        &sender_cfg,
        crate::core::connector::mcp::config::McpConfigSource::Global,
        None,
    )
    .expect("synchronize global config");

    assert!(receiver.mcp_manager().statuses().is_empty());
}
#[tokio::test]
async fn factory_method_auto_starts_global_and_trusted_project_servers() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let mut cfg = AppConfig::default();
    cfg.connector.enabled = true;
    cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());

    let global_path = get_work_dir(&cfg).expect("work dir").join("mcp.json");
    std::fs::create_dir_all(global_path.parent().expect("global parent")).expect("global parent");
    std::fs::write(
        &global_path,
        serde_json::json!({
            "mcpServers": {
                "global-fake": { "command": "node", "args": fake_fixture_args(&[]) }
            }
        })
        .to_string(),
    )
    .expect("global MCP config");

    let project_path = crate::core::connector::mcp::config::project_mcp_path(&cfg, &workspace)
        .expect("project MCP path");
    std::fs::create_dir_all(project_path.parent().expect("project parent"))
        .expect("project parent");
    std::fs::write(
        &project_path,
        serde_json::json!({
            "mcpServers": {
                "project-fake": { "command": "node", "args": fake_fixture_args(&[]) }
            }
        })
        .to_string(),
    )
    .expect("project MCP config");

    let root = crate::core::security::project_trust::ProjectTrustStore::root_for(&workspace)
        .expect("project root");
    crate::core::security::project_trust::ProjectTrustStore::open(&cfg)
        .expect("trust store")
        .trust(&root)
        .expect("trust project");

    let registry = ConnectorRegistry::create_and_start(&cfg, Some(&workspace))
        .expect("create and start connector registry");
    assert!(registry.mcp_manager().project_trusted());
    for (server, model_name) in [
        ("global-fake", "mcp__global-fake__capture"),
        ("project-fake", "mcp__project-fake__capture"),
    ] {
        for _ in 0..100 {
            if !registry.mcp_manager().tool_defs(server).is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            !registry.mcp_manager().tool_defs(server).is_empty(),
            "{model_name} did not become available"
        );
    }
}

#[tokio::test]
async fn direct_new_does_not_auto_start_connections() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let mut cfg = AppConfig::default();
    cfg.connector.enabled = true;
    cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
    let config_path = get_work_dir(&cfg).expect("work dir").join("mcp.json");
    std::fs::create_dir_all(config_path.parent().expect("config parent"))
        .expect("config directory");
    std::fs::write(
        &config_path,
        fake_mcp_config(fake_fixture_args(&[])).to_string(),
    )
    .expect("MCP config");

    let registry = ConnectorRegistry::new(&cfg, Some(&workspace)).expect("registry");
    assert_eq!(registry.mcp_manager().statuses().len(), 1);
    assert!(matches!(
        registry.mcp_manager().statuses()[0].state,
        crate::core::connector::mcp::manager::ServerState::Pending
    ));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(registry.mcp_manager().tool_defs("fake").is_empty());
}

#[test]
fn create_and_start_reuses_the_cached_registry_and_is_idempotent() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let mut cfg = AppConfig::default();
    cfg.connector.enabled = true;
    cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
    let first = ConnectorRegistry::create_and_start(&cfg, None).expect("first registry");
    let second = ConnectorRegistry::create_and_start(&cfg, None).expect("second registry");
    assert!(Arc::ptr_eq(&first, &second));
}

#[tokio::test]
async fn factory_method_leaves_untrusted_project_servers_awaiting_trust() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let mut cfg = AppConfig::default();
    cfg.connector.enabled = true;
    cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
    let project_path = crate::core::connector::mcp::config::project_mcp_path(&cfg, &workspace)
        .expect("project MCP path");
    std::fs::create_dir_all(project_path.parent().expect("project parent"))
        .expect("project parent");
    std::fs::write(
        project_path,
        serde_json::json!({
            "mcpServers": {
                "project-fake": { "command": "node", "args": fake_fixture_args(&[]) }
            }
        })
        .to_string(),
    )
    .expect("project MCP config");

    let registry = ConnectorRegistry::create_and_start(&cfg, Some(&workspace))
        .expect("create and start connector registry");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        registry.mcp_manager().statuses()[0].state,
        crate::core::connector::mcp::manager::ServerState::AwaitingProjectTrust
    );
    assert!(registry.mcp_manager().tool_defs("project-fake").is_empty());
}

#[tokio::test]
async fn startup_connect_is_non_blocking() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let mut cfg = AppConfig::default();
    cfg.connector.enabled = true;
    cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
    let config_path = get_work_dir(&cfg).expect("work dir").join("mcp.json");
    std::fs::create_dir_all(config_path.parent().expect("config parent"))
        .expect("config directory");
    std::fs::write(
        config_path,
        fake_mcp_config(fake_fixture_args(&["--hang-startup"])).to_string(),
    )
    .expect("write MCP config");

    let connectors = ConnectorRegistry::new(&cfg, Some(&workspace)).expect("connector registry");
    let registry_impl = Arc::new(DefaultToolRegistry::new(
        connectors.mcp_executor(),
        Arc::new(TracingAuditRecorder),
    ));
    tokio::time::timeout(Duration::from_millis(100), connectors.spawn_connect_all())
        .await
        .expect("startup must return before a server finishes connecting");
    assert!(registry_impl.get_tool("mcp__fake__capture").await.is_err());
}
