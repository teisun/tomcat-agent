use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use tracing::warn;

use crate::core::connector::mcp::executor::McpToolExecutor;
use crate::core::connector::mcp::manager::McpManager;
use crate::core::tools::contract::registry::{Tool, ToolExecutor};
use crate::infra::error::AppError;
use crate::AppConfig;

pub mod mcp;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectorType {
    Mcp,
    Cli,
    A2a,
}

pub trait Connector: Send + Sync {
    fn connector_type(&self) -> ConnectorType;
}

impl Connector for McpManager {
    fn connector_type(&self) -> ConnectorType {
        ConnectorType::Mcp
    }
}

/// Routes registry calls by their owner namespace without changing the existing
/// JSON-only ToolExecutor contract.
pub struct CompositeToolExecutor {
    plugin_executor: Arc<dyn ToolExecutor>,
    mcp_executor: Arc<dyn ToolExecutor>,
}

impl CompositeToolExecutor {
    pub fn new(
        plugin_executor: Arc<dyn ToolExecutor>,
        mcp_executor: Arc<dyn ToolExecutor>,
    ) -> Arc<Self> {
        Arc::new(Self {
            plugin_executor,
            mcp_executor,
        })
    }
}

#[async_trait]
impl ToolExecutor for CompositeToolExecutor {
    async fn execute(
        &self,
        tool: &Tool,
        params: serde_json::Value,
        caller_plugin_id: &str,
        session_id: Option<&str>,
    ) -> Result<serde_json::Value, AppError> {
        let executor = if tool.plugin_id.starts_with("mcp:") {
            &self.mcp_executor
        } else {
            &self.plugin_executor
        };
        executor
            .execute(tool, params, caller_plugin_id, session_id)
            .await
    }
}

/// Owns connector configuration and lifecycle orchestration. MCP catalogs live
/// exclusively in `McpManager`; they are queried through stable `tool_*`
/// builtins and never registered into the prompt-facing ToolRegistry.
pub struct ConnectorRegistry {
    enabled: bool,
    config: AppConfig,
    mcp: Arc<McpManager>,
    started: AtomicBool,
}

impl ConnectorRegistry {
    pub fn new(
        cfg: &AppConfig,
        workspace_root: Option<&std::path::Path>,
    ) -> Result<Arc<Self>, AppError> {
        Ok(Arc::new(Self {
            enabled: cfg.connector.enabled,
            config: cfg.clone(),
            mcp: McpManager::new(cfg, workspace_root)?,
            started: AtomicBool::new(false),
        }))
    }

    pub fn mcp_manager(&self) -> Arc<McpManager> {
        self.mcp.clone()
    }

    pub fn mcp_executor(&self) -> Arc<dyn ToolExecutor> {
        McpToolExecutor::new(self.mcp.clone())
    }

    /// Tool-surface gating must derive from config rather than asynchronous Ready state.
    pub fn has_configured_mcp_servers(&self) -> bool {
        self.enabled && self.mcp.has_configured_servers()
    }

    /// Starts trusted configured MCP servers concurrently. This method only
    /// spawns tasks; it never waits for a server and therefore cannot delay a
    /// chat's first request or a serve handshake.
    pub async fn spawn_connect_all(self: &Arc<Self>) {
        if !self.enabled || self.started.swap(true, Ordering::AcqRel) {
            return;
        }
        for status in self.mcp.statuses() {
            let manager = self.mcp.clone();
            tokio::spawn(async move {
                connect_with_backoff(manager, status.name).await;
            });
        }
    }

    pub async fn approve_and_connect(&self, server_name: &str) -> Result<(), AppError> {
        self.mcp.approve(server_name)?;
        self.mcp.connect_server(server_name).await
    }

    pub fn deny(&self, server_name: &str) -> Result<(), AppError> {
        self.mcp.deny(server_name)
    }

    pub async fn reload(&self) -> Result<(), AppError> {
        let server_names = self.mcp.reload_configuration(&self.config)?;
        for server_name in server_names {
            if let Err(error) = self.mcp.connect_server(&server_name).await {
                warn!(server = %server_name, error = %error, "MCP server did not become ready after reload");
            }
        }
        Ok(())
    }
}

async fn connect_with_backoff(manager: Arc<McpManager>, server_name: String) {
    for delay in [0_u64, 250, 1_000] {
        if delay > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
        }
        match manager.connect_server(&server_name).await {
            Ok(()) => return,
            Err(error) => {
                warn!(
                    server = %server_name,
                    error = %error,
                    "MCP server did not become ready during background startup"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
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

        let connectors =
            ConnectorRegistry::new(&cfg, Some(&workspace)).expect("connector registry");
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

        connectors.deny("fake").expect("deny MCP server");
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

        let connectors =
            ConnectorRegistry::new(&cfg, Some(&workspace)).expect("connector registry");
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

        let connectors =
            ConnectorRegistry::new(&cfg, Some(&workspace)).expect("connector registry");
        let registry_impl = Arc::new(DefaultToolRegistry::new(
            connectors.mcp_executor(),
            Arc::new(TracingAuditRecorder),
        ));
        tokio::time::timeout(Duration::from_millis(100), connectors.spawn_connect_all())
            .await
            .expect("startup must return before a server finishes connecting");
        assert!(registry_impl.get_tool("mcp__fake__capture").await.is_err());
    }
}
