use crate::ext::{
    HostApiDispatcher, PluginEngine, PluginFunctionInvoker, PluginManager, PluginRuntimeManager,
    RegisteredFunction,
};
use crate::infra::{DefaultEventBus, TracingAuditRecorder};
use serde_json::json;
use std::fs;
use std::sync::Arc;
use std::time::Duration;

fn plugin_function_fixture(
    plugin_id: &str,
    functions: &[(&str, &str)],
    script: &str,
) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("create plugin tempdir");
    let manifest_functions = functions
        .iter()
        .map(|(point, function)| {
            json!({
                "point": point,
                "function": function,
            })
        })
        .collect::<Vec<_>>();
    let manifest = json!({
        "id": plugin_id,
        "name": plugin_id,
        "version": "0.1.0",
        "description": "plugin function test fixture",
        "author": "tests",
        "main": "main.js",
        "requiredPermissions": [],
        "requiredApiVersion": "1.0",
        "tags": [],
        "tools": [],
        "functions": manifest_functions,
    });
    fs::write(
        tmp.path().join("plugin.json"),
        serde_json::to_string_pretty(&manifest).expect("serialize manifest"),
    )
    .expect("write plugin.json");
    fs::write(tmp.path().join("main.js"), script).expect("write main.js");
    tmp
}

fn registered_function(
    plugin_dir: &tempfile::TempDir,
    plugin_id: &str,
    point: &str,
    function: &str,
) -> RegisteredFunction {
    RegisteredFunction {
        plugin_id: plugin_id.to_string(),
        plugin_root: plugin_dir.path().to_path_buf(),
        point: point.to_string(),
        function: function.to_string(),
    }
}

fn real_invoker_harness(
    plugin_id: &str,
    functions: &[(&str, &str)],
    script: &str,
    timeout: Duration,
) -> (
    Arc<PluginFunctionInvoker>,
    Arc<PluginManager>,
    Arc<HostApiDispatcher>,
    tempfile::TempDir,
) {
    let plugin_dir = plugin_function_fixture(plugin_id, functions, script);
    let event_bus = Arc::new(DefaultEventBus::new());
    let mut manager = Arc::new(PluginManager::new(event_bus.clone()));
    let inner = Arc::get_mut(&mut manager).expect("plugin manager should be uniquely owned");
    inner.set_plugin_engine(PluginEngine::global(None).expect("create quickjs engine"));
    inner.set_plugin_runtime_manager(Arc::new(PluginRuntimeManager::new()));
    inner.set_audit_recorder(Arc::new(TracingAuditRecorder));

    let invoker = PluginFunctionInvoker::with_timeout(Arc::downgrade(&manager), timeout);
    let dispatcher = Arc::new(
        HostApiDispatcher::new(event_bus.clone())
            .with_tokio_handle(tokio::runtime::Handle::current()),
    );
    invoker.attach_dispatcher(Arc::downgrade(&dispatcher));
    manager.set_host_dispatcher(dispatcher.clone());
    manager
        .load_plugin(plugin_dir.path())
        .expect("load plugin function fixture");

    (invoker, manager, dispatcher, plugin_dir)
}

#[tokio::test]
async fn plugin_function_invoker_requires_session_id() {
    let invoker = PluginFunctionInvoker::new(std::sync::Weak::new());
    let plugin_dir = tempfile::tempdir().expect("tempdir");
    let err = invoker
        .execute(
            &RegisteredFunction {
                plugin_id: "plugin-a".to_string(),
                plugin_root: plugin_dir.path().to_path_buf(),
                point: "test.echo".to_string(),
                function: "echo_host".to_string(),
            },
            json!({}),
            None,
        )
        .await
        .expect_err("missing session_id should fail before runtime access");
    assert!(
        err.to_string().contains("插件宿主函数执行缺少 session_id"),
        "error should explain the missing session id: {err}"
    );
}

#[tokio::test]
async fn plugin_function_invoker_errors_when_dispatcher_dropped() {
    let manager = Arc::new(PluginManager::new(Arc::new(DefaultEventBus::new())));
    let invoker = PluginFunctionInvoker::new(Arc::downgrade(&manager));
    let plugin_dir = tempfile::tempdir().expect("tempdir");

    let err = invoker
        .execute(
            &RegisteredFunction {
                plugin_id: "plugin-a".to_string(),
                plugin_root: plugin_dir.path().to_path_buf(),
                point: "test.echo".to_string(),
                function: "echo_host".to_string(),
            },
            json!({}),
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
async fn plugin_function_invoker_roundtrips_registered_function() {
    let (invoker, manager, _dispatcher, plugin_dir) = real_invoker_harness(
        "plugin-echo",
        &[("test.echo", "echoHost")],
        r#"
pi.registerFunction("echoHost", function (params) {
  return { echoed: params.text, kind: "function" };
});
"#,
        Duration::from_secs(1),
    );
    let result = invoker
        .execute(
            &registered_function(&plugin_dir, "plugin-echo", "test.echo", "echoHost"),
            json!({ "text": "hello" }),
            Some("s1"),
        )
        .await
        .expect("function call should complete");
    assert_eq!(result["echoed"], "hello");
    assert_eq!(result["kind"], "function");

    manager.end_session("s1").await.expect("end session");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plugin_function_invoker_times_out_and_drops_waiter() {
    let (invoker, manager, dispatcher, plugin_dir) = real_invoker_harness(
        "plugin-hang",
        &[("test.counter", "nextCount")],
        r#"
pi.registerFunction("nextCount", function () {
  return new Promise(function () {});
});
"#,
        Duration::from_millis(50),
    );

    assert_eq!(dispatcher.command_waiter_count(), 0);
    let err = invoker
        .execute(
            &registered_function(&plugin_dir, "plugin-hang", "test.counter", "nextCount"),
            json!({}),
            Some("s1"),
        )
        .await
        .expect_err("hung plugin function should time out");
    assert!(
        err.to_string().contains("插件宿主函数执行超时: nextCount"),
        "timeout error should mention the function name: {err}"
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn function_channel_missing_handler_errors_cleanly() {
    let (invoker, manager, _dispatcher, plugin_dir) = real_invoker_harness(
        "plugin-mismatch",
        &[("test.echo", "echoHost")],
        r#"
pi.registerFunction("otherName", function (params) {
  return { echoed: params.text };
});
"#,
        Duration::from_secs(1),
    );

    let err = invoker
        .execute(
            &registered_function(&plugin_dir, "plugin-mismatch", "test.echo", "echoHost"),
            json!({ "text": "hi" }),
            Some("s1"),
        )
        .await
        .expect_err("manifest/js mismatch should fail cleanly");
    assert!(
        err.to_string().contains("function not found: echoHost"),
        "missing handler should surface a precise error: {err}"
    );

    manager.end_session("s1").await.expect("end session");
}
