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

struct BlockingRead {
    entered: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
}
#[async_trait::async_trait]
impl crate::core::PrimitiveExecutor for BlockingRead {
    async fn read_file(&self, _: &str, _: &str) -> Result<String, crate::AppError> {
        if let Some(tx) = self.entered.lock().unwrap().take() {
            let _ = tx.send(());
        }
        self.release
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        Ok("released".into())
    }
    async fn list_dir(
        &self,
        _: &str,
        _: &str,
    ) -> Result<Vec<crate::core::DirEntry>, crate::AppError> {
        Err(crate::AppError::Tool("unused list".into()))
    }
    async fn write_file(
        &self,
        _: &str,
        _: &str,
        _: bool,
        _: &str,
    ) -> Result<crate::core::WriteFileResult, crate::AppError> {
        Err(crate::AppError::Tool("unused write".into()))
    }
    async fn edit_file(
        &self,
        _: &str,
        _: Vec<crate::core::EditOperation>,
        _: &str,
    ) -> Result<crate::core::EditFileResult, crate::AppError> {
        Err(crate::AppError::Tool("unused edit".into()))
    }
    async fn execute_bash(
        &self,
        _: &str,
        _: Option<&str>,
        _: &str,
        _: Option<u64>,
    ) -> Result<crate::core::BashResult, crate::AppError> {
        Err(crate::AppError::Tool("unused bash".into()))
    }
    async fn require_user_confirmation(
        &self,
        _: crate::core::PrimitiveOperation,
        _: &str,
        _: &str,
    ) -> Result<bool, crate::AppError> {
        Ok(true)
    }
}

/// Reconcile A while B is inside an actual QuickJS hostcall. B finishes its
/// current round on old code, then switches at its own idle boundary.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retiring_shared_catalog_leaves_other_sessions_inflight_and_round_untouched() {
    let fixture = plugin_tool_fixture("drain", "pi.registerTool({name:'drain_tool',parameters:{type:'object'},execute:function(_id,p){if(p.block)pi.readFile('/barrier');return {version:'old'};}});");
    let bus = Arc::new(DefaultEventBus::new());
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let primitive = Arc::new(BlockingRead {
        entered: std::sync::Mutex::new(Some(entered_tx)),
        release: std::sync::Mutex::new(release_rx),
    });
    let runtime = Arc::new(PluginRuntimeManager::new());
    let mut manager = Arc::new(PluginManager::new(bus.clone()));
    let inner = Arc::get_mut(&mut manager).unwrap();
    inner.set_plugin_engine(PluginEngine::global(None).unwrap());
    inner.set_plugin_runtime_manager(runtime.clone());
    let executor =
        PluginToolExecutor::with_timeout(Arc::downgrade(&manager), Duration::from_secs(4));
    let registry = Arc::new(DefaultToolRegistry::new(
        executor.clone(),
        Arc::new(TracingAuditRecorder),
    ));
    let dispatcher = Arc::new(
        HostApiDispatcher::new(bus)
            .with_primitive(primitive)
            .with_tokio_handle(tokio::runtime::Handle::current())
            .with_tools(registry.clone())
            .with_plugin_manager(Arc::downgrade(&manager)),
    );
    executor.attach_dispatcher(Arc::downgrade(&dispatcher));
    manager.set_tool_registry(registry.clone());
    manager.set_host_dispatcher(dispatcher);
    let manifest = crate::ext::parse_manifest(
        &fs::read_to_string(fixture.path().join("plugin.json")).unwrap(),
    )
    .unwrap();
    manager
        .register_catalog_plugin(fixture.path(), manifest.clone())
        .unwrap();
    manager.set_catalog_fingerprint("drain", 1);
    let tool = make_tool("drain_tool", "drain");
    registry.register_tool(tool.clone(), "drain").await.unwrap();
    let old_handle = manager.start_session_vm("B", "drain").await.unwrap();
    let task_executor = executor.clone();
    let saved = tool.clone();
    let call = tokio::spawn(async move {
        task_executor
            .execute(&saved, json!({"block":true}), "test", Some("B"))
            .await
    });
    if let Err(error) = tokio::time::timeout(Duration::from_secs(3), entered_rx).await {
        let _ = release_tx.send(());
        let outcome = call.await;
        manager.end_session("B").await.unwrap();
        panic!("hostcall barrier was not entered: {error}; invocation={outcome:?}");
    }
    manager.retire_plugin("drain").unwrap();
    assert!(
        registry.get_tool("drain_tool").await.is_err(),
        "deleted names are unavailable immediately"
    );
    fs::write(fixture.path().join("main.js"), "pi.registerTool({name:'drain_tool',parameters:{type:'object'},execute:function(){return {version:'new'};}});").unwrap();
    manager
        .register_catalog_plugin(fixture.path(), manifest)
        .unwrap();
    manager.set_catalog_fingerprint("drain", 2);
    registry.register_tool(tool.clone(), "drain").await.unwrap();
    assert_eq!(manager.stop_stale_session_vms("A").await.unwrap(), 0);
    assert!(manager.has_session_vm("B", "drain"));
    assert_eq!(
        runtime
            .birth(&crate::ext::PluginRuntimeKey::new("B", "drain"))
            .unwrap()
            .fingerprint,
        Some(1)
    );
    release_tx.send(()).unwrap();
    assert_eq!(call.await.unwrap().unwrap()["version"], "old");
    assert_eq!(
        executor
            .execute(&tool, json!({}), "test", Some("B"))
            .await
            .unwrap()["version"],
        "old",
        "B retains old code for the remainder of the round"
    );
    assert_eq!(manager.stop_stale_session_vms("B").await.unwrap(), 1);
    tokio::time::timeout(Duration::from_secs(1), async {
        while old_handle.current_state() != crate::ext::VmActorState::Stopped {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        executor
            .execute(&tool, json!({}), "test", Some("B"))
            .await
            .unwrap()["version"],
        "new"
    );
    manager.end_session("B").await.unwrap();
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
