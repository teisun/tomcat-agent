use crate::core::llm::thinking_policy::ThinkingFormat;
use crate::core::tools::web_search::backend::BackendFailure;
use crate::core::tools::web_search::plugin_backend::PluginSearchInvoker;
use crate::ext::{
    ExtPluginSearchInvoker, FunctionRegistry, HostApiDispatcher, HostRequest, ManifestFunction,
    PluginEngine, PluginFunctionInvoker, PluginManager, PluginRuntimeManager, RegisteredFunction,
};
use crate::infra::{DefaultEventBus, TracingAuditRecorder};
use crate::{
    AppError, Capabilities, ChatMessage, ChatRequest, ChatResponse, ChatResponseChoice,
    LlmProvider, LlmResolver, LlmScene, ResolvedCall, StreamEvent,
};
use async_trait::async_trait;
use futures_util::stream;
use rcgen::generate_simple_self_signed;
use serde_json::json;
use serial_test::serial;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio_rustls::rustls::{
    self,
    pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer},
};
use tokio_rustls::TlsAcceptor;

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

fn builtin_web_search_backends_fixture_with_main_suffix(suffix: &str) -> tempfile::TempDir {
    let tmp = builtin_web_search_backends_fixture();
    if !suffix.trim().is_empty() {
        let main_path = tmp.path().join("main.js");
        let mut script = fs::read_to_string(&main_path).expect("read builtin main.js");
        script.push('\n');
        script.push_str(suffix);
        fs::write(main_path, script).expect("rewrite builtin main.js with test suffix");
    }
    tmp
}

fn plugin_function_fixture_with_manifest(
    plugin_id: &str,
    manifest_overrides: serde_json::Value,
    script: &str,
) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("create plugin tempdir");
    let mut manifest = json!({
        "id": plugin_id,
        "name": plugin_id,
        "version": "0.1.0",
        "description": "plugin function test fixture",
        "author": "tests",
        "main": "main.js",
        "requiredPermissions": [],
        "requiredSecrets": [],
        "allowedHosts": [],
        "requiredApiVersion": "1.0",
        "tags": [],
        "tools": [],
        "functions": []
    });
    if let (Some(base), Some(overrides)) =
        (manifest.as_object_mut(), manifest_overrides.as_object())
    {
        for (key, value) in overrides {
            base.insert(key.clone(), value.clone());
        }
    }
    fs::write(
        tmp.path().join("plugin.json"),
        serde_json::to_string_pretty(&manifest).expect("serialize manifest"),
    )
    .expect("write plugin.json");
    fs::write(tmp.path().join("main.js"), script).expect("write main.js");
    tmp
}

struct EnvGuard {
    saved: Vec<(String, Option<String>)>,
}

impl EnvGuard {
    fn set_many(entries: &[(&str, Option<&str>)]) -> Self {
        let saved = entries
            .iter()
            .map(|(key, value)| {
                let previous = std::env::var(key).ok();
                match value {
                    Some(next) => std::env::set_var(key, next),
                    None => std::env::remove_var(key),
                }
                ((*key).to_string(), previous)
            })
            .collect();
        Self { saved }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        while let Some((key, value)) = self.saved.pop() {
            match value {
                Some(previous) => std::env::set_var(&key, previous),
                None => std::env::remove_var(&key),
            }
        }
    }
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

fn function_search_harness_with_net_fetch(
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

    let dispatcher = Arc::new(
        HostApiDispatcher::new(event_bus.clone())
            .with_tokio_handle(tokio::runtime::Handle::current())
            .with_plugin_manager(Arc::downgrade(&manager)),
    );
    let invoker = PluginFunctionInvoker::with_timeout(Arc::downgrade(&manager), timeout);
    invoker.attach_dispatcher(Arc::downgrade(&dispatcher));
    manager.set_host_dispatcher(dispatcher.clone());

    (invoker, function_registry, manager, dispatcher)
}

fn function_search_harness_with_custom_net_fetch(
    timeout: Duration,
    client: reqwest::Client,
    concurrency: usize,
    max_body_bytes: usize,
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

    let dispatcher = Arc::new(
        HostApiDispatcher::new(event_bus.clone())
            .with_tokio_handle(tokio::runtime::Handle::current())
            .with_plugin_manager(Arc::downgrade(&manager))
            .with_fetch_http_client(client)
            .with_fetch_concurrency(concurrency)
            .with_fetch_max_body_bytes(max_body_bytes),
    );
    let invoker = PluginFunctionInvoker::with_timeout(Arc::downgrade(&manager), timeout);
    invoker.attach_dispatcher(Arc::downgrade(&dispatcher));
    manager.set_host_dispatcher(dispatcher.clone());

    (invoker, function_registry, manager, dispatcher)
}

struct HttpsTestServer {
    addr: std::net::SocketAddr,
    max_concurrency: Arc<AtomicUsize>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
}

impl HttpsTestServer {
    async fn start(
        hostname: &str,
        status_line: &str,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
        delay: Duration,
    ) -> Self {
        let certified = generate_simple_self_signed(vec![hostname.to_string()])
            .expect("generate self-signed cert");
        let cert_chain = vec![CertificateDer::from(certified.cert.der().to_vec())];
        let private_key = PrivateKeyDer::from(PrivatePkcs8KeyDer::from(
            certified.signing_key.serialize_der(),
        ));
        let server_config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(cert_chain, private_key)
            .expect("build rustls server config");
        let acceptor = TlsAcceptor::from(Arc::new(server_config));
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind https test server");
        let addr = listener.local_addr().expect("listener addr");
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
        let current_concurrency = Arc::new(AtomicUsize::new(0));
        let max_concurrency = Arc::new(AtomicUsize::new(0));
        let task_current = Arc::clone(&current_concurrency);
        let task_max = Arc::clone(&max_concurrency);
        let status_line = status_line.to_string();
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => break,
                    accept = listener.accept() => {
                        let Ok((stream, _)) = accept else { break; };
                        let acceptor = acceptor.clone();
                        let headers = headers.clone();
                        let body = body.clone();
                        let status_line = status_line.clone();
                        let task_current = Arc::clone(&task_current);
                        let task_max = Arc::clone(&task_max);
                        tokio::spawn(async move {
                            let Ok(mut tls_stream) = acceptor.accept(stream).await else {
                                return;
                            };
                            let mut request_buf = vec![0u8; 4096];
                            let _ = tls_stream.read(&mut request_buf).await;
                            let in_flight = task_current.fetch_add(1, Ordering::SeqCst) + 1;
                            task_max.fetch_max(in_flight, Ordering::SeqCst);
                            if !delay.is_zero() {
                                tokio::time::sleep(delay).await;
                            }
                            let mut response = format!("HTTP/1.1 {status_line}\r\n");
                            for (name, value) in &headers {
                                response.push_str(name);
                                response.push_str(": ");
                                response.push_str(value);
                                response.push_str("\r\n");
                            }
                            response.push_str(&format!("Content-Length: {}\r\n", body.len()));
                            response.push_str("Connection: close\r\n\r\n");
                            let _ = tls_stream.write_all(response.as_bytes()).await;
                            let _ = tls_stream.write_all(&body).await;
                            let _ = tls_stream.flush().await;
                            let _ = tls_stream.shutdown().await;
                            task_current.fetch_sub(1, Ordering::SeqCst);
                        });
                    }
                }
            }
        });
        Self {
            addr,
            max_concurrency,
            shutdown_tx: Some(shutdown_tx),
            task,
        }
    }

    fn client_for(&self, hostname: &str, timeout: Duration) -> reqwest::Client {
        reqwest::Client::builder()
            .no_proxy()
            .danger_accept_invalid_certs(true)
            .http1_only()
            .pool_max_idle_per_host(0)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(timeout)
            .resolve(hostname, self.addr)
            .build()
            .expect("build https test client")
    }

    fn max_concurrency(&self) -> usize {
        self.max_concurrency.load(Ordering::SeqCst)
    }
}

impl Drop for HttpsTestServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        self.task.abort();
    }
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
async fn official_web_search_backend_function_auto_prefers_mimo_from_config() {
    let plugin_dir = builtin_web_search_backends_fixture_with_main_suffix(
        r#"
searchWithMimo = async function (req) {
  return {
    backend: req.backend,
    hits: [{ title: "MiMo", url: "https://mimo.example.com" }],
    warnings: ["auto_prefers_mimo"]
  };
};
searchWithTavily = async function (req) {
  return {
    backend: req.backend,
    hits: [{ title: "Tavily", url: "https://tavily.example.com" }],
    warnings: []
  };
};
backends.mimo = searchWithMimo;
backends.tavily = searchWithTavily;
"#,
    );
    let dispatcher = Arc::new(
        HostApiDispatcher::new(Arc::new(DefaultEventBus::new()))
            .with_tokio_handle(tokio::runtime::Handle::current()),
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
                "backend": "auto",
                "query": "reqwest rust",
                "count": 3
            }),
            Some("s1"),
        )
        .await
        .expect("builtin web_search function call");

    assert_eq!(result["backend"], "mimo");
    assert_eq!(result["hits"][0]["url"], json!("https://mimo.example.com"));
    assert!(result["warnings"]
        .as_array()
        .expect("warnings array")
        .iter()
        .any(|warning| warning == "auto_prefers_mimo"));

    manager.end_session("s1").await.expect("end session");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn official_web_search_backend_function_returns_missing_key_sentinel_for_tavily() {
    let plugin_dir = builtin_web_search_backends_fixture();
    let _env = EnvGuard::set_many(&[
        ("TAVILY_API_KEY", None),
        ("BRAVE_API_KEY", None),
        ("SERPER_API_KEY", None),
    ]);
    let (invoker, function_registry, manager, dispatcher) =
        function_search_harness_with_net_fetch(Duration::from_secs(1));

    function_registry.register_plugin_functions(
        "tomcat.web-search-backends",
        plugin_dir.path(),
        &[manifest_function("web_search.backend", "webSearchBackend")],
    );
    manager
        .load_plugin(plugin_dir.path())
        .expect("load builtin web_search plugin");
    let loaded = manager
        .get_plugin("tomcat.web-search-backends")
        .expect("plugin should be loaded");
    assert!(loaded
        .manifest
        .required_secrets
        .iter()
        .any(|name| name == "TAVILY_API_KEY"));

    let preflight = dispatcher
        .dispatch_async(
            "s1/tomcat.web-search-backends",
            HostRequest {
                module: "net".to_string(),
                method: "fetch".to_string(),
                params: json!({
                    "method": "POST",
                    "url": "https://api.tavily.com/search",
                    "headers": {
                        "Authorization": "Bearer {{secret:TAVILY_API_KEY}}",
                        "Content-Type": "application/json"
                    },
                    "body": {
                        "query": "tokio rust"
                    }
                }),
                call_id: None,
            },
        )
        .await
        .expect("hostcall should resolve with HostResponse");
    assert!(!preflight.ok);
    assert!(preflight
        .error
        .as_deref()
        .unwrap_or("")
        .contains("missing_secret"));

    let result = invoker
        .execute(
            &registered_function(
                &plugin_dir,
                "tomcat.web-search-backends",
                "web_search.backend",
                "webSearchBackend",
            ),
            json!({
                "backend": "tavily",
                "query": "tokio rust",
                "count": 3
            }),
            Some("s1"),
        )
        .await
        .expect("builtin web_search function call");

    assert_eq!(result["backend"], "tavily");
    assert_eq!(result["hits"].as_array().map(Vec::len), Some(0));
    assert!(result["warnings"]
        .as_array()
        .expect("warnings array")
        .iter()
        .any(|warning| warning == "__missing_key__:TAVILY_API_KEY"));

    manager.end_session("s1").await.expect("end session");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn official_web_search_backend_function_executes_tavily_via_fetch_stub() {
    let plugin_dir = builtin_web_search_backends_fixture_with_main_suffix(
        r#"
pi.fetch = async function () {
  return {
    status: 200,
    body: JSON.stringify({
      results: [
        {
          title: "Tokio",
          url: "https://tokio.rs",
          content: "Async runtime for Rust"
        },
        {
          title: "Drop Missing Url"
        }
      ]
    })
  };
};
"#,
    );
    let dispatcher = Arc::new(
        HostApiDispatcher::new(Arc::new(DefaultEventBus::new()))
            .with_tokio_handle(tokio::runtime::Handle::current()),
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
                "backend": "tavily",
                "query": "tokio rust",
                "count": 3,
                "country": "us",
                "language": "en",
                "domainFilter": ["tokio.rs"]
            }),
            Some("s1"),
        )
        .await
        .expect("builtin web_search function call");

    assert_eq!(result["backend"], "tavily");
    assert_eq!(result["hits"].as_array().map(Vec::len), Some(1));
    assert_eq!(result["hits"][0]["url"], json!("https://tokio.rs"));
    assert!(result["warnings"]
        .as_array()
        .expect("warnings array")
        .iter()
        .any(|warning| warning == "tavily_ignores_country_language"));

    manager.end_session("s1").await.expect("end session");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn official_web_search_backend_function_returns_unauthorized_sentinel_for_brave() {
    let plugin_dir = builtin_web_search_backends_fixture_with_main_suffix(
        r#"
pi.fetch = async function () {
  return {
    status: 403,
    body: JSON.stringify({ message: "forbidden" })
  };
};
"#,
    );
    let dispatcher = Arc::new(
        HostApiDispatcher::new(Arc::new(DefaultEventBus::new()))
            .with_tokio_handle(tokio::runtime::Handle::current()),
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
                "backend": "brave",
                "query": "reqwest rust",
                "count": 3
            }),
            Some("s1"),
        )
        .await
        .expect("builtin web_search function call");

    assert_eq!(result["backend"], "brave");
    assert_eq!(result["hits"].as_array().map(Vec::len), Some(0));
    assert!(result["warnings"]
        .as_array()
        .expect("warnings array")
        .iter()
        .any(|warning| warning == "__unauthorized__:403"));

    manager.end_session("s1").await.expect("end session");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn net_fetch_hostcall_denies_plugins_without_permission() {
    let plugin_dir = plugin_function_fixture(
        "no-net-permission",
        &[("web_search.backend", "webSearchBackend")],
        "pi.registerFunction('webSearchBackend', function () { return { hits: [], warnings: [] }; });",
    );
    let (_invoker, _function_registry, manager, dispatcher) =
        function_search_harness_with_net_fetch(Duration::from_secs(1));
    manager
        .load_plugin(plugin_dir.path())
        .expect("load plugin without net:fetch");

    let response = dispatcher
        .dispatch_async(
            "s1/no-net-permission",
            HostRequest {
                module: "net".to_string(),
                method: "fetch".to_string(),
                params: json!({
                    "url": "https://api.tavily.com/search"
                }),
                call_id: None,
            },
        )
        .await
        .expect("hostcall should resolve with HostResponse");

    assert!(!response.ok);
    assert!(response
        .error
        .as_deref()
        .unwrap_or("")
        .contains("permission_denied"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn net_fetch_hostcall_rejects_secret_and_host_policy_violations() {
    let plugin_dir = plugin_function_fixture_with_manifest(
        "net-fetch-policy",
        json!({
            "requiredPermissions": ["net:fetch"],
            "requiredSecrets": ["TAVILY_API_KEY"],
            "allowedHosts": ["api.tavily.com"]
        }),
        "pi.registerFunction('webSearchBackend', function () { return { hits: [], warnings: [] }; });",
    );
    let (_invoker, _function_registry, manager, dispatcher) =
        function_search_harness_with_net_fetch(Duration::from_secs(1));
    manager
        .load_plugin(plugin_dir.path())
        .expect("load plugin with net:fetch policy");

    let cases = vec![
        (
            json!({
                "url": "http://api.tavily.com/search"
            }),
            "ssrf_rejected",
        ),
        (
            json!({
                "url": "https://169.254.10.20/search"
            }),
            "ssrf_rejected",
        ),
        (
            json!({
                "url": "https://printer.local/search"
            }),
            "ssrf_rejected",
        ),
        (
            json!({
                "url": "https://example.com/search"
            }),
            "host_not_allowed",
        ),
        (
            json!({
                "url": "https://api.tavily.com/search",
                "query": {
                    "q": "{{secret:TAVILY_API_KEY}}"
                }
            }),
            "forbidden_secret",
        ),
        (
            json!({
                "url": "https://api.tavily.com/search",
                "headers": {
                    "Authorization": "Bearer {{secret:BRAVE_API_KEY}}"
                }
            }),
            "forbidden_secret",
        ),
    ];

    for (params, expected_code) in cases {
        let response = dispatcher
            .dispatch_async(
                "s1/net-fetch-policy",
                HostRequest {
                    module: "net".to_string(),
                    method: "fetch".to_string(),
                    params,
                    call_id: None,
                },
            )
            .await
            .expect("hostcall should resolve with HostResponse");
        assert!(!response.ok, "case `{expected_code}` should fail");
        assert!(
            response
                .error
                .as_deref()
                .unwrap_or("")
                .contains(expected_code),
            "expected error code `{expected_code}`, got {:?}",
            response.error
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn net_fetch_hostcall_rejects_redirect_responses_by_default() {
    let server = HttpsTestServer::start(
        "api.tavily.com",
        "302 Found",
        vec![(
            "Location".to_string(),
            "https://api.tavily.com/redirected".to_string(),
        )],
        Vec::new(),
        Duration::ZERO,
    )
    .await;
    let client = server.client_for("api.tavily.com", Duration::from_secs(5));
    let plugin_dir = plugin_function_fixture_with_manifest(
        "net-fetch-redirect",
        json!({
            "requiredPermissions": ["net:fetch"],
            "allowedHosts": ["api.tavily.com"]
        }),
        "",
    );
    let (_invoker, _function_registry, manager, dispatcher) =
        function_search_harness_with_custom_net_fetch(Duration::from_secs(1), client, 5, 1024);
    manager
        .load_plugin(plugin_dir.path())
        .expect("load plugin with redirect policy");

    let response = dispatcher
        .dispatch_async(
            "s1/net-fetch-redirect",
            HostRequest {
                module: "net".to_string(),
                method: "fetch".to_string(),
                params: json!({
                    "url": "https://api.tavily.com/search"
                }),
                call_id: None,
            },
        )
        .await
        .expect("hostcall should resolve with HostResponse");

    assert!(!response.ok);
    assert!(
        response
            .error
            .as_deref()
            .unwrap_or("")
            .contains("redirect_not_allowed"),
        "expected redirect_not_allowed, got {:?}",
        response.error
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn net_fetch_hostcall_enforces_response_size_limit() {
    let server = HttpsTestServer::start(
        "api.tavily.com",
        "200 OK",
        vec![("Content-Type".to_string(), "application/json".to_string())],
        b"{\"data\":\"this-body-is-way-too-large-for-the-test-limit\"}".to_vec(),
        Duration::ZERO,
    )
    .await;
    let client = server.client_for("api.tavily.com", Duration::from_secs(5));
    let plugin_dir = plugin_function_fixture_with_manifest(
        "net-fetch-large-body",
        json!({
            "requiredPermissions": ["net:fetch"],
            "allowedHosts": ["api.tavily.com"]
        }),
        "",
    );
    let (_invoker, _function_registry, manager, dispatcher) =
        function_search_harness_with_custom_net_fetch(Duration::from_secs(1), client, 5, 16);
    manager
        .load_plugin(plugin_dir.path())
        .expect("load plugin with size limit");

    let response = dispatcher
        .dispatch_async(
            "s1/net-fetch-large-body",
            HostRequest {
                module: "net".to_string(),
                method: "fetch".to_string(),
                params: json!({
                    "url": "https://api.tavily.com/search"
                }),
                call_id: None,
            },
        )
        .await
        .expect("hostcall should resolve with HostResponse");

    assert!(!response.ok);
    assert!(response
        .error
        .as_deref()
        .unwrap_or("")
        .contains("response_too_large"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn net_fetch_hostcall_enforces_request_timeout() {
    let server = HttpsTestServer::start(
        "api.tavily.com",
        "200 OK",
        vec![("Content-Type".to_string(), "application/json".to_string())],
        b"{}".to_vec(),
        Duration::from_millis(200),
    )
    .await;
    let client = server.client_for("api.tavily.com", Duration::from_millis(50));
    let plugin_dir = plugin_function_fixture_with_manifest(
        "net-fetch-timeout",
        json!({
            "requiredPermissions": ["net:fetch"],
            "allowedHosts": ["api.tavily.com"]
        }),
        "",
    );
    let (_invoker, _function_registry, manager, dispatcher) =
        function_search_harness_with_custom_net_fetch(Duration::from_secs(1), client, 5, 1024);
    manager
        .load_plugin(plugin_dir.path())
        .expect("load plugin with timeout policy");

    let response = dispatcher
        .dispatch_async(
            "s1/net-fetch-timeout",
            HostRequest {
                module: "net".to_string(),
                method: "fetch".to_string(),
                params: json!({
                    "url": "https://api.tavily.com/search"
                }),
                call_id: None,
            },
        )
        .await
        .expect("hostcall should resolve with HostResponse");

    assert!(!response.ok);
    assert!(response.error.as_deref().unwrap_or("").contains("timeout"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn net_fetch_hostcall_respects_concurrency_limit() {
    let server = HttpsTestServer::start(
        "api.tavily.com",
        "200 OK",
        vec![("Content-Type".to_string(), "application/json".to_string())],
        b"{}".to_vec(),
        Duration::from_millis(120),
    )
    .await;
    let client = server.client_for("api.tavily.com", Duration::from_secs(10));
    let plugin_dir = plugin_function_fixture_with_manifest(
        "net-fetch-concurrency",
        json!({
            "requiredPermissions": ["net:fetch"],
            "allowedHosts": ["api.tavily.com"]
        }),
        "",
    );
    let (_invoker, _function_registry, manager, dispatcher) =
        function_search_harness_with_custom_net_fetch(Duration::from_secs(1), client, 1, 1024);
    manager
        .load_plugin(plugin_dir.path())
        .expect("load plugin with concurrency policy");

    let request = HostRequest {
        module: "net".to_string(),
        method: "fetch".to_string(),
        params: json!({
            "url": "https://api.tavily.com/search"
        }),
        call_id: None,
    };
    let dispatcher_a = dispatcher.clone();
    let dispatcher_b = dispatcher.clone();
    let request_first = request.clone();
    let request_second = request;
    let first = tokio::spawn(async move {
        dispatcher_a
            .dispatch_async("s1/net-fetch-concurrency", request_first)
            .await
            .expect("first hostcall response")
    });
    let second = tokio::spawn(async move {
        dispatcher_b
            .dispatch_async("s1/net-fetch-concurrency", request_second)
            .await
            .expect("second hostcall response")
    });

    let first = first.await.expect("join first");
    let second = second.await.expect("join second");
    assert!(first.ok, "first request should succeed, got {:?}", first.error);
    assert!(second.ok, "second request should succeed, got {:?}", second.error);
    assert_eq!(
        server.max_concurrency(),
        1,
        "fetch semaphore should serialize requests when concurrency=1"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ext_plugin_search_invoker_uses_first_registered_provider_only() {
    let plugin_a = plugin_function_fixture(
        "plugin-a",
        &[("web_search.backend", "webSearchBackend")],
        r#"
pi.registerFunction("webSearchBackend", function (params) {
  return {
    backend: params.backend,
    provider: "plugin-a",
    hits: [{ title: "Docs A", url: "https://a.example.com" }],
    warnings: []
  };
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
    provider: "plugin-b",
    hits: [{ title: "Docs B", url: "https://b.example.com" }],
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
        .expect("first provider should satisfy request");

    assert_eq!(result["provider"], "plugin-a");
    assert_eq!(result["hits"][0]["url"], json!("https://a.example.com"));
    assert!(manager.has_session_vm("s1", "plugin-a"));
    assert!(
        !manager.has_session_vm("s1", "plugin-b"),
        "shadowed provider should not be started once a winner exists"
    );

    manager.end_session("s1").await.expect("end session");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ext_plugin_search_invoker_passthroughs_unsupported_backend_without_shadow_fallback() {
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
        .expect("unsupported_backend should be passed through for upper-layer classification");

    assert_eq!(result["unsupported_backend"], json!(true));
    assert!(manager.has_session_vm("s1", "plugin-a"));
    assert!(
        !manager.has_session_vm("s1", "plugin-b"),
        "shadowed provider should stay untouched after unsupported_backend"
    );

    manager.end_session("s1").await.expect("end session");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ext_plugin_search_invoker_returns_raw_unsupported_backend_when_only_provider_declines() {
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
    let result = search_invoker
        .search("mimo", json!({ "backend": "mimo", "query": "rust" }), "s1")
        .await
        .expect("unsupportedBackend payload should be returned for upper-layer classification");
    assert_eq!(result["unsupportedBackend"], json!(true));

    manager.end_session("s1").await.expect("end session");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ext_plugin_search_invoker_without_registered_provider_stays_incompatible() {
    let dispatcher = Arc::new(
        HostApiDispatcher::new(Arc::new(DefaultEventBus::new()))
            .with_tokio_handle(tokio::runtime::Handle::current()),
    );
    let (invoker, function_registry, manager, _dispatcher) =
        function_search_harness(dispatcher, Duration::from_secs(1));

    let search_invoker = ExtPluginSearchInvoker::new(function_registry, invoker);
    let err = search_invoker
        .search("mimo", json!({ "backend": "mimo", "query": "rust" }), "s1")
        .await
        .expect_err("missing provider registration should still be incompatible");

    match err {
        BackendFailure::Incompatible { detail } => {
            assert!(detail.contains("未找到名为 `mimo` 的 web_search 插件后端"));
        }
        other => panic!("unexpected error: {other:?}"),
    }

    manager.end_session("s1").await.expect("end session");
}
