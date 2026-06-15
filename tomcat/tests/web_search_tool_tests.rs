mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::json;
use serial_test::serial;
use tomcat::core::tools::web_search::types::WebSearchArgs;
use tomcat::core::tools::web_search::WebSearchRuntime;
use tomcat::{
    AppConfig, DefaultEventBus, DefaultLlmResolver, ExtPluginSearchInvoker, FunctionRegistry,
    HostApiDispatcher, ManifestFunction, ModelCatalog, PluginEngine, PluginFunctionInvoker,
    PluginManager, PluginRuntimeManager, TracingAuditRecorder,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
#[serial]
async fn runtime_explicit_tavily_works_from_public_api() {
    let tavily = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [
                {
                    "title": "Tokio",
                    "url": "https://tokio.rs",
                    "content": "Async runtime for Rust"
                }
            ]
        })))
        .expect(1)
        .mount(&tavily)
        .await;

    let _env = EnvGuard::set_many(&[
        ("TAVILY_API_KEY", Some("tavily-test-key")),
        ("BRAVE_API_KEY", None),
        ("SERPER_API_KEY", None),
        ("DEEPSEEK_API_KEY", None),
    ]);

    let mut cfg = AppConfig::default();
    cfg.tools.web_search.backend = "tavily".into();
    cfg.tools.web_search.tavily_base_url = tavily.uri();
    let runtime = build_runtime(cfg, None);

    let output = runtime
        .search(
            WebSearchArgs {
                query: "tokio rust".into(),
                count: Some(3),
                freshness: Some("week".into()),
                country: None,
                language: None,
                domain_filter: vec!["tokio.rs".into()],
            },
            "test-session",
        )
        .await
        .expect("tavily search");

    assert_eq!(output.backend, "tavily");
    assert_eq!(output.hits.len(), 1);
    assert_eq!(output.hits[0].url, "https://tokio.rs/");
}

#[tokio::test]
#[serial]
async fn runtime_auto_routes_to_http_fallback_chain() {
    let brave = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/res/v1/web/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "web": {
                "results": [
                    {
                        "title": "reqwest",
                        "url": "https://docs.rs/reqwest",
                        "description": "HTTP client"
                    }
                ]
            }
        })))
        .expect(1)
        .mount(&brave)
        .await;

    let _env = EnvGuard::set_many(&[
        ("TAVILY_API_KEY", None),
        ("BRAVE_API_KEY", Some("brave-test-key")),
        ("SERPER_API_KEY", None),
        ("DEEPSEEK_API_KEY", None),
    ]);

    let mut cfg = AppConfig::default();
    cfg.tools.web_search.backend = "auto".into();
    cfg.tools.web_search.brave_base_url = brave.uri();
    cfg.tools.web_search.cache_ttl_secs = 60;
    cfg.tools.web_search.cache_capacity = 8;
    let runtime = build_runtime(cfg, None);

    let output = runtime
        .search(
            WebSearchArgs {
                query: "reqwest rust".into(),
                count: None,
                freshness: None,
                country: None,
                language: None,
                domain_filter: vec!["docs.rs".into()],
            },
            "test-session",
        )
        .await
        .expect("auto search");

    assert_eq!(output.backend, "brave");
    assert_eq!(output.hits.len(), 1);
    assert!(output
        .warnings
        .iter()
        .any(|warning| warning == "backend_unavailable:tavily, fallback=brave"));
    assert!(output
        .warnings
        .iter()
        .any(|warning| warning == "brave_domain_filter_via_query_rewrite"));
}

#[tokio::test]
#[serial]
async fn runtime_explicit_serper_works_from_public_api() {
    let serper = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "organic": [
                {
                    "title": "Rust Book",
                    "link": "https://doc.rust-lang.org/book/",
                    "snippet": "The Rust Programming Language"
                }
            ]
        })))
        .expect(1)
        .mount(&serper)
        .await;

    let _env = EnvGuard::set_many(&[
        ("TAVILY_API_KEY", None),
        ("BRAVE_API_KEY", None),
        ("SERPER_API_KEY", Some("serper-test-key")),
        ("DEEPSEEK_API_KEY", None),
    ]);

    let mut cfg = AppConfig::default();
    cfg.tools.web_search.backend = "serper".into();
    cfg.tools.web_search.serper_base_url = serper.uri();
    let runtime = build_runtime(cfg, None);

    let output = runtime
        .search(
            WebSearchArgs {
                query: "rust book".into(),
                count: Some(5),
                freshness: Some("month".into()),
                country: Some("us".into()),
                language: Some("en".into()),
                domain_filter: vec!["doc.rust-lang.org".into()],
            },
            "test-session",
        )
        .await
        .expect("serper search");

    assert_eq!(output.backend, "serper");
    assert_eq!(output.hits.len(), 1);
    assert_eq!(output.hits[0].url, "https://doc.rust-lang.org/book/");
    assert!(output
        .warnings
        .iter()
        .any(|warning| warning == "serper_domain_filter_via_query_rewrite"));
}

#[tokio::test]
#[serial]
async fn runtime_explicit_openai_uses_project_hosted_candidate() {
    let hosted = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "output": [
                {
                    "type": "web_search_call",
                    "results": [
                        {
                            "title": "Rust",
                            "url": "https://www.rust-lang.org",
                            "snippet": "Language homepage"
                        }
                    ]
                }
            ]
        })))
        .expect(1)
        .mount(&hosted)
        .await;

    let _env = EnvGuard::set_many(&[
        ("TAVILY_API_KEY", None),
        ("BRAVE_API_KEY", None),
        ("SERPER_API_KEY", None),
        ("OPENAI_API_KEY", Some("openai-test-key")),
    ]);

    let mut cfg = AppConfig::default();
    cfg.tools.web_search.backend = "openai".into();
    let runtime = build_runtime(
        cfg,
        Some(format!(
            r#"
[[models]]
id = "gpt-5.4-web"
api = "openai-responses"
provider = "openai"
base_url = "{}"

[models.capabilities]
web_search = true
"#,
            hosted.uri()
        )),
    );

    let output = runtime
        .search(
            WebSearchArgs {
                query: "rust language".into(),
                count: Some(3),
                freshness: None,
                country: None,
                language: None,
                domain_filter: vec!["rust-lang.org".into()],
            },
            "test-session",
        )
        .await
        .expect("explicit hosted search");

    assert_eq!(output.backend, "openai");
    assert_eq!(output.hits.len(), 1);

    let requests = hosted.received_requests().await.expect("requests");
    let request = requests.last().expect("hosted request");
    let body: serde_json::Value = serde_json::from_slice(&request.body).expect("json body");
    assert_eq!(body["model"], json!("gpt-5.4-web"));
    assert_eq!(body["tools"][0]["type"], json!("web_search"));
}

#[tokio::test]
#[serial]
async fn live_tavily_search_smoke() {
    if std::env::var("PI_LIVE_WEB_SEARCH").ok().as_deref() != Some("1") {
        return;
    }

    let mut cfg = AppConfig::default();
    cfg.tools.web_search.backend = "tavily".into();
    let runtime = build_runtime(cfg, None);
    let output = runtime
        .search(
            WebSearchArgs {
                query: "rust async runtime".into(),
                count: Some(3),
                freshness: Some("month".into()),
                country: None,
                language: Some("en".into()),
                domain_filter: vec!["tokio.rs".into(), "docs.rs".into()],
            },
            "test-session",
        )
        .await
        .expect("live tavily search");

    assert_eq!(output.backend, "tavily");
    assert!(
        !output.hits.is_empty(),
        "expected at least one Tavily hit when PI_LIVE_WEB_SEARCH=1"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn real_mimo_web_search() {
    common::setup_logging();
    common::load_openai_test_env();
    if std::env::var("PI_LIVE_MIMO_WEB_SEARCH").ok().as_deref() != Some("1") {
        return;
    }
    require_env_var("MIMO_API_KEY", "real_mimo_web_search");

    let _env = EnvGuard::set_many(&[
        ("TAVILY_API_KEY", None),
        ("BRAVE_API_KEY", None),
        ("SERPER_API_KEY", None),
    ]);
    let mut cfg = AppConfig::default();
    cfg.tools.web_search.backend = "mimo".into();

    let temp = tempfile::tempdir().expect("tempdir");
    let models_toml_path = temp.path().join("models.toml");
    let mimo_model =
        std::env::var("TOMCAT_E2E_MIMO_MODEL").unwrap_or_else(|_| "mimo-v2.5-pro".to_string());
    let mimo_base_url = std::env::var("TOMCAT_E2E_MIMO_BASE_URL")
        .or_else(|_| std::env::var("PI_LIVE_MIMO_BASE_URL"))
        .unwrap_or_else(|_| "https://token-plan-cn.xiaomimimo.com".to_string());
    std::fs::write(
        &models_toml_path,
        format!(
            r#"
[[models]]
id = "{mimo_model}"
api = "openai"
provider = "mimo"
base_url = "{mimo_base_url}"

[models.capabilities]
tools = true
reasoning = true
"#
        ),
    )
    .expect("write live mimo models.toml");
    let catalog = Arc::new(
        ModelCatalog::load_from_path(&cfg, models_toml_path).expect("load live mimo catalog"),
    );
    let runtime = WebSearchRuntime::new(&cfg, catalog.clone()).expect("build runtime");

    let plugin_root = install_builtin_web_search_plugin(temp.path());
    let event_bus = Arc::new(DefaultEventBus::new());
    let mut manager = Arc::new(PluginManager::new(event_bus.clone()));
    let function_registry = Arc::new(FunctionRegistry::new());
    let inner = Arc::get_mut(&mut manager).expect("plugin manager should be uniquely owned");
    inner.set_plugin_engine(PluginEngine::global(None).expect("create quickjs engine"));
    inner.set_plugin_runtime_manager(Arc::new(PluginRuntimeManager::new()));
    inner.set_audit_recorder(Arc::new(TracingAuditRecorder));
    inner.set_function_registry(function_registry.clone());

    let invoker = PluginFunctionInvoker::new(Arc::downgrade(&manager));
    let dispatcher = Arc::new(
        HostApiDispatcher::new(event_bus.clone())
            .with_tokio_handle(tokio::runtime::Handle::current())
            .with_llm_resolver(Arc::new(DefaultLlmResolver::new(cfg.clone(), catalog))),
    );
    invoker.attach_dispatcher(Arc::downgrade(&dispatcher));
    manager.set_host_dispatcher(dispatcher);
    function_registry.register_plugin_functions(
        "tomcat.web-search-backends",
        &plugin_root,
        &[ManifestFunction {
            point: "web_search.backend".to_string(),
            function: "webSearchBackend".to_string(),
        }],
    );
    manager
        .load_plugin(&plugin_root)
        .expect("load builtin web_search plugin");
    runtime.set_plugin_invoker(ExtPluginSearchInvoker::new(function_registry, invoker));

    let output = runtime
        .search(
            WebSearchArgs {
                query: "Rust reqwest async".into(),
                count: Some(5),
                freshness: Some("month".into()),
                country: Some("us".into()),
                language: Some("en".into()),
                domain_filter: Vec::new(),
            },
            "live-mimo-session",
        )
        .await
        .expect("live mimo plugin search");

    assert_eq!(output.backend, "mimo");
    assert!(
        !output.hits.is_empty(),
        "expected MiMo annotations to map into at least one hit, warnings={:?}",
        output.warnings
    );
    assert!(
        output.hits.iter().all(|hit| hit.url.starts_with("http")),
        "all mapped hits should contain URLs"
    );

    manager
        .end_session("live-mimo-session")
        .await
        .expect("end live mimo session");
}

fn require_env_var(env_key: &str, test_name: &str) -> String {
    std::env::var(env_key)
        .unwrap_or_else(|_| panic!("{test_name} 必须设置 {env_key}（环境变量或 tomcat/.env）"))
}

fn build_runtime(config: AppConfig, models_toml: Option<String>) -> WebSearchRuntime {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("models.toml");
    if let Some(contents) = models_toml {
        std::fs::write(&path, contents).expect("write models.toml");
    }
    let catalog =
        Arc::new(ModelCatalog::load_from_path(&config, path).expect("load model catalog"));
    WebSearchRuntime::new(&config, catalog).expect("build runtime")
}

fn install_builtin_web_search_plugin(dest_root: &Path) -> PathBuf {
    let src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join("plugins")
        .join("web-search-backends");
    let dst = dest_root.join("web-search-backends");
    std::fs::create_dir_all(&dst).expect("create builtin web_search plugin dir");
    for name in ["plugin.json", "main.js", "README.md"] {
        std::fs::copy(src.join(name), dst.join(name)).expect("copy builtin web_search asset");
    }
    dst
}

struct EnvGuard {
    saved: Vec<(String, Option<String>)>,
}

impl EnvGuard {
    fn set_many(entries: &[(&str, Option<&str>)]) -> Self {
        let mut saved = Vec::new();
        for (key, value) in entries {
            saved.push(((*key).to_string(), std::env::var(key).ok()));
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
        Self { saved }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in self.saved.drain(..) {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}
