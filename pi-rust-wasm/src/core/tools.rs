//! # 工具注册中心 Trait 与类型（与 design CODE_BLOCK_P1_007 一致）

use crate::infra::error::AppError;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tool {
    pub name: String,
    pub label: String,
    pub description: String,
    pub parameters: serde_json::Value,
    pub plugin_id: String,
    pub is_enabled: bool,
    pub created_at: i64,
}

/// 工具注册中心 Trait（与 design CODE_BLOCK_P1_007 一致）。
#[async_trait]
pub trait ToolRegistry: Send + Sync + 'static {
    async fn register_tool(&self, tool: Tool, plugin_id: &str) -> Result<(), AppError>;
    async fn unregister_tool(&self, tool_name: &str, plugin_id: &str) -> Result<(), AppError>;
    async fn get_tool(&self, tool_name: &str) -> Result<Tool, AppError>;
    async fn list_tools(&self, plugin_id: Option<&str>) -> Result<Vec<Tool>, AppError>;
    async fn call_tool(
        &self,
        tool_name: &str,
        params: serde_json::Value,
        plugin_id: &str,
    ) -> Result<serde_json::Value, AppError>;
    fn unregister_plugin_tools(&self, plugin_id: &str);
}
