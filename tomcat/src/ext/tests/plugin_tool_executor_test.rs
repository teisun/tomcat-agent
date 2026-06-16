use crate::core::{DefaultToolRegistry, Tool, ToolExecutor, ToolRegistry};
use crate::ext::{
    HostApiDispatcher, PluginEngine, PluginManager, PluginRuntimeManager, PluginToolExecutor,
};
use crate::infra::{DefaultEventBus, TracingAuditRecorder};
use serde_json::json;
use std::fs;
use std::sync::Arc;
use std::time::Duration;

fn make_tool(tool_name: &str, plugin_id: &str) -> Tool {
    Tool {
        name: tool_name.to_string(),
        label: tool_name.to_string(),
        description: format!("{plugin_id}::{tool_name}"),
        parameters: json!({
            "type": "object",
            "properties": {},
        }),
        plugin_id: plugin_id.to_string(),
        is_enabled: true,
        created_at: 0,
    }
}

fn plugin_tool_fixture(plugin_id: &str, script: &str) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("create plugin tempdir");
    let manifest = json!({
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

fn real_executor_harness(
    plugin_id: &str,
    script: &str,
    timeout: Duration,
) -> (
    Arc<PluginToolExecutor>,
    Arc<PluginManager>,
    Arc<HostApiDispatcher>,
    tempfile::TempDir,
) {
    let plugin_dir = plugin_tool_fixture(plugin_id, script);
    let event_bus = Arc::new(DefaultEventBus::new());
    let mut manager = Arc::new(PluginManager::new(event_bus.clone()));
    let inner = Arc::get_mut(&mut manager).expect("plugin manager should be uniquely owned");
    inner.set_plugin_engine(PluginEngine::global(None).expect("create quickjs engine"));
    inner.set_plugin_runtime_manager(Arc::new(PluginRuntimeManager::new()));
    inner.set_audit_recorder(Arc::new(TracingAuditRecorder));

    let executor = PluginToolExecutor::with_timeout(Arc::downgrade(&manager), timeout);
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
    manager.set_tool_registry(registry);
    manager.set_host_dispatcher(dispatcher.clone());
    manager
        .load_plugin(plugin_dir.path())
        .expect("load plugin tool fixture");

    (executor, manager, dispatcher, plugin_dir)
}

#[tokio::test]
async fn plugin_tool_executor_requires_session_id() {
    let executor = PluginToolExecutor::new(std::sync::Weak::new());
    let err = executor
        .execute(
            &make_tool("plugin_echo", "plugin-a"),
            json!({}),
            "__test__",
            None,
        )
        .await
        .expect_err("missing session_id should fail before any runtime access");
    assert!(
        err.to_string().contains("插件工具执行缺少 session_id"),
        "error should explain the missing session id: {err}"
    );
}

#[tokio::test]
async fn plugin_tool_executor_errors_when_dispatcher_dropped() {
    let manager = Arc::new(PluginManager::new(Arc::new(DefaultEventBus::new())));
    let executor = PluginToolExecutor::new(Arc::downgrade(&manager));

    let err = executor
        .execute(
            &make_tool("plugin_echo", "plugin-a"),
            json!({}),
            "__test__",
            Some("s1"),
        )
        .await
        .expect_err("missing dispatcher should fail fast");
    assert!(
        err.to_string().contains("host dispatcher unavailable"),
        "error should point to the dropped dispatcher: {err}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plugin_tool_executor_times_out_and_drops_waiter() {
    let (executor, manager, dispatcher, _plugin_dir) = real_executor_harness(
        "plugin-hang",
        r#"
pi.registerTool({
  name: "plugin_hang",
  description: "hang forever",
  parameters: { type: "object", properties: {} },
  execute: function () {
    return new Promise(function () {});
  }
});
"#,
        Duration::from_millis(50),
    );

    assert_eq!(dispatcher.command_waiter_count(), 0);
    let err = executor
        .execute(
            &make_tool("plugin_hang", "plugin-hang"),
            json!({}),
            "__test__",
            Some("s1"),
        )
        .await
        .expect_err("hung plugin tool should time out");
    assert!(
        err.to_string().contains("插件工具执行超时: plugin_hang"),
        "timeout error should mention the plugin tool name: {err}"
    );
    assert_eq!(
        dispatcher.command_waiter_count(),
        0,
        "timeout path should eagerly drop the dangling command waiter"
    );

    manager
        .end_session("s1")
        .await
        .expect("cleanup timed-out plugin session");
}
