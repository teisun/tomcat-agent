//! # 工具注册中心 Trait 与类型（与 design CODE_BLOCK_P1_007 一致）

use crate::infra::error::AppError;
use crate::infra::{AuditRecorder, ToolAuditEntry};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tool {
    pub name: String,
    pub label: String,
    pub description: String,
    /// JSON Schema，与 pi-mono parameters 一致
    pub parameters: serde_json::Value,
    pub plugin_id: String,
    pub is_enabled: bool,
    pub created_at: i64,
}

/// 工具执行器：由 008 注入，实际执行由 Wasm 插件完成；call_tool 时调用。
#[async_trait]
pub trait ToolExecutor: Send + Sync + 'static {
    /// 执行工具，返回原始结果；DefaultToolRegistry 会封装为 AgentToolResult 形态。
    async fn execute(
        &self,
        tool: &Tool,
        params: serde_json::Value,
        caller_plugin_id: &str,
    ) -> Result<serde_json::Value, AppError>;
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

fn tool_key(plugin_id: &str, name: &str) -> String {
    format!("{}::{}", plugin_id, name)
}

/// 返回值形态与 AgentToolResult 一致：content（Vec<ContentBlock> 等价）、details。
fn wrap_tool_result(
    content: serde_json::Value,
    details: Option<serde_json::Value>,
) -> serde_json::Value {
    serde_json::json!({
        "content": content,
        "details": details
    })
}

/// 默认工具注册中心：内存存储，支持注入 ToolExecutor 与 AuditRecorder。
pub struct DefaultToolRegistry {
    tools: RwLock<HashMap<String, Tool>>,
    executor: Arc<dyn ToolExecutor>,
    audit: Arc<dyn AuditRecorder>,
}

impl DefaultToolRegistry {
    pub fn new(executor: Arc<dyn ToolExecutor>, audit: Arc<dyn AuditRecorder>) -> Self {
        Self {
            tools: RwLock::new(HashMap::new()),
            executor,
            audit,
        }
    }
}

fn get_tool_from_map(map: &HashMap<String, Tool>, tool_name: &str) -> Option<Tool> {
    map.values()
        .find(|t| t.name == tool_name && t.is_enabled)
        .cloned()
}

#[async_trait]
impl ToolRegistry for DefaultToolRegistry {
    async fn register_tool(&self, tool: Tool, plugin_id: &str) -> Result<(), AppError> {
        let key = tool_key(plugin_id, &tool.name);
        let mut t = tool;
        t.plugin_id = plugin_id.to_string();
        t.created_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        self.tools.write().insert(key, t);
        Ok(())
    }

    async fn unregister_tool(&self, tool_name: &str, plugin_id: &str) -> Result<(), AppError> {
        let key = tool_key(plugin_id, tool_name);
        self.tools.write().remove(&key);
        Ok(())
    }

    async fn get_tool(&self, tool_name: &str) -> Result<Tool, AppError> {
        let guard = self.tools.read();
        get_tool_from_map(&guard, tool_name)
            .ok_or_else(|| AppError::Tool(format!("工具不存在或已禁用: {}", tool_name)))
    }

    async fn list_tools(&self, plugin_id: Option<&str>) -> Result<Vec<Tool>, AppError> {
        let guard = self.tools.read();
        let out: Vec<Tool> = guard
            .values()
            .filter(|t| plugin_id.is_none_or(|p| t.plugin_id.as_str() == p))
            .filter(|t| t.is_enabled)
            .cloned()
            .collect();
        Ok(out)
    }

    async fn call_tool(
        &self,
        tool_name: &str,
        params: serde_json::Value,
        plugin_id: &str,
    ) -> Result<serde_json::Value, AppError> {
        let tool = self.get_tool(tool_name).await?;
        let result = self
            .executor
            .execute(&tool, params.clone(), plugin_id)
            .await;
        match result {
            Ok(out) => {
                self.audit.record_tool_call(ToolAuditEntry {
                    tool_name: tool_name.to_string(),
                    plugin_id: tool.plugin_id.clone(),
                    caller_plugin_id: plugin_id.to_string(),
                    success: true,
                    detail: None,
                });
                Ok(wrap_tool_result(out, None))
            }
            Err(e) => {
                self.audit.record_tool_call(ToolAuditEntry {
                    tool_name: tool_name.to_string(),
                    plugin_id: tool.plugin_id.clone(),
                    caller_plugin_id: plugin_id.to_string(),
                    success: false,
                    detail: Some(e.to_string()),
                });
                Err(e)
            }
        }
    }

    fn unregister_plugin_tools(&self, plugin_id: &str) {
        let key_prefix = format!("{}::", plugin_id);
        self.tools
            .write()
            .retain(|k, _| !k.starts_with(&key_prefix));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::TracingAuditRecorder;
    use std::sync::Arc;

    struct MockToolExecutor;
    #[async_trait::async_trait]
    impl ToolExecutor for MockToolExecutor {
        async fn execute(
            &self,
            _tool: &Tool,
            params: serde_json::Value,
            _caller_plugin_id: &str,
        ) -> Result<serde_json::Value, AppError> {
            Ok(
                serde_json::json!({ "output": params.get("x").cloned().unwrap_or(serde_json::Value::Null) }),
            )
        }
    }

    fn make_tool(name: &str, plugin_id: &str) -> Tool {
        Tool {
            name: name.to_string(),
            label: name.to_string(),
            description: format!("tool {}", name),
            parameters: serde_json::json!({}),
            plugin_id: plugin_id.to_string(),
            is_enabled: true,
            created_at: 0,
        }
    }

    #[tokio::test]
    async fn register_and_get_tool() {
        let reg =
            DefaultToolRegistry::new(Arc::new(MockToolExecutor), Arc::new(TracingAuditRecorder));
        let t = make_tool("foo", "p1");
        reg.register_tool(t, "p1").await.unwrap();
        let got = reg.get_tool("foo").await.unwrap();
        assert_eq!(got.name, "foo");
        assert_eq!(got.plugin_id, "p1");
    }

    #[tokio::test]
    async fn unregister_tool() {
        let reg =
            DefaultToolRegistry::new(Arc::new(MockToolExecutor), Arc::new(TracingAuditRecorder));
        reg.register_tool(make_tool("bar", "p1"), "p1")
            .await
            .unwrap();
        reg.unregister_tool("bar", "p1").await.unwrap();
        assert!(reg.get_tool("bar").await.is_err());
    }

    #[tokio::test]
    async fn list_tools_filters_by_plugin_id() {
        let reg =
            DefaultToolRegistry::new(Arc::new(MockToolExecutor), Arc::new(TracingAuditRecorder));
        reg.register_tool(make_tool("a", "p1"), "p1").await.unwrap();
        reg.register_tool(make_tool("b", "p2"), "p2").await.unwrap();
        let all = reg.list_tools(None).await.unwrap();
        assert_eq!(all.len(), 2);
        let p1_only = reg.list_tools(Some("p1")).await.unwrap();
        assert_eq!(p1_only.len(), 1);
        assert_eq!(p1_only[0].name, "a");
    }

    #[tokio::test]
    async fn unregister_plugin_tools_removes_all_plugin_tools() {
        let reg =
            DefaultToolRegistry::new(Arc::new(MockToolExecutor), Arc::new(TracingAuditRecorder));
        reg.register_tool(make_tool("x", "p1"), "p1").await.unwrap();
        reg.register_tool(make_tool("y", "p1"), "p1").await.unwrap();
        reg.register_tool(make_tool("z", "p2"), "p2").await.unwrap();
        reg.unregister_plugin_tools("p1");
        assert!(reg.get_tool("x").await.is_err());
        assert!(reg.get_tool("y").await.is_err());
        let z = reg.get_tool("z").await.unwrap();
        assert_eq!(z.name, "z");
    }

    #[tokio::test]
    async fn call_tool_returns_content_and_details() {
        let reg =
            DefaultToolRegistry::new(Arc::new(MockToolExecutor), Arc::new(TracingAuditRecorder));
        reg.register_tool(make_tool("run", "p1"), "p1")
            .await
            .unwrap();
        let out = reg
            .call_tool("run", serde_json::json!({ "x": 42 }), "p1")
            .await
            .unwrap();
        assert!(out.get("content").is_some());
        assert!(out.get("details").is_some());
        let content = out.get("content").unwrap();
        assert_eq!(content.get("output"), Some(&serde_json::json!(42)));
    }
}
