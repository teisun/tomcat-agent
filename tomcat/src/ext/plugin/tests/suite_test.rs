use super::super::manager::PluginManager;
use super::super::types::{
    parse_manifest, PluginActivation, PluginInstance, PluginManifest, PluginStatus,
};
use crate::core::tools::contract::registry::{
    DefaultToolRegistry, Tool, ToolExecutor, ToolRegistry,
};
use crate::ext::{
    HostApiDispatcher, PluginEngine, PluginRuntimeKey, PluginRuntimeManager,
    SharedPluginRuntimeManager,
};
use crate::infra::error::AppError;
use crate::infra::{DefaultEventBus, TracingAuditRecorder};
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

struct NoopToolExecutor;

#[async_trait]
impl ToolExecutor for NoopToolExecutor {
    async fn execute(
        &self,
        _tool: &Tool,
        _params: serde_json::Value,
        _caller_plugin_id: &str,
        _session_id: Option<&str>,
    ) -> Result<serde_json::Value, AppError> {
        Ok(serde_json::Value::Null)
    }
}

fn plugin_fixture(
    plugin_id: &str,
    tools: serde_json::Value,
    activation: PluginActivation,
    script: &str,
) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("create temp plugin");
    let activation = match activation {
        PluginActivation::Lazy => "lazy",
        PluginActivation::Session => "session",
    };
    let manifest = serde_json::json!({
        "id": plugin_id,
        "name": plugin_id,
        "version": "0.1.0",
        "description": format!("fixture {plugin_id}"),
        "author": "tests",
        "main": "main.js",
        "requiredPermissions": [],
        "requiredApiVersion": "1.0",
        "tags": [],
        "tools": tools,
        "activation": activation
    });
    std::fs::write(
        tmp.path().join("plugin.json"),
        serde_json::to_string_pretty(&manifest).expect("serialize manifest"),
    )
    .expect("write plugin manifest");
    std::fs::write(tmp.path().join("main.js"), script).expect("write plugin main");
    tmp
}

fn manager_with_runtime() -> (
    PluginManager,
    Arc<HostApiDispatcher>,
    Arc<DefaultToolRegistry>,
    SharedPluginRuntimeManager,
) {
    manager_with_runtime_and_idle_ttl(Duration::from_millis(
        crate::ext::DEFAULT_PLUGIN_IDLE_TTL_MS,
    ))
}

fn manager_with_runtime_and_idle_ttl(
    idle_ttl: Duration,
) -> (
    PluginManager,
    Arc<HostApiDispatcher>,
    Arc<DefaultToolRegistry>,
    SharedPluginRuntimeManager,
) {
    let bus = Arc::new(DefaultEventBus::new());
    let tool_registry = Arc::new(DefaultToolRegistry::new(
        Arc::new(NoopToolExecutor),
        Arc::new(TracingAuditRecorder),
    ));
    let dispatcher =
        Arc::new(HostApiDispatcher::new(bus.clone()).with_tools(tool_registry.clone()));
    let runtime_manager: SharedPluginRuntimeManager =
        Arc::new(PluginRuntimeManager::with_idle_ttl(idle_ttl));
    let mut manager = PluginManager::new(bus);
    manager.set_plugin_engine(PluginEngine::global(None).expect("create quickjs engine"));
    manager.set_plugin_runtime_manager(runtime_manager.clone());
    manager.set_tool_registry(tool_registry.clone());
    manager.set_host_dispatcher(dispatcher.clone());
    manager.set_audit_recorder(Arc::new(TracingAuditRecorder));
    (manager, dispatcher, tool_registry, runtime_manager)
}

#[test]
fn parse_manifest_valid() {
    let json = r#"{
        "id": "test-plugin",
        "name": "Test",
        "version": "0.1.0",
        "description": "d",
        "author": "a",
        "main": "index.js",
        "requiredPermissions": ["read"],
        "requiredApiVersion": "1.0",
        "tags": []
    }"#;
    let m = parse_manifest(json).unwrap();
    assert_eq!(m.id, "test-plugin");
    assert_eq!(m.required_api_version, "1.0");
}

#[test]
fn parse_manifest_missing_id_fails() {
    let json = r#"{
        "id": "",
        "name": "x",
        "version": "0.1.0",
        "description": "d",
        "author": "a",
        "main": "index.js",
        "requiredPermissions": [],
        "requiredApiVersion": "1.0",
        "tags": []
    }"#;
    let r = parse_manifest(json);
    assert!(r.is_err());
    assert!(r.unwrap_err().to_string().contains("id"));
}

#[test]
fn manager_register_and_unload() {
    let bus = Arc::new(DefaultEventBus::new());
    let manager = PluginManager::new(bus);
    let inst = PluginInstance {
        id: "p1".to_string(),
        manifest: PluginManifest {
            id: "p1".to_string(),
            name: "P1".to_string(),
            version: "0.1.0".to_string(),
            description: String::new(),
            author: String::new(),
            main: "index.js".to_string(),
            required_permissions: vec![],
            required_api_version: "1.0".to_string(),
            tags: vec![],
            tools: vec![],
            events: vec![],
            activation: PluginActivation::Lazy,
        },
        plugin_vm_instance: None,
        status: PluginStatus::Loaded,
        registered_tools: vec![],
        registered_commands: vec![],
        event_listener_ids: vec![],
        config: serde_json::Value::Null,
        created_at: 0,
        loaded_at: 0,
        plugin_root: PathBuf::from("."),
    };
    manager.register_plugin(inst).unwrap();
    assert_eq!(manager.list_loaded(), vec!["p1"]);
    manager.unload_plugin("p1").unwrap();
    assert!(manager.list_loaded().is_empty());
}

#[test]
fn get_plugin_returns_some_after_register_none_for_unknown() {
    let bus = Arc::new(DefaultEventBus::new());
    let manager = PluginManager::new(bus);
    let inst = PluginInstance {
        id: "p-get".to_string(),
        manifest: PluginManifest {
            id: "p-get".to_string(),
            name: "PGet".to_string(),
            version: "0.1.0".to_string(),
            description: String::new(),
            author: String::new(),
            main: "index.js".to_string(),
            required_permissions: vec![],
            required_api_version: "1.0".to_string(),
            tags: vec![],
            tools: vec![],
            events: vec![],
            activation: PluginActivation::Lazy,
        },
        plugin_vm_instance: None,
        status: PluginStatus::Loaded,
        registered_tools: vec![],
        registered_commands: vec![],
        event_listener_ids: vec![],
        config: serde_json::Value::Null,
        created_at: 0,
        loaded_at: 0,
        plugin_root: PathBuf::from("."),
    };
    assert!(manager.get_plugin("p-get").is_none());
    manager.register_plugin(inst).unwrap();
    let info = manager.get_plugin("p-get").unwrap();
    assert_eq!(info.id, "p-get");
    assert_eq!(info.manifest.name, "PGet");
    assert!(manager.get_plugin("unknown").is_none());
}

#[test]
fn register_plugin_duplicate_returns_err() {
    let bus = Arc::new(DefaultEventBus::new());
    let manager = PluginManager::new(bus);
    let inst = PluginInstance {
        id: "dup".to_string(),
        manifest: PluginManifest {
            id: "dup".to_string(),
            name: "Dup".to_string(),
            version: "0.1.0".to_string(),
            description: String::new(),
            author: String::new(),
            main: "index.js".to_string(),
            required_permissions: vec![],
            required_api_version: "1.0".to_string(),
            tags: vec![],
            tools: vec![],
            events: vec![],
            activation: PluginActivation::Lazy,
        },
        plugin_vm_instance: None,
        status: PluginStatus::Loaded,
        registered_tools: vec![],
        registered_commands: vec![],
        event_listener_ids: vec![],
        config: serde_json::Value::Null,
        created_at: 0,
        loaded_at: 0,
        plugin_root: PathBuf::from("."),
    };
    manager.register_plugin(inst).unwrap();
    let inst2 = PluginInstance {
        id: "dup".to_string(),
        manifest: PluginManifest {
            id: "dup".to_string(),
            name: "Dup".to_string(),
            version: "0.1.0".to_string(),
            description: String::new(),
            author: String::new(),
            main: "index.js".to_string(),
            required_permissions: vec![],
            required_api_version: "1.0".to_string(),
            tags: vec![],
            tools: vec![],
            events: vec![],
            activation: PluginActivation::Lazy,
        },
        plugin_vm_instance: None,
        status: PluginStatus::Loaded,
        registered_tools: vec![],
        registered_commands: vec![],
        event_listener_ids: vec![],
        config: serde_json::Value::Null,
        created_at: 0,
        loaded_at: 0,
        plugin_root: PathBuf::from("."),
    };
    let r = manager.register_plugin(inst2);
    assert!(r.is_err());
    assert!(r.unwrap_err().to_string().contains("already loaded"));
}

#[test]
fn enable_disable_changes_status() {
    let bus = Arc::new(DefaultEventBus::new());
    let manager = PluginManager::new(bus);
    let inst = PluginInstance {
        id: "p2".to_string(),
        manifest: PluginManifest {
            id: "p2".to_string(),
            name: "P2".to_string(),
            version: "0.1.0".to_string(),
            description: String::new(),
            author: String::new(),
            main: "index.js".to_string(),
            required_permissions: vec![],
            required_api_version: "1.0".to_string(),
            tags: vec![],
            tools: vec![],
            events: vec![],
            activation: PluginActivation::Lazy,
        },
        plugin_vm_instance: None,
        status: PluginStatus::Loaded,
        registered_tools: vec![],
        registered_commands: vec![],
        event_listener_ids: vec![],
        config: serde_json::Value::Null,
        created_at: 0,
        loaded_at: 0,
        plugin_root: PathBuf::from("."),
    };
    manager.register_plugin(inst).unwrap();
    manager.enable_plugin("p2").unwrap();
    assert_eq!(
        manager.get_plugin("p2").map(|i| i.status).unwrap(),
        PluginStatus::Enabled
    );
    manager.disable_plugin("p2").unwrap();
    assert_eq!(
        manager.get_plugin("p2").map(|i| i.status).unwrap(),
        PluginStatus::Disabled
    );
}

#[test]
fn unload_nonexistent_returns_err() {
    let bus = Arc::new(DefaultEventBus::new());
    let manager = PluginManager::new(bus);
    let r = manager.unload_plugin("nonexistent");
    assert!(r.is_err());
}

#[test]
fn enable_plugin_not_found_returns_err() {
    let bus = Arc::new(DefaultEventBus::new());
    let manager = PluginManager::new(bus);
    let r = manager.enable_plugin("nonexistent");
    assert!(r.is_err());
    assert!(r.unwrap_err().to_string().contains("not found"));
}

#[test]
fn disable_plugin_not_found_returns_err() {
    let bus = Arc::new(DefaultEventBus::new());
    let manager = PluginManager::new(bus);
    let r = manager.disable_plugin("nonexistent");
    assert!(r.is_err());
    assert!(r.unwrap_err().to_string().contains("not found"));
}

#[test]
fn load_plugin_without_wasm_engine_returns_err() {
    let bus = Arc::new(DefaultEventBus::new());
    let manager = PluginManager::new(bus);
    let tmp = tempfile::tempdir().unwrap();
    let plugin_json = r#"{"id":"x","name":"X","version":"0.1.0","description":"","author":"","main":"index.js","requiredPermissions":[],"requiredApiVersion":"1.0","tags":[]}"#;
    std::fs::write(tmp.path().join("plugin.json"), plugin_json).unwrap();
    std::fs::write(tmp.path().join("index.js"), "// empty").unwrap();
    let r = manager.load_plugin(tmp.path());
    assert!(r.is_err());
    assert!(r.unwrap_err().to_string().contains("set_plugin_engine"));
}

#[test]
fn load_plugin_nonexistent_path_returns_err() {
    let bus = Arc::new(DefaultEventBus::new());
    let manager = PluginManager::new(bus);
    let r = manager.load_plugin(Path::new("/nonexistent/dir/12345"));
    assert!(r.is_err());
    assert!(matches!(r.unwrap_err(), AppError::Plugin(_)));
}

#[test]
fn load_plugin_dir_without_manifest_returns_err() {
    let bus = Arc::new(DefaultEventBus::new());
    let mut manager = PluginManager::new(bus);
    let _ = crate::ext::PluginEngine::global(None).map(|e| manager.set_plugin_engine(e));
    let tmp = tempfile::tempdir().unwrap();
    let r = manager.load_plugin(tmp.path());
    assert!(r.is_err());
    let err = r.unwrap_err();
    assert!(
        err.to_string().contains("plugin.json")
            || err.to_string().contains("pi-plugin")
            || err.to_string().contains("未找到")
    );
}

#[test]
fn load_plugin_user_deny_returns_permission_err() {
    let bus = Arc::new(DefaultEventBus::new());
    let mut manager = PluginManager::new(bus);
    let engine = match crate::ext::PluginEngine::global(None) {
        Ok(e) => e,
        Err(_) => return,
    };
    manager.set_plugin_engine(engine);
    manager.set_confirm_permissions(Arc::new(|_| Ok(false)));

    let tmp = tempfile::tempdir().unwrap();
    let plugin_json = r#"{
        "id": "deny-test",
        "name": "DenyTest",
        "version": "0.1.0",
        "description": "",
        "author": "",
        "main": "index.js",
        "requiredPermissions": [],
        "requiredApiVersion": "1.0",
        "tags": []
    }"#;
    std::fs::write(tmp.path().join("plugin.json"), plugin_json).unwrap();
    std::fs::write(tmp.path().join("index.js"), "// empty").unwrap();

    let r = manager.load_plugin(tmp.path());
    assert!(r.is_err());
    let err = r.unwrap_err();
    assert!(matches!(err, AppError::Permission(_)));
    assert!(err.to_string().contains("拒绝"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn load_enable_unload_cleans_registered_side_effects() {
    let fixture = plugin_fixture(
        "cleanup-plugin",
        serde_json::json!([]),
        PluginActivation::Lazy,
        r#"
pi.registerTool({
  name: "cleanup_echo",
  description: "cleanup tool",
  parameters: { type: "object", properties: { text: { type: "string" } }, required: ["text"] },
  execute: function (_callId, params) {
    return { echo: params.text };
  }
});
pi.registerCommand("cleanup_cmd", {
  description: "cleanup command",
  handler: function () { return "ok"; }
});
pi.on("cleanup_evt", function () {});
"#,
    );
    let (manager, dispatcher, tool_registry, runtime_manager) = manager_with_runtime();

    manager
        .load_plugin(fixture.path())
        .expect("load cleanup plugin");
    manager
        .disable_plugin("cleanup-plugin")
        .expect("disable plugin");
    manager
        .enable_plugin("cleanup-plugin")
        .expect("re-enable plugin");

    let tools_before = tool_registry
        .list_tools(None)
        .await
        .expect("list tools before unload");
    assert!(
        tools_before.iter().any(|tool| tool.name == "cleanup_echo"),
        "load_plugin should surface plugin tools into the shared registry"
    );
    assert_eq!(
        dispatcher.registered_plugin_tools("cleanup-plugin"),
        vec!["cleanup_echo".to_string()]
    );
    assert_eq!(
        dispatcher
            .registered_plugin_commands("cleanup-plugin")
            .into_iter()
            .map(|(name, _)| name)
            .collect::<Vec<_>>(),
        vec!["cleanup_cmd".to_string()]
    );
    assert!(
        !dispatcher
            .registered_plugin_listener_ids("cleanup-plugin")
            .is_empty(),
        "plugin should register at least one event listener side effect"
    );

    let vm_key = PluginRuntimeKey::new("suite-session", "cleanup-plugin");
    manager
        .start_session_vm("suite-session", "cleanup-plugin")
        .await
        .expect("start session vm for cleanup test");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        runtime_manager.contains(&vm_key),
        "session runtime should be present before unload"
    );

    manager
        .unload_plugin("cleanup-plugin")
        .expect("unload cleanup plugin");

    let tools_after = tool_registry
        .list_tools(None)
        .await
        .expect("list tools after unload");
    assert!(
        tools_after.iter().all(|tool| tool.name != "cleanup_echo"),
        "unload_plugin should remove plugin tools from shared registry"
    );
    assert!(
        dispatcher
            .registered_plugin_tools("cleanup-plugin")
            .is_empty(),
        "dispatcher plugin tool metadata should be cleaned on unload"
    );
    assert!(
        dispatcher
            .registered_plugin_commands("cleanup-plugin")
            .is_empty(),
        "dispatcher plugin command metadata should be cleaned on unload"
    );
    assert!(
        dispatcher
            .registered_plugin_listener_ids("cleanup-plugin")
            .is_empty(),
        "dispatcher listener bookkeeping should be cleaned on unload"
    );
    assert!(
        !runtime_manager.contains(&vm_key),
        "unload_plugin should evict all session runtimes for the plugin"
    );
    assert!(
        dispatcher.get_event_sender(&vm_key.to_string()).is_none(),
        "unload_plugin should drop plugin event channels"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn registered_tool_surfaces_to_tool_registry() {
    let fixture = plugin_fixture(
        "surface-plugin",
        serde_json::json!([
            {
                "name": "surface_echo",
                "description": "surface tool",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "text": { "type": "string" }
                    },
                    "required": ["text"]
                }
            }
        ]),
        PluginActivation::Lazy,
        r#"
pi.registerTool({
  name: "surface_echo",
  description: "surface tool",
  parameters: { type: "object", properties: { text: { type: "string" } }, required: ["text"] },
  execute: function (_callId, params) {
    return { plugin: "surface-plugin", echo: params.text };
  }
});
"#,
    );
    let (manager, _dispatcher, tool_registry, _runtime_manager) = manager_with_runtime();

    manager
        .load_plugin(fixture.path())
        .expect("load surface plugin");
    let tool = tool_registry
        .get_tool("surface_echo")
        .await
        .expect("surface tool should be discoverable");
    assert_eq!(tool.plugin_id, "surface-plugin");
    assert_eq!(tool.name, "surface_echo");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_session_vm_opportunistically_reaps_expired_runtime() {
    let stale_fixture = plugin_fixture(
        "stale-plugin",
        serde_json::json!([]),
        PluginActivation::Lazy,
        "pi.on('noop', function () {});",
    );
    let fresh_fixture = plugin_fixture(
        "fresh-plugin",
        serde_json::json!([]),
        PluginActivation::Lazy,
        "pi.on('noop', function () {});",
    );
    let (manager, _dispatcher, _tool_registry, runtime_manager) =
        manager_with_runtime_and_idle_ttl(Duration::from_millis(5));

    manager
        .load_plugin(stale_fixture.path())
        .expect("load stale plugin");
    manager
        .load_plugin(fresh_fixture.path())
        .expect("load fresh plugin");

    let stale_key = PluginRuntimeKey::new("stale-session", "stale-plugin");
    manager
        .start_session_vm("stale-session", "stale-plugin")
        .await
        .expect("start stale session vm");
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        runtime_manager.contains(&stale_key),
        "stale runtime should exist before opportunistic reap"
    );

    manager
        .start_session_vm("fresh-session", "fresh-plugin")
        .await
        .expect("start fresh session vm");

    assert!(
        !runtime_manager.contains(&stale_key),
        "starting another plugin session should opportunistically reap expired runtimes"
    );

    manager
        .end_session("fresh-session")
        .await
        .expect("cleanup fresh session");
    manager
        .end_session("stale-session")
        .await
        .expect("cleanup stale session");
}
