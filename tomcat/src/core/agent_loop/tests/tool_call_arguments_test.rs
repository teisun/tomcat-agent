use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::core::agent_loop::tool_dispatcher::run_tool_calls;
use crate::core::agent_loop::tool_exec::{execute_tool, NORMALIZED_TOOL_CALL_ARGUMENTS};
use crate::core::agent_loop::{AgentLoop, AgentLoopConfig, ToolCallInfo};
use crate::core::llm::ChatMessage;
use crate::core::tools::contract::registry::{Tool, ToolRegistry};
use crate::core::tools::primitive::PrimitiveExecutor;
use crate::infra::error::AppError;
use crate::infra::DefaultEventBus;

use super::mocks::{MockLlmProvider, MockPrimitiveExecutor};

fn make_agent() -> AgentLoop {
    let llm = Arc::new(MockLlmProvider::new(vec![]));
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());
    let config = AgentLoopConfig {
        model: "gpt-4".to_string(),
        session_id: "s-tool-call-args".to_string(),
        ..Default::default()
    };
    AgentLoop::new(llm, primitive, event_bus, config, CancellationToken::new())
}

struct MockPluginToolRegistry;

#[async_trait::async_trait]
impl ToolRegistry for MockPluginToolRegistry {
    async fn register_tool(&self, _tool: Tool, _plugin_id: &str) -> Result<(), AppError> {
        Ok(())
    }

    async fn unregister_tool(&self, _tool_name: &str, _plugin_id: &str) -> Result<(), AppError> {
        Ok(())
    }

    async fn get_tool(&self, tool_name: &str) -> Result<Tool, AppError> {
        if tool_name == "plugin_echo" {
            Ok(Tool {
                name: "plugin_echo".to_string(),
                label: "Plugin Echo".to_string(),
                description: "plugin echo".to_string(),
                parameters: serde_json::json!({ "type": "object", "properties": {} }),
                plugin_id: "demo-plugin".to_string(),
                is_enabled: true,
                created_at: 0,
            })
        } else {
            Err(AppError::Tool("not found".to_string()))
        }
    }

    async fn list_tools(&self, _plugin_id: Option<&str>) -> Result<Vec<Tool>, AppError> {
        Ok(vec![])
    }

    async fn call_tool(
        &self,
        tool_name: &str,
        params: serde_json::Value,
        caller_plugin_id: &str,
        session_id: Option<&str>,
    ) -> Result<serde_json::Value, AppError> {
        assert_eq!(tool_name, "plugin_echo");
        assert_eq!(caller_plugin_id, "__agent__");
        assert_eq!(session_id, Some("s-tool-call-args"));
        Ok(serde_json::json!({
            "content": format!("plugin says {}", params.get("x").and_then(|v| v.as_i64()).unwrap_or_default()),
            "details": null
        }))
    }

    fn unregister_plugin_tools(&self, _plugin_id: &str) {}
}

#[tokio::test]
async fn run_tool_calls_normalizes_persisted_invalid_arguments_and_keeps_preview() {
    let mut agent = make_agent();
    let mut messages = Vec::<ChatMessage>::new();
    let tool_calls = vec![ToolCallInfo {
        id: "call_bad".into(),
        name: "read".into(),
        arguments: "{\"country\":\"".into(),
    }];

    let dispatch = run_tool_calls(
        &mut agent,
        &mut messages,
        &tool_calls,
        "",
        "",
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .expect("invalid arguments should not abort dispatcher");

    assert_eq!(dispatch.tool_results.len(), 1);
    assert_eq!(
        messages.len(),
        2,
        "assistant + tool result should be appended"
    );

    let stored_args = messages[0]
        .tool_calls
        .as_ref()
        .and_then(|calls| calls.first())
        .and_then(|call| call["function"]["arguments"].as_str())
        .expect("assistant tool_call arguments should be present");
    assert_eq!(stored_args, NORMALIZED_TOOL_CALL_ARGUMENTS);

    let tool_text = messages[1]
        .text_content()
        .expect("tool result text should be present");
    assert!(tool_text.contains("Argument parse failed"));
    assert!(tool_text.contains("persisted tool_call arguments were normalized to {}"));
    assert!(tool_text.contains(r#"Raw arguments preview (truncated): "{\"country\":\"""#));
}

#[tokio::test]
async fn run_tool_calls_keeps_valid_arguments_unchanged() {
    let mut agent = make_agent();
    let mut messages = Vec::<ChatMessage>::new();
    let raw_arguments = r#"{"path":"/tmp/abc"}"#.to_string();
    let tool_calls = vec![ToolCallInfo {
        id: "call_ok".into(),
        name: "read".into(),
        arguments: raw_arguments.clone(),
    }];

    run_tool_calls(
        &mut agent,
        &mut messages,
        &tool_calls,
        "",
        "",
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .expect("valid tool call should succeed");

    let stored_args = messages[0]
        .tool_calls
        .as_ref()
        .and_then(|calls| calls.first())
        .and_then(|call| call["function"]["arguments"].as_str())
        .expect("assistant tool_call arguments should be present");
    assert_eq!(stored_args, raw_arguments);
}

#[tokio::test]
async fn run_tool_calls_normalizes_empty_arguments_to_empty_object() {
    let mut agent = make_agent();
    let mut messages = Vec::<ChatMessage>::new();
    let tool_calls = vec![ToolCallInfo {
        id: "call_empty".into(),
        name: "read".into(),
        arguments: String::new(),
    }];

    run_tool_calls(
        &mut agent,
        &mut messages,
        &tool_calls,
        "",
        "",
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .expect("empty arguments should not abort dispatcher");

    let stored_args = messages[0]
        .tool_calls
        .as_ref()
        .and_then(|calls| calls.first())
        .and_then(|call| call["function"]["arguments"].as_str())
        .expect("assistant tool_call arguments should be present");
    assert_eq!(stored_args, NORMALIZED_TOOL_CALL_ARGUMENTS);

    let tool_text = messages[1]
        .text_content()
        .expect("tool result text should be present");
    assert!(tool_text.contains(r#"Raw arguments preview (truncated): """#));
}

#[tokio::test]
async fn execute_tool_invalid_arguments_preview_is_truncated() {
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(MockPrimitiveExecutor);
    let raw_arguments = format!("{{\"query\":\"{}TAIL_MARKER", "a".repeat(160));
    let tc = ToolCallInfo {
        id: "call_long".into(),
        name: "read".into(),
        arguments: raw_arguments,
    };

    let (msg, is_error, _) = execute_tool(&primitive, &None, &None, None, &tc).await;

    assert!(is_error, "invalid arguments must report is_error=true");
    assert!(msg.contains("Argument parse failed"));
    assert!(msg.contains("Raw arguments preview (truncated):"));
    assert!(
        msg.contains("..."),
        "long preview should be visibly truncated: {msg}"
    );
    assert!(
        !msg.contains("TAIL_MARKER"),
        "truncated preview must not leak tail marker: {msg}"
    );
}

#[tokio::test]
async fn execute_tool_invalid_arguments_preview_escapes_control_characters() {
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(MockPrimitiveExecutor);
    let tc = ToolCallInfo {
        id: "call_ctrl".into(),
        name: "read".into(),
        arguments: "{\"query\":\"line\n\t".into(),
    };

    let (msg, is_error, _) = execute_tool(&primitive, &None, &None, None, &tc).await;

    assert!(is_error, "invalid arguments must report is_error=true");
    assert!(msg.contains("Argument parse failed"));
    assert!(msg.contains(r#"Raw arguments preview (truncated): "{\"query\":\"line\n\t""#));
}

#[tokio::test]
async fn run_tool_calls_dispatches_registered_plugin_tool_through_registry() {
    let mut agent = make_agent().with_tool_registry(Arc::new(MockPluginToolRegistry));
    let mut messages = Vec::<ChatMessage>::new();
    let tool_calls = vec![ToolCallInfo {
        id: "call_plugin".into(),
        name: "plugin_echo".into(),
        arguments: r#"{"x":7}"#.into(),
    }];

    let dispatch = run_tool_calls(
        &mut agent,
        &mut messages,
        &tool_calls,
        "",
        "",
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .expect("plugin tool should execute through ToolRegistry");

    assert_eq!(dispatch.tool_results.len(), 1);
    let tool_text = messages[1]
        .text_content()
        .expect("tool result text should be present");
    assert_eq!(tool_text, "plugin says 7");
}
