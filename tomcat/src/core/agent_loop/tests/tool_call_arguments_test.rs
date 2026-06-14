use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;

use crate::core::agent_loop::tool_dispatcher::run_tool_calls;
use crate::core::agent_loop::tool_exec::{execute_tool, NORMALIZED_TOOL_CALL_ARGUMENTS};
use crate::core::agent_loop::{AgentLoop, AgentLoopConfig, ToolCallInfo};
use crate::core::llm::ChatMessage;
use crate::core::tools::contract::registry::{DefaultToolRegistry, Tool, ToolRegistry};
use crate::core::tools::primitive::PrimitiveExecutor;
use crate::ext::{
    HostApiDispatcher, PluginEngine, PluginManager, PluginRuntimeManager, PluginToolExecutor,
};
use crate::infra::error::AppError;
use crate::infra::{wire, DefaultEventBus, EventBus, EventContext, TracingAuditRecorder};

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

struct CountingPrimitiveExecutor {
    calls: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl PrimitiveExecutor for CountingPrimitiveExecutor {
    async fn read_file(&self, _path: &str, _plugin_id: &str) -> Result<String, AppError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(AppError::Tool(
            "primitive read_file should not be used".to_string(),
        ))
    }

    async fn list_dir(
        &self,
        _path: &str,
        _plugin_id: &str,
    ) -> Result<Vec<crate::core::DirEntry>, AppError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(AppError::Tool(
            "primitive list_dir should not be used".to_string(),
        ))
    }

    async fn write_file(
        &self,
        _path: &str,
        _content: &str,
        _overwrite: bool,
        _plugin_id: &str,
    ) -> Result<crate::core::WriteFileResult, AppError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(AppError::Tool(
            "primitive write_file should not be used".to_string(),
        ))
    }

    async fn edit_file(
        &self,
        _path: &str,
        _edits: Vec<crate::core::EditOperation>,
        _plugin_id: &str,
    ) -> Result<crate::core::EditFileResult, AppError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(AppError::Tool(
            "primitive edit_file should not be used".to_string(),
        ))
    }

    async fn execute_bash(
        &self,
        _command: &str,
        _cwd: Option<&str>,
        _plugin_id: &str,
        _argv: Option<&[String]>,
        _timeout_ms: Option<u64>,
    ) -> Result<crate::core::BashResult, AppError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(AppError::Tool(
            "primitive execute_bash should not be used".to_string(),
        ))
    }

    async fn require_user_confirmation(
        &self,
        _operation: crate::core::PrimitiveOperation,
        _preview: &str,
        _plugin_id: &str,
    ) -> Result<bool, AppError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(true)
    }
}

struct RealPluginHarness {
    registry: Arc<dyn ToolRegistry>,
    manager: Arc<PluginManager>,
    _dispatcher: Arc<HostApiDispatcher>,
    _plugin_dir: tempfile::TempDir,
}

fn plugin_tool_fixture(plugin_id: &str, script: &str) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("create plugin tempdir");
    let manifest = serde_json::json!({
        "id": plugin_id,
        "name": plugin_id,
        "version": "0.1.0",
        "description": "plugin tool test fixture",
        "author": "tests",
        "main": "main.js",
        "requiredPermissions": [],
        "requiredApiVersion": "1.0",
        "tags": []
    });
    fs::write(
        tmp.path().join("plugin.json"),
        serde_json::to_string_pretty(&manifest).expect("serialize manifest"),
    )
    .expect("write plugin.json");
    fs::write(tmp.path().join("main.js"), script).expect("write main.js");
    tmp
}

async fn real_plugin_harness(
    event_bus: Arc<DefaultEventBus>,
    plugin_id: &str,
    script: &str,
) -> RealPluginHarness {
    let plugin_dir = plugin_tool_fixture(plugin_id, script);
    let mut manager = Arc::new(PluginManager::new(event_bus.clone()));
    let inner = Arc::get_mut(&mut manager).expect("plugin manager should be uniquely owned");
    inner.set_plugin_engine(PluginEngine::global(None).expect("create quickjs engine"));
    inner.set_plugin_runtime_manager(Arc::new(PluginRuntimeManager::new()));
    inner.set_audit_recorder(Arc::new(TracingAuditRecorder));

    let executor = PluginToolExecutor::new(Arc::downgrade(&manager));
    let registry_impl = Arc::new(DefaultToolRegistry::new(
        executor.clone(),
        Arc::new(TracingAuditRecorder),
    ));
    let registry: Arc<dyn ToolRegistry> = registry_impl.clone();
    let dispatcher = Arc::new(
        HostApiDispatcher::new(event_bus.clone())
            .with_tokio_handle(tokio::runtime::Handle::current())
            .with_tools(registry.clone()),
    );
    executor.attach_dispatcher(Arc::downgrade(&dispatcher));
    manager.set_tool_registry(registry.clone());
    manager.set_host_dispatcher(dispatcher.clone());
    manager
        .load_plugin(plugin_dir.path())
        .expect("load real plugin tool fixture");

    RealPluginHarness {
        registry,
        manager,
        _dispatcher: dispatcher,
        _plugin_dir: plugin_dir,
    }
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

#[tokio::test]
async fn plugin_tool_events_stay_balanced_and_skip_primitive_bypass() {
    let event_bus = Arc::new(DefaultEventBus::new());
    let harness = real_plugin_harness(
        Arc::clone(&event_bus),
        "plugin-echo",
        r#"
pi.registerTool({
  name: "plugin_echo",
  description: "plugin echo",
  parameters: { type: "object", properties: { x: { type: "integer" } }, required: ["x"] },
  execute: function (_callId, params) {
    return "plugin says " + String(params.x);
  }
});
"#,
    )
    .await;

    let observed: Arc<Mutex<Vec<(String, serde_json::Value)>>> = Arc::new(Mutex::new(Vec::new()));
    for name in [
        wire::WIRE_TOOL_EXECUTION_START,
        wire::WIRE_TOOL_CALL,
        wire::WIRE_TOOL_RESULT,
        wire::WIRE_TOOL_EXECUTION_END,
    ] {
        let sink = Arc::clone(&observed);
        event_bus.on(
            name,
            Box::new(move |ctx: EventContext| {
                sink.lock()
                    .unwrap()
                    .push((ctx.event_name.clone(), ctx.payload.clone()));
                Ok(())
            }),
        );
    }

    let primitive_calls = Arc::new(AtomicUsize::new(0));
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(CountingPrimitiveExecutor {
        calls: Arc::clone(&primitive_calls),
    });
    let llm = Arc::new(MockLlmProvider::new(vec![]));
    let config = AgentLoopConfig {
        model: "gpt-4".to_string(),
        session_id: "s-tool-call-args".to_string(),
        ..Default::default()
    };
    let mut agent = AgentLoop::new(
        llm,
        primitive,
        event_bus.clone(),
        config,
        CancellationToken::new(),
    )
    .with_tool_registry(harness.registry.clone());

    let mut messages = Vec::<ChatMessage>::new();
    let tool_calls = vec![ToolCallInfo {
        id: "call_plugin_real".into(),
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
    .expect("plugin tool should complete via PluginToolExecutor");

    assert_eq!(
        primitive_calls.load(Ordering::SeqCst),
        0,
        "plugin tool execution must not bypass into primitive branches"
    );
    assert_eq!(dispatch.tool_results.len(), 1);
    assert_eq!(
        messages[1].text_content(),
        Some("plugin says 7"),
        "tool result should come from plugin tool executor path"
    );

    let events = observed.lock().unwrap().clone();
    let names: Vec<&str> = events.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            wire::WIRE_TOOL_EXECUTION_START,
            wire::WIRE_TOOL_CALL,
            wire::WIRE_TOOL_RESULT,
            wire::WIRE_TOOL_EXECUTION_END,
        ]
    );
    assert_eq!(
        events[2].1.get("isError").and_then(|value| value.as_bool()),
        Some(false)
    );
    assert_eq!(
        events[3].1.get("isError").and_then(|value| value.as_bool()),
        Some(false)
    );

    harness
        .manager
        .end_session("s-tool-call-args")
        .await
        .expect("cleanup plugin session runtime");
}

#[tokio::test]
async fn plugin_tool_interrupt_emits_tool_result_before_end() {
    let event_bus = Arc::new(DefaultEventBus::new());
    let harness = real_plugin_harness(
        Arc::clone(&event_bus),
        "plugin-slow",
        r#"
pi.registerTool({
  name: "plugin_slow",
  description: "plugin slow",
  parameters: { type: "object", properties: {}, additionalProperties: false },
  execute: function () {
    return new Promise(function (resolve) {
      setTimeout(function () {
        resolve("too late");
      }, 400);
    });
  }
});
"#,
    )
    .await;

    let observed: Arc<Mutex<Vec<(String, serde_json::Value)>>> = Arc::new(Mutex::new(Vec::new()));
    for name in [
        wire::WIRE_TOOL_EXECUTION_START,
        wire::WIRE_TOOL_CALL,
        wire::WIRE_TOOL_RESULT,
        wire::WIRE_TOOL_EXECUTION_END,
    ] {
        let sink = Arc::clone(&observed);
        event_bus.on(
            name,
            Box::new(move |ctx: EventContext| {
                sink.lock()
                    .unwrap()
                    .push((ctx.event_name.clone(), ctx.payload.clone()));
                Ok(())
            }),
        );
    }

    let primitive_calls = Arc::new(AtomicUsize::new(0));
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(CountingPrimitiveExecutor {
        calls: Arc::clone(&primitive_calls),
    });
    let llm = Arc::new(MockLlmProvider::new(vec![]));
    let cancel = CancellationToken::new();
    let config = AgentLoopConfig {
        model: "gpt-4".to_string(),
        session_id: "s-tool-call-args".to_string(),
        ..Default::default()
    };
    let mut agent = AgentLoop::new(llm, primitive, event_bus.clone(), config, cancel.clone())
        .with_tool_registry(harness.registry.clone());

    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        cancel.cancel();
    });

    let mut messages = Vec::<ChatMessage>::new();
    let tool_calls = vec![ToolCallInfo {
        id: "call_plugin_slow".into(),
        name: "plugin_slow".into(),
        arguments: "{}".into(),
    }];

    let result = run_tool_calls(
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
    .await;
    assert!(
        result.is_err(),
        "cancelled plugin tool should abort the tool loop"
    );
    assert_eq!(
        primitive_calls.load(Ordering::SeqCst),
        0,
        "cancelled plugin tool must still stay on plugin executor path"
    );

    let events = observed.lock().unwrap().clone();
    let names: Vec<&str> = events.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            wire::WIRE_TOOL_EXECUTION_START,
            wire::WIRE_TOOL_CALL,
            wire::WIRE_TOOL_RESULT,
            wire::WIRE_TOOL_EXECUTION_END,
        ]
    );
    assert_eq!(
        events[2].1.get("isError").and_then(|value| value.as_bool()),
        Some(true),
        "interrupted plugin tool should publish tool_result as error"
    );
    assert_eq!(
        events[2]
            .1
            .get("content")
            .and_then(|value| value.as_array())
            .and_then(|blocks| blocks.first())
            .and_then(|block| block.get("text"))
            .and_then(|value| value.as_str()),
        Some("[interrupted]")
    );
    assert_eq!(
        events[3].1.get("isError").and_then(|value| value.as_bool()),
        Some(true),
        "tool_execution_end should stay paired with interrupted result"
    );

    harness
        .manager
        .end_session("s-tool-call-args")
        .await
        .expect("cleanup slow plugin session runtime");
}
