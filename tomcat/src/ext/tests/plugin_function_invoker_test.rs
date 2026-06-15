use crate::core::llm::thinking_policy::ThinkingFormat;
use crate::core::tools::web_search::backend::BackendFailure;
use crate::core::tools::web_search::plugin_backend::PluginSearchInvoker;
use crate::ext::{
    ExtPluginSearchInvoker, FunctionRegistry, HostApiDispatcher, ManifestFunction, PluginEngine,
    PluginFunctionInvoker, PluginManager, PluginRuntimeManager, RegisteredFunction,
};
use crate::infra::{DefaultEventBus, TracingAuditRecorder};
use crate::{
    AppError, Capabilities, ChatMessage, ChatRequest, ChatResponse, ChatResponseChoice,
    LlmProvider, LlmResolver, LlmScene, ResolvedCall, StreamEvent,
};
use async_trait::async_trait;
use futures_util::stream;
use serde_json::json;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
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

fn manifest_function(point: &str, function: &str) -> ManifestFunction {
    ManifestFunction {
        point: point.to_string(),
        function: function.to_string(),
    }
}

fn builtin_web_search_backends_fixture() -> tempfile::TempDir {
    let src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join("plugins")
        .join("web-search-backends");
    let tmp = tempfile::tempdir().expect("create builtin web_search tempdir");
    for name in ["plugin.json", "main.js", "README.md"] {
        fs::copy(src.join(name), tmp.path().join(name)).expect("copy builtin web_search asset");
    }
    tmp
}

fn function_search_harness(
    dispatcher: Arc<HostApiDispatcher>,
    timeout: Duration,
) -> (
    Arc<PluginFunctionInvoker>,
    Arc<FunctionRegistry>,
    Arc<PluginManager>,
    Arc<HostApiDispatcher>,
) {
    let event_bus = Arc::new(DefaultEventBus::new());
    let mut manager = Arc::new(PluginManager::new(event_bus.clone()));
    let function_registry = Arc::new(FunctionRegistry::new());
    let inner = Arc::get_mut(&mut manager).expect("plugin manager should be uniquely owned");
    inner.set_plugin_engine(PluginEngine::global(None).expect("create quickjs engine"));
    inner.set_plugin_runtime_manager(Arc::new(PluginRuntimeManager::new()));
    inner.set_audit_recorder(Arc::new(TracingAuditRecorder));
    inner.set_function_registry(function_registry.clone());

    let invoker = PluginFunctionInvoker::with_timeout(Arc::downgrade(&manager), timeout);
    invoker.attach_dispatcher(Arc::downgrade(&dispatcher));
    manager.set_host_dispatcher(dispatcher.clone());

    (invoker, function_registry, manager, dispatcher)
}

#[derive(Clone)]
struct FixedResolver {
    provider: Arc<dyn LlmProvider>,
}

impl LlmResolver for FixedResolver {
    fn resolve(
        &self,
        _scene: LlmScene,
        session_override: Option<&str>,
    ) -> Result<ResolvedCall, AppError> {
        Ok(ResolvedCall {
            provider_impl: self.provider.clone(),
            model: session_override.unwrap_or("mimo-v2.5-pro").to_string(),
            api: "openai".to_string(),
            provider: "mimo".to_string(),
            base_url: None,
            key_source: "test".to_string(),
            thinking_format: ThinkingFormat::Doubao,
            capabilities: Capabilities::default(),
        })
    }
}

type RecordingMimoCalls = Arc<Mutex<Vec<(String, Vec<serde_json::Value>)>>>;

struct RecordingMimoLlm {
    calls: RecordingMimoCalls,
}

#[async_trait]
impl LlmProvider for RecordingMimoLlm {
    fn provider_name(&self) -> &str {
        "recording-mimo"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, AppError> {
        self.calls
            .lock()
            .unwrap()
            .push((req.model.clone(), req.tools.clone().unwrap_or_default()));
        let mut message = ChatMessage::assistant("");
        message.annotations = Some(vec![
            json!({
                "type": "url_citation",
                "url": "https://docs.rs/reqwest",
                "title": "reqwest",
                "summary": "HTTP client",
                "publish_time": "2026-06-15"
            }),
            json!({
                "type": "url_citation",
                "url": "https://docs.rs/reqwest",
                "title": "duplicate reqwest",
                "summary": "duplicate"
            }),
            json!({
                "type": "ignored",
                "value": true
            }),
        ]);
        Ok(ChatResponse {
            id: Some("mock-mimo".to_string()),
            choices: vec![ChatResponseChoice {
                index: 0,
                message,
                finish_reason: Some("stop".to_string()),
            }],
            usage: None,
        })
    }

    async fn chat_stream(
        &self,
        _req: ChatRequest,
    ) -> Result<
        Box<dyn futures_util::Stream<Item = Result<StreamEvent, AppError>> + Send + Unpin>,
        AppError,
    > {
        Ok(Box::new(stream::iter(vec![Ok(
            StreamEvent::ContentDelta {
                delta: String::new(),
            },
        )])))
    }

    fn count_tokens(&self, _messages: &[ChatMessage]) -> Result<u32, AppError> {
        Ok(0)
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn official_web_search_backend_function_maps_mimo_annotations() {
    let plugin_dir = builtin_web_search_backends_fixture();
    let calls = Arc::new(Mutex::new(Vec::<(String, Vec<serde_json::Value>)>::new()));
    let llm = Arc::new(RecordingMimoLlm {
        calls: calls.clone(),
    });
    let dispatcher = Arc::new(
        HostApiDispatcher::new(Arc::new(DefaultEventBus::new()))
            .with_tokio_handle(tokio::runtime::Handle::current())
            .with_llm_resolver(Arc::new(FixedResolver { provider: llm })),
    );
    let (invoker, function_registry, manager, _dispatcher) =
        function_search_harness(dispatcher, Duration::from_secs(1));

    function_registry.register_plugin_functions(
        "tomcat.web-search-backends",
        plugin_dir.path(),
        &[manifest_function("web_search.backend", "webSearchBackend")],
    );
    manager
        .load_plugin(plugin_dir.path())
        .expect("load builtin web_search plugin");

    let result = invoker
        .execute(
            &registered_function(
                &plugin_dir,
                "tomcat.web-search-backends",
                "web_search.backend",
                "webSearchBackend",
            ),
            json!({
                "backend": "mimo",
                "query": "reqwest rust",
                "count": 3,
                "country": "us",
                "language": "en",
                "domainFilter": ["docs.rs"]
            }),
            Some("s1"),
        )
        .await
        .expect("builtin web_search function call");

    assert_eq!(result["backend"], "mimo");
    assert_eq!(result["hits"].as_array().map(Vec::len), Some(1));
    assert_eq!(result["hits"][0]["url"], json!("https://docs.rs/reqwest"));
    assert_eq!(result["hits"][0]["title"], json!("reqwest"));
    assert!(result["warnings"]
        .as_array()
        .expect("warnings array")
        .iter()
        .any(|warning| warning == "mimo_ignores_language"));

    {
        let seen = calls.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0, "mimo-v2.5-pro");
        assert_eq!(seen[0].1[0]["type"], json!("web_search"));
        assert_eq!(
            seen[0].1[0]["filters"]["allowed_domains"],
            json!(["docs.rs"])
        );
        assert_eq!(seen[0].1[0]["user_location"]["country"], json!("US"));
    }

    manager.end_session("s1").await.expect("end session");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ext_plugin_search_invoker_retries_after_unsupported_backend() {
    let plugin_a = plugin_function_fixture(
        "plugin-a",
        &[("web_search.backend", "webSearchBackend")],
        r#"
pi.registerFunction("webSearchBackend", function () {
  return { hits: [], warnings: [], unsupported_backend: true };
});
"#,
    );
    let plugin_b = plugin_function_fixture(
        "plugin-b",
        &[("web_search.backend", "webSearchBackend")],
        r#"
pi.registerFunction("webSearchBackend", function (params) {
  return {
    backend: params.backend,
    hits: [{ title: "Docs", url: "https://docs.rs" }],
    warnings: []
  };
});
"#,
    );
    let dispatcher = Arc::new(
        HostApiDispatcher::new(Arc::new(DefaultEventBus::new()))
            .with_tokio_handle(tokio::runtime::Handle::current()),
    );
    let (invoker, function_registry, manager, _dispatcher) =
        function_search_harness(dispatcher, Duration::from_secs(1));

    function_registry.register_plugin_functions(
        "plugin-a",
        plugin_a.path(),
        &[manifest_function("web_search.backend", "webSearchBackend")],
    );
    function_registry.register_plugin_functions(
        "plugin-b",
        plugin_b.path(),
        &[manifest_function("web_search.backend", "webSearchBackend")],
    );
    manager.load_plugin(plugin_a.path()).expect("load plugin-a");
    manager.load_plugin(plugin_b.path()).expect("load plugin-b");

    let search_invoker = ExtPluginSearchInvoker::new(function_registry, invoker);
    let result = search_invoker
        .search("mimo", json!({ "backend": "mimo", "query": "rust" }), "s1")
        .await
        .expect("second provider should handle backend");

    assert_eq!(result["backend"], "mimo");
    assert_eq!(result["hits"][0]["url"], json!("https://docs.rs"));

    manager.end_session("s1").await.expect("end session");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ext_plugin_search_invoker_reports_clear_error_when_all_providers_unsupported() {
    let plugin_a = plugin_function_fixture(
        "plugin-a",
        &[("web_search.backend", "webSearchBackend")],
        r#"
pi.registerFunction("webSearchBackend", function () {
  return { hits: [], warnings: [], unsupportedBackend: true };
});
"#,
    );
    let dispatcher = Arc::new(
        HostApiDispatcher::new(Arc::new(DefaultEventBus::new()))
            .with_tokio_handle(tokio::runtime::Handle::current()),
    );
    let (invoker, function_registry, manager, _dispatcher) =
        function_search_harness(dispatcher, Duration::from_secs(1));

    function_registry.register_plugin_functions(
        "plugin-a",
        plugin_a.path(),
        &[manifest_function("web_search.backend", "webSearchBackend")],
    );
    manager.load_plugin(plugin_a.path()).expect("load plugin-a");

    let search_invoker = ExtPluginSearchInvoker::new(function_registry, invoker);
    let err = search_invoker
        .search("mimo", json!({ "backend": "mimo", "query": "rust" }), "s1")
        .await
        .expect_err("all providers unsupported should fail clearly");

    match err {
        BackendFailure::Incompatible { detail } => {
            assert!(detail.contains("未找到名为 `mimo` 的 web_search 插件后端"));
        }
        other => panic!("unexpected error: {other:?}"),
    }

    manager.end_session("s1").await.expect("end session");
}
