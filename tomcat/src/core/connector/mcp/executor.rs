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
#[path = "tests/executor_test.rs"]
mod tests;
