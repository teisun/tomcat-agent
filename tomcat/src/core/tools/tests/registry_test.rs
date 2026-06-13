use crate::core::tools::contract::registry::{
    DefaultToolRegistry, Tool, ToolExecutor, ToolRegistry,
};
use crate::infra::error::AppError;
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
        _session_id: Option<&str>,
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
    let reg = DefaultToolRegistry::new(Arc::new(MockToolExecutor), Arc::new(TracingAuditRecorder));
    let t = make_tool("foo", "p1");
    reg.register_tool(t, "p1").await.unwrap();
    let got = reg.get_tool("foo").await.unwrap();
    assert_eq!(got.name, "foo");
    assert_eq!(got.plugin_id, "p1");
}

#[tokio::test]
async fn unregister_tool() {
    let reg = DefaultToolRegistry::new(Arc::new(MockToolExecutor), Arc::new(TracingAuditRecorder));
    reg.register_tool(make_tool("bar", "p1"), "p1")
        .await
        .unwrap();
    reg.unregister_tool("bar", "p1").await.unwrap();
    assert!(reg.get_tool("bar").await.is_err());
}

#[tokio::test]
async fn list_tools_filters_by_plugin_id() {
    let reg = DefaultToolRegistry::new(Arc::new(MockToolExecutor), Arc::new(TracingAuditRecorder));
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
    let reg = DefaultToolRegistry::new(Arc::new(MockToolExecutor), Arc::new(TracingAuditRecorder));
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
    let reg = DefaultToolRegistry::new(Arc::new(MockToolExecutor), Arc::new(TracingAuditRecorder));
    reg.register_tool(make_tool("run", "p1"), "p1")
        .await
        .unwrap();
    let out = reg
        .call_tool("run", serde_json::json!({ "x": 42 }), "p1", Some("s1"))
        .await
        .unwrap();
    assert!(out.get("content").is_some());
    assert!(out.get("details").is_some());
    let content = out.get("content").unwrap();
    assert_eq!(content.get("output"), Some(&serde_json::json!(42)));
}

#[tokio::test]
async fn register_tool_rejects_scope_name_conflict() {
    let reg = DefaultToolRegistry::new(Arc::new(MockToolExecutor), Arc::new(TracingAuditRecorder));
    reg.register_tool(make_tool("shared", "p1"), "p1")
        .await
        .unwrap();
    let err = reg
        .register_tool(make_tool("shared", "p2"), "p2")
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("scope 内工具名冲突"),
        "unexpected conflict error: {err}"
    );
}
