use std::sync::Arc;

use async_trait::async_trait;

use crate::core::connector::mcp::manager::McpManager;
use crate::core::tools::contract::registry::{Tool, ToolExecutor};
use crate::infra::error::AppError;

pub struct McpToolExecutor {
    manager: Arc<McpManager>,
}

impl McpToolExecutor {
    pub fn new(manager: Arc<McpManager>) -> Arc<Self> {
        Arc::new(Self { manager })
    }
}

#[async_trait]
impl ToolExecutor for McpToolExecutor {
    async fn execute(
        &self,
        tool: &Tool,
        params: serde_json::Value,
        _caller_plugin_id: &str,
        _session_id: Option<&str>,
    ) -> Result<serde_json::Value, AppError> {
        let server = tool.plugin_id.strip_prefix("mcp:").ok_or_else(|| {
            AppError::Tool(format!(
                "MCP executor received a non-MCP tool: {} ({})",
                tool.name, tool.plugin_id
            ))
        })?;
        // Call failures are not connection failures. Recovery belongs to the
        // manager's explicit connection-event path, never this adapter.
        self.manager.call_tool(server, &tool.name, params).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rpc_failure_does_not_start_a_second_recovery_loop() {
        let temp = tempfile::tempdir().unwrap();
        let log = temp.path().join("methods.log");
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/mcp/fake_stdio_server.mjs");
        let mut cfg = crate::AppConfig::default();
        cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
        super::super::config::add_global_server(
            &cfg,
            "fake".into(),
            serde_json::from_value(
                serde_json::json!({"command":"node", "args":[fixture,"--record",log]}),
            )
            .unwrap(),
        )
        .unwrap();
        let manager = McpManager::new(&cfg, None).unwrap();
        manager.connect_server("fake").await.unwrap();
        let executor = McpToolExecutor::new(manager.clone());
        let tool = Tool {
            name: "mcp__fake__capture".into(),
            label: "capture".into(),
            description: String::new(),
            parameters: serde_json::json!({"type":"object"}),
            plugin_id: "mcp:fake".into(),
            is_enabled: true,
            created_at: 0,
        };
        let error = executor
            .execute(
                &tool,
                serde_json::json!({"rpcError":true}),
                "test",
                Some("session-A"),
            )
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("-32602"),
            "must reach the RPC error, not a lookup failure: {error}"
        );
        // The removed adapter loop used to reconnect after 250ms on any error.
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        executor
            .execute(&tool, serde_json::json!({}), "test", Some("session-B"))
            .await
            .unwrap();
        let methods = tokio::fs::read_to_string(&log).await.unwrap();
        assert_eq!(methods.lines().filter(|m| *m == "tools/list").count(), 1);
        assert_eq!(methods.lines().filter(|m| *m == "tools/call").count(), 2);
        manager.remove_configured_server("fake", &cfg).unwrap();
    }
}
