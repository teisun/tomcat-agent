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
    PluginManager, PluginRuntimeManager, SessionMode, TracingAuditRecorder,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
#[serial]
async fn runtime_explicit_tavily_works_from_public_api() {
    let tavily = common::HttpsTestServer::start(
        "api.tavily.com",
        "200 OK",
        vec![("Content-Type".to_string(), "application/json".to_string())],
        serde_json::to_vec(&json!({
            "results": [
                {
                    "title": "Tokio",
                    "url": "https://tokio.rs",
                    "content": "Async runtime for Rust"
                }
            ]
        }))
        .expect("serialize tavily response"),
        std::time::Duration::ZERO,
    )
    .await;

    let _env = EnvGuard::set_many(&[
        ("TAVILY_API_KEY", Some("tavily-test-key")),
        ("BRAVE_API_KEY", None),
        ("SERPER_API_KEY", None),
        ("DEEPSEEK_API_KEY", None),
    ]);

    let mut cfg = AppConfig::default();
    cfg.tools.web_search.backend = "tavily".into();
    let harness = build_runtime_with_builtin_plugin_and_fetch_client(
        cfg,
        None,
        Some(tavily.client_for("api.tavily.com", std::time::Duration::from_secs(2))),
    );

    let output = harness
        .runtime
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
    harness
        .manager
        .end_session("test-session")
        .await
        .expect("end tavily plugin test session");
}

#[tokio::test]
#[serial]
async fn runtime_auto_routes_to_plugin_backends_after_retryable_failures() {
    common::setup_logging();
    let brave = common::HttpsTestServer::start(
        "api.search.brave.com",
        "200 OK",
        vec![("Content-Type".to_string(), "application/json".to_string())],
        serde_json::to_vec(&json!({
            "web": {
                "results": [
                    {
                        "title": "reqwest",
                        "url": "https://docs.rs/reqwest",
                        "description": "HTTP client"
                    }
                ]
            }
        }))
        .expect("serialize brave response"),
        std::time::Duration::ZERO,
    )
    .await;

    let _env = EnvGuard::set_many(&[
        ("TAVILY_API_KEY", None),
        ("BRAVE_API_KEY", Some("brave-test-key")),
        ("SERPER_API_KEY", None),
        ("DEEPSEEK_API_KEY", None),
    ]);

    let mut cfg = AppConfig::default();
    cfg.tools.web_search.backend = "auto".into();
    cfg.tools.web_search.cache_ttl_secs = 60;
    cfg.tools.web_search.cache_capacity = 8;
    let harness = build_runtime_with_builtin_plugin_without_mimo_and_fetch_client(
        cfg,
        None,
        Some(brave.client_for("api.search.brave.com", std::time::Duration::from_secs(2))),
    );

    let search_result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        harness.runtime.search(
            WebSearchArgs {
                query: "reqwest rust".into(),
                count: None,
                freshness: None,
                country: None,
                language: None,
                domain_filter: vec!["docs.rs".into()],
            },
            "test-session",
        ),
    )
    .await
    .expect("auto retryable-fallback search should not hang");
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        harness.manager.end_session("test-session"),
    )
    .await
    .expect("auto retryable-fallback teardown should not hang")
    .expect("end auto plugin test session");
    let output = search_result.expect("auto search");

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
async fn runtime_auto_builtin_plugin_runtime_error_fails_loud() {
    let _env = EnvGuard::set_many(&[
        ("MIMO_API_KEY", Some("mimo-test-key")),
        ("TAVILY_API_KEY", None),
        ("BRAVE_API_KEY", None),
        ("SERPER_API_KEY", None),
        ("DEEPSEEK_API_KEY", None),
        ("OPENAI_API_KEY", None),
    ]);

    let mut cfg = AppConfig::default();
    cfg.tools.web_search.backend = "auto".into();
    let harness = build_runtime_with_builtin_plugin(cfg, None);

    let search_result = harness
        .runtime
        .search(
            WebSearchArgs {
                query: "reqwest rust".into(),
                count: Some(3),
                freshness: None,
                country: None,
                language: None,
                domain_filter: Vec::new(),
            },
            "runtime-pluginruntime-1",
        )
        .await;
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        harness.manager.end_session("runtime-pluginruntime-1"),
    )
    .await
    .expect("plugin runtime failure teardown should not hang")
    .expect("end plugin runtime failure session");
    let err = search_result.expect_err("unexpected plugin runtime errors should fail loud");
    let text = err.to_string();
    assert!(text.contains("web_search backend `auto` 运行时错误"));
    assert!(text.contains("plugin_backend_error (backend=mimo)"));
    assert!(
        !text.contains("所有后端均不可用"),
        "plugin runtime failures should not be flattened into all_backends_unavailable: {text}"
    );
}

#[tokio::test]
#[serial]
async fn runtime_explicit_brave_works_from_public_api() {
    let brave = common::HttpsTestServer::start(
        "api.search.brave.com",
        "200 OK",
        vec![("Content-Type".to_string(), "application/json".to_string())],
        serde_json::to_vec(&json!({
            "web": {
                "results": [
                    {
                        "title": "Reqwest",
                        "url": "https://docs.rs/reqwest",
                        "description": "HTTP client"
                    }
                ]
            }
        }))
        .expect("serialize brave response"),
        std::time::Duration::ZERO,
    )
    .await;

    let _env = EnvGuard::set_many(&[
        ("TAVILY_API_KEY", None),
        ("BRAVE_API_KEY", Some("brave-test-key")),
        ("SERPER_API_KEY", None),
        ("DEEPSEEK_API_KEY", None),
    ]);

    let mut cfg = AppConfig::default();
    cfg.tools.web_search.backend = "brave".into();
    let harness = build_runtime_with_builtin_plugin_and_fetch_client(
        cfg,
        None,
        Some(brave.client_for("api.search.brave.com", std::time::Duration::from_secs(2))),
    );

    let output = harness
        .runtime
        .search(
            WebSearchArgs {
                query: "reqwest rust".into(),
                count: Some(3),
                freshness: Some("week".into()),
                country: Some("us".into()),
                language: Some("en".into()),
                domain_filter: vec!["docs.rs".into()],
            },
            "test-session",
        )
        .await
        .expect("brave search");

    assert_eq!(output.backend, "brave");
    assert_eq!(output.hits.len(), 1);
    assert_eq!(output.hits[0].url, "https://docs.rs/reqwest");
    assert!(output
        .warnings
        .iter()
        .any(|warning| warning == "brave_domain_filter_via_query_rewrite"));
    harness
        .manager
        .end_session("test-session")
        .await
        .expect("end brave plugin test session");
}

#[tokio::test]
#[serial]
async fn runtime_explicit_serper_works_from_public_api() {
    let serper = common::HttpsTestServer::start(
        "google.serper.dev",
        "200 OK",
        vec![("Content-Type".to_string(), "application/json".to_string())],
        serde_json::to_vec(&json!({
            "organic": [
                {
                    "title": "Rust Book",
                    "link": "https://doc.rust-lang.org/book/",
                    "snippet": "The Rust Programming Language"
                }
            ]
        }))
        .expect("serialize serper response"),
        std::time::Duration::ZERO,
    )
    .await;

    let _env = EnvGuard::set_many(&[
        ("TAVILY_API_KEY", None),
        ("BRAVE_API_KEY", None),
        ("SERPER_API_KEY", Some("serper-test-key")),
        ("DEEPSEEK_API_KEY", None),
    ]);

    let mut cfg = AppConfig::default();
    cfg.tools.web_search.backend = "serper".into();
    let harness = build_runtime_with_builtin_plugin_and_fetch_client(
        cfg,
        None,
        Some(serper.client_for("google.serper.dev", std::time::Duration::from_secs(2))),
    );

    let output = harness
        .runtime
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
    harness
        .manager
        .end_session("test-session")
        .await
        .expect("end serper plugin test session");
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
async fn runtime_explicit_openai_uses_llm_proxy_and_trims_whitespace() {
    let hosted = common::HttpsTestServer::start(
        "api.openai.com",
        "200 OK",
        vec![("Content-Type".to_string(), "application/json".to_string())],
        hosted_web_search_response(),
        std::time::Duration::ZERO,
    )
    .await;
    let proxy =
        common::ProxyTestServer::start(vec![("api.openai.com".to_string(), hosted.addr())]).await;
    let proxy_url = proxy.url();

    let _env = EnvGuard::set_many(&[
        ("TAVILY_API_KEY", None),
        ("BRAVE_API_KEY", None),
        ("SERPER_API_KEY", None),
        ("OPENAI_API_KEY", Some("openai-test-key")),
        ("HTTPS_PROXY", None),
        ("HTTP_PROXY", None),
        ("ALL_PROXY", None),
        ("NO_PROXY", None),
        ("no_proxy", None),
    ]);

    let mut cfg = AppConfig::default();
    cfg.tools.web_search.backend = "openai".into();
    cfg.llm.proxy = Some(format!("{proxy_url} "));
    let runtime = build_runtime(
        cfg,
        Some(hosted_openai_models_toml("https://api.openai.com")),
    );

    let err = runtime
        .search(
            WebSearchArgs {
                query: "rust language".into(),
                count: Some(3),
                freshness: None,
                country: None,
                language: None,
                domain_filter: vec!["rust-lang.org".into()],
            },
            "hosted-proxy-session",
        )
        .await
        .expect_err("proxy path should be exercised before TLS trust fails");

    assert!(err.to_string().contains("web_search backend `openai`"));
    assert!(proxy.saw_host("api.openai.com"));
}

#[tokio::test]
#[serial]
async fn runtime_injected_fetch_client_bypasses_env_proxy() {
    let tavily = common::HttpsTestServer::start(
        "api.tavily.com",
        "200 OK",
        vec![("Content-Type".to_string(), "application/json".to_string())],
        serde_json::to_vec(&json!({
            "results": [
                {
                    "title": "Tokio",
                    "url": "https://tokio.rs",
                    "content": "Async runtime for Rust"
                }
            ]
        }))
        .expect("serialize tavily response"),
        std::time::Duration::ZERO,
    )
    .await;
    let proxy =
        common::ProxyTestServer::start(vec![("api.tavily.com".to_string(), tavily.addr())]).await;
    let proxy_url = proxy.url();

    let _env = EnvGuard::set_many(&[
        ("TAVILY_API_KEY", Some("tavily-test-key")),
        ("BRAVE_API_KEY", None),
        ("SERPER_API_KEY", None),
        ("DEEPSEEK_API_KEY", None),
        ("HTTPS_PROXY", Some(proxy_url.as_str())),
        ("HTTP_PROXY", None),
        ("ALL_PROXY", None),
        ("NO_PROXY", None),
        ("no_proxy", None),
    ]);

    let mut cfg = AppConfig::default();
    cfg.tools.web_search.backend = "tavily".into();
    let harness = build_runtime_with_builtin_plugin_and_fetch_client(
        cfg,
        None,
        Some(tavily.client_for("api.tavily.com", std::time::Duration::from_secs(2))),
    );

    let output = harness
        .runtime
        .search(
            WebSearchArgs {
                query: "tokio rust".into(),
                count: Some(3),
                freshness: None,
                country: None,
                language: None,
                domain_filter: vec!["tokio.rs".into()],
            },
            "injected-fetch-client-session",
        )
        .await
        .expect("tavily search");

    assert_eq!(output.backend, "tavily");
    assert_eq!(output.hits.len(), 1);
    assert!(
        !proxy.saw_host("api.tavily.com"),
        "injected loopback client should bypass ambient proxy"
    );
    harness
        .manager
        .end_session("injected-fetch-client-session")
        .await
        .expect("end injected fetch client test session");
}

#[tokio::test]
#[serial]
async fn production_path_explicit_tavily_uses_env_https_proxy() {
    let tavily = common::HttpsTestServer::start(
        "api.tavily.com",
        "200 OK",
        vec![("Content-Type".to_string(), "application/json".to_string())],
        serde_json::to_vec(&json!({
            "results": [
                {
                    "title": "Tokio",
                    "url": "https://tokio.rs",
                    "content": "Async runtime for Rust"
                }
            ]
        }))
        .expect("serialize tavily response"),
        std::time::Duration::ZERO,
    )
    .await;
    let proxy =
        common::ProxyTestServer::start(vec![("api.tavily.com".to_string(), tavily.addr())]).await;
    let proxy_url = proxy.url();

    const ENV_KEY: &str = "TOMCAT_WEB_SEARCH_PROD_PROXY_KEY";
    let _env = EnvGuard::set_many(&[
        (ENV_KEY, Some("stub")),
        ("TAVILY_API_KEY", Some("tavily-test-key")),
        ("BRAVE_API_KEY", None),
        ("SERPER_API_KEY", None),
        ("HTTPS_PROXY", Some(proxy_url.as_str())),
        ("HTTP_PROXY", None),
        ("ALL_PROXY", None),
        ("NO_PROXY", None),
        ("no_proxy", None),
    ]);

    let mut cfg = AppConfig::default();
    cfg.tools.web_search.backend = "tavily".into();
    let harness = build_production_web_search_harness(cfg, ENV_KEY, None);
    let session_id = current_session_id(harness.ctx());
    let err = harness
        .ctx()
        .global_services
        .web_search_runtime
        .search(
            WebSearchArgs {
                query: "tokio rust".into(),
                count: Some(3),
                freshness: None,
                country: None,
                language: None,
                domain_filter: vec!["tokio.rs".into()],
            },
            &session_id,
        )
        .await
        .expect_err("proxy path should be exercised before TLS trust fails");

    end_current_plugin_session(harness.ctx()).await;
    assert!(err.to_string().contains("tavily"));
    assert!(proxy.saw_host("api.tavily.com"));
}

#[tokio::test]
#[serial]
async fn runtime_session_vm_survives_idle_beyond_call_timeout() {
    common::setup_logging();
    let tavily = common::HttpsTestServer::start(
        "api.tavily.com",
        "200 OK",
        vec![("Content-Type".to_string(), "application/json".to_string())],
        serde_json::to_vec(&json!({
            "results": [
                {
                    "title": "Tokio",
                    "url": "https://tokio.rs",
                    "content": "Async runtime for Rust"
                }
            ]
        }))
        .expect("serialize tavily response"),
        std::time::Duration::ZERO,
    )
    .await;

    let _env = EnvGuard::set_many(&[
        ("TAVILY_API_KEY", Some("tavily-test-key")),
        ("BRAVE_API_KEY", None),
        ("SERPER_API_KEY", None),
        ("DEEPSEEK_API_KEY", None),
        ("OPENAI_API_KEY", None),
    ]);

    let mut cfg = AppConfig::default();
    cfg.tools.web_search.backend = "tavily".into();
    cfg.plugin.call_timeout_ms = 200;
    cfg.plugin.interrupt_budget = 0;
    let harness = build_runtime_with_builtin_plugin_and_fetch_client(
        cfg,
        None,
        Some(tavily.client_for("api.tavily.com", std::time::Duration::from_secs(5))),
    );

    let first_result = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        harness.runtime.search(
            WebSearchArgs {
                query: "tokio rust".into(),
                count: Some(3),
                freshness: None,
                country: None,
                language: None,
                domain_filter: vec!["tokio.rs".into()],
            },
            "idle-web-search-session",
        ),
    )
    .await
    .expect("first idle search should not hang");
    let first = first_result.expect("first search should succeed");
    assert_eq!(first.backend, "tavily");

    tokio::time::sleep(std::time::Duration::from_millis(350)).await;

    let second_result = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        harness.runtime.search(
            WebSearchArgs {
                query: "tokio runtime".into(),
                count: Some(3),
                freshness: None,
                country: None,
                language: None,
                domain_filter: vec!["tokio.rs".into()],
            },
            "idle-web-search-session",
        ),
    )
    .await
    .expect("second idle search should not hang");
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        harness.manager.end_session("idle-web-search-session"),
    )
    .await
    .expect("idle session teardown should not hang")
    .expect("end idle web search session");
    let second = second_result.expect("second search should still succeed after idle timeout budget passes");
    assert_eq!(second.backend, "tavily");
    assert!(
        !second.hits.is_empty(),
        "second search should prove session VM was not poisoned by idle timeout"
    );
}

#[test]
#[serial]
fn production_ordering_explicit_tavily_reaches_proxy_without_tokio_handle_error() {
    let driver_rt = tokio::runtime::Runtime::new().expect("create async driver");
    let tavily = driver_rt.block_on(common::HttpsTestServer::start(
        "api.tavily.com",
        "200 OK",
        vec![("Content-Type".to_string(), "application/json".to_string())],
        serde_json::to_vec(&json!({
            "results": [
                {
                    "title": "Tokio",
                    "url": "https://tokio.rs",
                    "content": "Async runtime for Rust"
                }
            ]
        }))
        .expect("serialize tavily response"),
        std::time::Duration::ZERO,
    ));
    let proxy = driver_rt.block_on(common::ProxyTestServer::start(vec![(
        "api.tavily.com".to_string(),
        tavily.addr(),
    )]));
    let proxy_url = proxy.url();

    const ENV_KEY: &str = "TOMCAT_WEB_SEARCH_PROD_ORDERING_KEY";
    let _env = EnvGuard::set_many(&[
        (ENV_KEY, Some("stub")),
        ("TAVILY_API_KEY", Some("tavily-test-key")),
        ("BRAVE_API_KEY", None),
        ("SERPER_API_KEY", None),
        ("HTTPS_PROXY", Some(proxy_url.as_str())),
        ("HTTP_PROXY", None),
        ("ALL_PROXY", None),
        ("NO_PROXY", None),
        ("no_proxy", None),
    ]);

    let mut cfg = AppConfig::default();
    cfg.tools.web_search.backend = "tavily".into();
    let harness = build_production_web_search_harness(cfg, ENV_KEY, None);
    let session_id = current_session_id(harness.ctx());
    let err = driver_rt
        .block_on(async {
            harness
                .ctx()
                .global_services
                .web_search_runtime
                .search(
                    WebSearchArgs {
                        query: "tokio rust".into(),
                        count: Some(3),
                        freshness: None,
                        country: None,
                        language: None,
                        domain_filter: vec!["tokio.rs".into()],
                    },
                    &session_id,
                )
                .await
        })
        .expect_err("proxy path should be exercised before TLS trust fails");

    driver_rt.block_on(async {
        end_current_plugin_session(harness.ctx()).await;
    });

    let text = err.to_string();
    assert!(
        !text.contains("async hostcall requires a Tokio runtime handle"),
        "production-ordering regression should not surface handle-capture bug anymore: {text}"
    );
    assert!(text.contains("tavily"));
    assert!(proxy.saw_host("api.tavily.com"));
}

#[tokio::test]
#[serial]
async fn production_path_llm_proxy_overrides_env_proxy_for_plugin_fetch() {
    let tavily = common::HttpsTestServer::start(
        "api.tavily.com",
        "200 OK",
        vec![("Content-Type".to_string(), "application/json".to_string())],
        serde_json::to_vec(&json!({
            "results": [
                {
                    "title": "Tokio",
                    "url": "https://tokio.rs",
                    "content": "Async runtime for Rust"
                }
            ]
        }))
        .expect("serialize tavily response"),
        std::time::Duration::ZERO,
    )
    .await;
    let env_proxy =
        common::ProxyTestServer::start(vec![("api.tavily.com".to_string(), tavily.addr())]).await;
    let cfg_proxy =
        common::ProxyTestServer::start(vec![("api.tavily.com".to_string(), tavily.addr())]).await;
    let env_proxy_url = env_proxy.url();
    let cfg_proxy_url = cfg_proxy.url();

    const ENV_KEY: &str = "TOMCAT_WEB_SEARCH_CFG_PROXY_KEY";
    let _env = EnvGuard::set_many(&[
        (ENV_KEY, Some("stub")),
        ("TAVILY_API_KEY", Some("tavily-test-key")),
        ("BRAVE_API_KEY", None),
        ("SERPER_API_KEY", None),
        ("HTTPS_PROXY", Some(env_proxy_url.as_str())),
        ("HTTP_PROXY", None),
        ("ALL_PROXY", None),
        ("NO_PROXY", None),
        ("no_proxy", None),
    ]);

    let mut cfg = AppConfig::default();
    cfg.tools.web_search.backend = "tavily".into();
    cfg.llm.proxy = Some(format!("{cfg_proxy_url} "));
    let harness = build_production_web_search_harness(cfg, ENV_KEY, None);
    let session_id = current_session_id(harness.ctx());
    let err = harness
        .ctx()
        .global_services
        .web_search_runtime
        .search(
            WebSearchArgs {
                query: "tokio rust".into(),
                count: Some(3),
                freshness: None,
                country: None,
                language: None,
                domain_filter: vec!["tokio.rs".into()],
            },
            &session_id,
        )
        .await
        .expect_err("cfg proxy path should be exercised before TLS trust fails");

    end_current_plugin_session(harness.ctx()).await;
    assert!(err.to_string().contains("tavily"));
    assert!(cfg_proxy.saw_host("api.tavily.com"));
    assert!(
        !env_proxy.saw_host("api.tavily.com"),
        "explicit llm.proxy should override ambient HTTPS_PROXY"
    );
}

#[tokio::test]
#[serial]
async fn production_path_plugin_timeout_returns_tool_error_before_vm_timeout() {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind hanging tcp listener");
    let hang_addr = listener.local_addr().expect("hang listener addr");
    let hang_task = tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            let _stream = stream;
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    });
    let proxy =
        common::ProxyTestServer::start(vec![("api.tavily.com".to_string(), hang_addr)]).await;
    let proxy_url = proxy.url();

    const ENV_KEY: &str = "TOMCAT_WEB_SEARCH_TIMEOUT_KEY";
    let _env = EnvGuard::set_many(&[
        (ENV_KEY, Some("stub")),
        ("TAVILY_API_KEY", Some("tavily-test-key")),
        ("BRAVE_API_KEY", None),
        ("SERPER_API_KEY", None),
        ("HTTPS_PROXY", Some(proxy_url.as_str())),
        ("HTTP_PROXY", None),
        ("ALL_PROXY", None),
        ("NO_PROXY", None),
        ("no_proxy", None),
    ]);

    let mut cfg = AppConfig::default();
    cfg.tools.web_search.backend = "tavily".into();
    cfg.tools.web_fetch.fetch_timeout_ms = 60_000;
    cfg.plugin.call_timeout_ms = 1_500;
    let harness = build_production_web_search_harness(cfg, ENV_KEY, None);
    let session_id = current_session_id(harness.ctx());
    let start = std::time::Instant::now();
    let err = harness
        .ctx()
        .global_services
        .web_search_runtime
        .search(
            WebSearchArgs {
                query: "tokio rust".into(),
                count: Some(3),
                freshness: None,
                country: None,
                language: None,
                domain_filter: vec!["tokio.rs".into()],
            },
            &session_id,
        )
        .await
        .expect_err("timeout should surface as tool error");
    hang_task.abort();

    end_current_plugin_session(harness.ctx()).await;
    assert!(
        start.elapsed() < std::time::Duration::from_millis(2_000),
        "client timeout should fire before plugin VM hard timeout, elapsed={:?}",
        start.elapsed()
    );
    assert!(
        err.to_string().contains("请求超时"),
        "expected structured timeout, got: {err}"
    );
    assert!(proxy.saw_host("api.tavily.com"));
}

#[tokio::test]
#[serial]
async fn runtime_auto_timeout_falls_back_to_brave_after_tavily_timeout() {
    common::setup_logging();
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind hanging tcp listener");
    let hang_addr = listener.local_addr().expect("hang listener addr");
    let hang_task = tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            let _stream = stream;
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    });
    let brave = common::HttpsTestServer::start(
        "api.search.brave.com",
        "200 OK",
        vec![("Content-Type".to_string(), "application/json".to_string())],
        serde_json::to_vec(&json!({
            "web": {
                "results": [
                    {
                        "title": "reqwest",
                        "url": "https://docs.rs/reqwest",
                        "description": "HTTP client"
                    }
                ]
            }
        }))
        .expect("serialize brave response"),
        std::time::Duration::ZERO,
    )
    .await;
    const ENV_KEY: &str = "TOMCAT_WEB_SEARCH_AUTO_TIMEOUT_KEY";
    let _env = EnvGuard::set_many(&[
        (ENV_KEY, Some("stub")),
        ("MIMO_API_KEY", None),
        ("TAVILY_API_KEY", Some("tavily-test-key")),
        ("BRAVE_API_KEY", Some("brave-test-key")),
        ("SERPER_API_KEY", None),
        ("DEEPSEEK_API_KEY", None),
        ("HTTPS_PROXY", None),
        ("HTTP_PROXY", None),
        ("ALL_PROXY", None),
        ("NO_PROXY", None),
        ("no_proxy", None),
    ]);

    let mut cfg = AppConfig::default();
    cfg.tools.web_search.backend = "auto".into();
    cfg.tools.web_fetch.fetch_timeout_ms = 2_000;
    cfg.plugin.call_timeout_ms = 1_500;
    let fetch_client = reqwest::Client::builder()
        .no_proxy()
        .danger_accept_invalid_certs(true)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_millis(2_000))
        .resolve("api.tavily.com", hang_addr)
        .resolve("api.search.brave.com", brave.addr())
        .build()
        .expect("build timeout fallback client");
    let harness =
        build_runtime_with_builtin_plugin_without_mimo_and_fetch_client(cfg, None, Some(fetch_client));
    let start = std::time::Instant::now();
    let search_result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        harness.runtime.search(
            WebSearchArgs {
                query: "reqwest rust".into(),
                count: Some(3),
                freshness: None,
                country: None,
                language: None,
                domain_filter: vec!["docs.rs".into()],
            },
            "timeout-fallback-session",
        ),
    )
    .await
    .expect("auto timeout fallback search should not hang");
    hang_task.abort();

    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        harness.manager.end_session("timeout-fallback-session"),
    )
    .await
    .expect("auto timeout fallback teardown should not hang")
    .expect("end timeout fallback session");
    let output = search_result.expect("auto search should fall back to brave after tavily timeout");
    assert!(
        start.elapsed() < std::time::Duration::from_millis(5_000),
        "timeout fallback should finish after a structured timeout and fallback, elapsed={:?}",
        start.elapsed()
    );
    assert_eq!(output.backend, "brave");
    assert_eq!(output.hits.len(), 1);
    assert!(output
        .warnings
        .iter()
        .any(|warning| warning.contains("tavily") && warning.contains("fallback=brave")));
}

#[tokio::test]
#[serial]
async fn production_path_explicit_openai_uses_env_https_proxy() {
    let hosted = common::HttpsTestServer::start(
        "api.openai.com",
        "200 OK",
        vec![("Content-Type".to_string(), "application/json".to_string())],
        hosted_web_search_response(),
        std::time::Duration::ZERO,
    )
    .await;
    let proxy =
        common::ProxyTestServer::start(vec![("api.openai.com".to_string(), hosted.addr())]).await;
    let proxy_url = proxy.url();

    const ENV_KEY: &str = "TOMCAT_WEB_SEARCH_HOSTED_PROXY_KEY";
    let _env = EnvGuard::set_many(&[
        (ENV_KEY, Some("stub")),
        ("OPENAI_API_KEY", Some("openai-test-key")),
        ("TAVILY_API_KEY", None),
        ("BRAVE_API_KEY", None),
        ("SERPER_API_KEY", None),
        ("HTTPS_PROXY", Some(proxy_url.as_str())),
        ("HTTP_PROXY", None),
        ("ALL_PROXY", None),
        ("NO_PROXY", None),
        ("no_proxy", None),
    ]);

    let mut cfg = AppConfig::default();
    cfg.tools.web_search.backend = "openai".into();
    let harness = build_production_web_search_harness(
        cfg,
        ENV_KEY,
        Some(hosted_openai_models_toml("https://api.openai.com")),
    );
    let session_id = current_session_id(harness.ctx());
    let err = harness
        .ctx()
        .global_services
        .web_search_runtime
        .search(
            WebSearchArgs {
                query: "rust language".into(),
                count: Some(3),
                freshness: None,
                country: None,
                language: None,
                domain_filter: vec!["rust-lang.org".into()],
            },
            &session_id,
        )
        .await
        .expect_err("proxy path should be exercised before TLS trust fails");

    end_current_plugin_session(harness.ctx()).await;
    assert!(err.to_string().contains("web_search backend `openai`"));
    assert!(proxy.saw_host("api.openai.com"));
}

#[tokio::test]
#[serial]
async fn production_path_hosted_openai_direct_without_proxy_succeeds() {
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

    const ENV_KEY: &str = "TOMCAT_WEB_SEARCH_HOSTED_DIRECT_KEY";
    let _env = EnvGuard::set_many(&[
        (ENV_KEY, Some("stub")),
        ("OPENAI_API_KEY", Some("openai-test-key")),
        ("TAVILY_API_KEY", None),
        ("BRAVE_API_KEY", None),
        ("SERPER_API_KEY", None),
        ("HTTPS_PROXY", None),
        ("HTTP_PROXY", None),
        ("ALL_PROXY", None),
        ("NO_PROXY", None),
        ("no_proxy", None),
    ]);

    let mut cfg = AppConfig::default();
    cfg.tools.web_search.backend = "openai".into();
    let harness = build_production_web_search_harness(
        cfg,
        ENV_KEY,
        Some(hosted_openai_models_toml(&hosted.uri())),
    );
    let session_id = current_session_id(harness.ctx());
    let output = harness
        .ctx()
        .global_services
        .web_search_runtime
        .search(
            WebSearchArgs {
                query: "rust language".into(),
                count: Some(3),
                freshness: None,
                country: None,
                language: None,
                domain_filter: vec!["rust-lang.org".into()],
            },
            &session_id,
        )
        .await
        .expect("direct hosted search should succeed");

    end_current_plugin_session(harness.ctx()).await;
    assert_eq!(output.backend, "openai");
    assert_eq!(output.hits.len(), 1);
}

#[tokio::test]
#[serial]
async fn production_path_explicit_serper_uses_env_https_proxy() {
    let serper = common::HttpsTestServer::start(
        "google.serper.dev",
        "200 OK",
        vec![("Content-Type".to_string(), "application/json".to_string())],
        serde_json::to_vec(&json!({
            "organic": [
                {
                    "title": "Rust Book",
                    "link": "https://doc.rust-lang.org/book/",
                    "snippet": "The Rust Programming Language"
                }
            ]
        }))
        .expect("serialize serper response"),
        std::time::Duration::ZERO,
    )
    .await;
    let proxy =
        common::ProxyTestServer::start(vec![("google.serper.dev".to_string(), serper.addr())])
            .await;
    let proxy_url = proxy.url();

    const ENV_KEY: &str = "TOMCAT_WEB_SEARCH_SERPER_PROXY_KEY";
    let _env = EnvGuard::set_many(&[
        (ENV_KEY, Some("stub")),
        ("OPENAI_API_KEY", Some("openai-test-key")),
        ("TAVILY_API_KEY", None),
        ("BRAVE_API_KEY", None),
        ("SERPER_API_KEY", Some("serper-test-key")),
        ("HTTPS_PROXY", Some(proxy_url.as_str())),
        ("HTTP_PROXY", None),
        ("ALL_PROXY", None),
        ("NO_PROXY", None),
        ("no_proxy", None),
    ]);

    let mut cfg = AppConfig::default();
    cfg.tools.web_search.backend = "serper".into();
    let harness = build_production_web_search_harness(cfg, ENV_KEY, None);
    let session_id = current_session_id(harness.ctx());
    let err = harness
        .ctx()
        .global_services
        .web_search_runtime
        .search(
            WebSearchArgs {
                query: "rust programming language".into(),
                count: Some(5),
                freshness: None,
                country: Some("us".into()),
                language: Some("en".into()),
                domain_filter: Vec::new(),
            },
            &session_id,
        )
        .await
        .expect_err("proxy path should be exercised before TLS trust fails");

    end_current_plugin_session(harness.ctx()).await;
    assert!(err.to_string().contains("serper"));
    assert!(proxy.saw_host("google.serper.dev"));
}

#[tokio::test]
#[serial]
async fn live_tavily_search_smoke() {
    if std::env::var("PI_LIVE_WEB_SEARCH").ok().as_deref() != Some("1") {
        return;
    }

    let mut cfg = AppConfig::default();
    cfg.tools.web_search.backend = "tavily".into();
    let harness = build_runtime_with_builtin_plugin(cfg, None);
    let output = harness
        .runtime
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
    harness
        .manager
        .end_session("test-session")
        .await
        .expect("end live tavily session");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn real_tavily_plugin_web_search() {
    common::setup_logging();
    common::load_openai_test_env();
    if !require_env_var_or_skip("TAVILY_API_KEY", "real_tavily_plugin_web_search") {
        return;
    }
    const ENV_KEY: &str = "TOMCAT_REAL_TAVILY_PROD_CLIENT_KEY";
    let _env = EnvGuard::set_many(&[(ENV_KEY, Some("stub"))]);

    let mut cfg = AppConfig::default();
    cfg.tools.web_search.backend = "tavily".into();
    let harness = build_production_web_search_harness(cfg, ENV_KEY, None);
    let session_id = current_session_id(harness.ctx());
    let output = harness
        .ctx()
        .global_services
        .web_search_runtime
        .search(
            WebSearchArgs {
                query: "rust async runtime".into(),
                count: Some(3),
                freshness: Some("month".into()),
                country: Some("us".into()),
                language: Some("en".into()),
                domain_filter: vec!["tokio.rs".into(), "docs.rs".into()],
            },
            &session_id,
        )
        .await
        .expect("live tavily plugin search");

    end_current_plugin_session(harness.ctx()).await;
    assert_eq!(output.backend, "tavily");
    assert!(
        !output.hits.is_empty(),
        "expected Tavily plugin backend to return hits, warnings={:?}",
        output.warnings
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn real_brave_plugin_web_search() {
    common::setup_logging();
    common::load_openai_test_env();
    if !require_env_var_or_skip("BRAVE_API_KEY", "real_brave_plugin_web_search") {
        return;
    }
    const ENV_KEY: &str = "TOMCAT_REAL_BRAVE_PROD_CLIENT_KEY";
    let _env = EnvGuard::set_many(&[(ENV_KEY, Some("stub"))]);

    let mut cfg = AppConfig::default();
    cfg.tools.web_search.backend = "brave".into();
    let harness = build_production_web_search_harness(cfg, ENV_KEY, None);
    let session_id = current_session_id(harness.ctx());
    let output = harness
        .ctx()
        .global_services
        .web_search_runtime
        .search(
            WebSearchArgs {
                query: "reqwest rust".into(),
                count: Some(3),
                freshness: Some("month".into()),
                country: Some("us".into()),
                language: Some("en".into()),
                domain_filter: Vec::new(),
            },
            &session_id,
        )
        .await
        .expect("live brave plugin search");

    end_current_plugin_session(harness.ctx()).await;
    assert_eq!(output.backend, "brave");
    assert!(
        !output.hits.is_empty(),
        "expected Brave plugin backend to return hits, warnings={:?}",
        output.warnings
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn real_serper_plugin_web_search() {
    common::setup_logging();
    common::load_openai_test_env();
    if !require_env_var_or_skip("SERPER_API_KEY", "real_serper_plugin_web_search") {
        return;
    }
    const ENV_KEY: &str = "TOMCAT_REAL_SERPER_PROD_CLIENT_KEY";
    let _env = EnvGuard::set_many(&[(ENV_KEY, Some("stub"))]);

    let mut cfg = AppConfig::default();
    cfg.tools.web_search.backend = "serper".into();
    let harness = build_production_web_search_harness(cfg, ENV_KEY, None);
    let session_id = current_session_id(harness.ctx());
    let output = harness
        .ctx()
        .global_services
        .web_search_runtime
        .search(
            WebSearchArgs {
                query: "rust programming language".into(),
                count: Some(5),
                freshness: None,
                country: Some("us".into()),
                language: Some("en".into()),
                domain_filter: Vec::new(),
            },
            &session_id,
        )
        .await
        .expect("live serper plugin search");

    end_current_plugin_session(harness.ctx()).await;
    assert_eq!(output.backend, "serper");
    assert!(
        !output.hits.is_empty(),
        "expected Serper plugin backend to return hits, warnings={:?}",
        output.warnings
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn real_mimo_web_search() {
    common::setup_logging();
    common::load_openai_test_env();
    if !require_env_var_or_skip("MIMO_API_KEY", "real_mimo_web_search") {
        return;
    }
    const ENV_KEY: &str = "TOMCAT_REAL_MIMO_PROD_CLIENT_KEY";
    let _env = EnvGuard::set_many(&[
        (ENV_KEY, Some("stub")),
        ("TAVILY_API_KEY", None),
        ("BRAVE_API_KEY", None),
        ("SERPER_API_KEY", None),
    ]);

    let mimo_model =
        std::env::var("TOMCAT_E2E_MIMO_MODEL").unwrap_or_else(|_| "mimo-v2.5-pro".to_string());
    let mimo_base_url = std::env::var("TOMCAT_E2E_MIMO_BASE_URL")
        .or_else(|_| std::env::var("PI_LIVE_MIMO_BASE_URL"))
        .unwrap_or_else(|_| "https://token-plan-cn.xiaomimimo.com".to_string());

    let mut cfg = AppConfig::default();
    cfg.tools.web_search.backend = "mimo".into();
    let harness = build_production_web_search_harness(
        cfg,
        ENV_KEY,
        Some(mimo_models_toml(&mimo_model, &mimo_base_url)),
    );
    let session_id = current_session_id(harness.ctx());
    let output = harness
        .ctx()
        .global_services
        .web_search_runtime
        .search(
            WebSearchArgs {
                query: "Rust reqwest async".into(),
                count: Some(5),
                freshness: Some("month".into()),
                country: Some("us".into()),
                language: Some("en".into()),
                domain_filter: Vec::new(),
            },
            &session_id,
        )
        .await
        .expect("live mimo plugin search");

    end_current_plugin_session(harness.ctx()).await;
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
}

fn require_env_var_or_skip(env_key: &str, test_name: &str) -> bool {
    if std::env::var(env_key).is_ok() {
        return true;
    }
    eprintln!("skip {test_name}: missing {env_key}");
    false
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

struct PluginRuntimeHarness {
    runtime: WebSearchRuntime,
    manager: Arc<PluginManager>,
    _temp: tempfile::TempDir,
}

struct ProductionWebSearchHarness {
    _runtime: Option<tokio::runtime::Runtime>,
    ctx: Option<tomcat::api::chat::ChatContext>,
    _work_dir: tempfile::TempDir,
    _workspace: tempfile::TempDir,
    _cwd_guard: common::CwdGuard,
}

impl ProductionWebSearchHarness {
    fn ctx(&self) -> &tomcat::api::chat::ChatContext {
        self.ctx.as_ref().expect("chat context should exist")
    }
}

impl Drop for ProductionWebSearchHarness {
    fn drop(&mut self) {
        if let Some(ctx) = self.ctx.take() {
            drop(ctx);
        }
        if let Some(runtime) = self._runtime.take() {
            std::thread::spawn(move || drop(runtime))
                .join()
                .expect("drop production web search runtime");
        }
    }
}

fn build_runtime_with_builtin_plugin(
    config: AppConfig,
    models_toml: Option<String>,
) -> PluginRuntimeHarness {
    build_runtime_with_builtin_plugin_and_fetch_client(config, models_toml, None)
}

fn build_runtime_with_builtin_plugin_without_mimo_and_fetch_client(
    config: AppConfig,
    models_toml: Option<String>,
    fetch_client: Option<reqwest::Client>,
) -> PluginRuntimeHarness {
    build_runtime_with_builtin_plugin_and_fetch_client_with_patch(
        config,
        models_toml,
        fetch_client,
        Some("\nbackends.mimo = undefined;\n"),
    )
}

fn build_runtime_with_builtin_plugin_and_fetch_client(
    config: AppConfig,
    models_toml: Option<String>,
    fetch_client: Option<reqwest::Client>,
) -> PluginRuntimeHarness {
    build_runtime_with_builtin_plugin_and_fetch_client_with_patch(
        config,
        models_toml,
        fetch_client,
        None,
    )
}

fn build_runtime_with_builtin_plugin_and_fetch_client_with_patch(
    config: AppConfig,
    models_toml: Option<String>,
    fetch_client: Option<reqwest::Client>,
    patch_main_js: Option<&str>,
) -> PluginRuntimeHarness {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("models.toml");
    if let Some(contents) = models_toml {
        std::fs::write(&path, contents).expect("write models.toml");
    }
    let catalog =
        Arc::new(ModelCatalog::load_from_path(&config, path).expect("load model catalog"));
    let runtime = WebSearchRuntime::new(&config, catalog.clone()).expect("build runtime");

    let plugin_root = install_builtin_web_search_plugin(temp.path());
    if let Some(snippet) = patch_main_js {
        let main_js = plugin_root.join("main.js");
        let mut script = std::fs::read_to_string(&main_js).expect("read builtin web_search main.js");
        script.push_str(snippet);
        std::fs::write(&main_js, script).expect("patch builtin web_search main.js");
    }
    let event_bus = Arc::new(DefaultEventBus::new());
    let mut manager = Arc::new(PluginManager::new(event_bus.clone()));
    let function_registry = Arc::new(FunctionRegistry::new());
    let inner = Arc::get_mut(&mut manager).expect("plugin manager should be uniquely owned");
    inner.set_plugin_engine(PluginEngine::global(None).expect("create quickjs engine"));
    inner.set_plugin_runtime_manager(Arc::new(PluginRuntimeManager::new()));
    inner.set_audit_recorder(Arc::new(TracingAuditRecorder));
    inner.set_function_registry(function_registry.clone());

    let invoker = PluginFunctionInvoker::new(Arc::downgrade(&manager));
    let mut dispatcher = HostApiDispatcher::new(event_bus.clone())
        .with_tokio_handle(tokio::runtime::Handle::current())
        .with_plugin_manager(Arc::downgrade(&manager))
        .with_llm_resolver(Arc::new(DefaultLlmResolver::new(
            config.clone(),
            catalog.clone(),
        )));
    if let Some(fetch_client) = fetch_client {
        dispatcher = dispatcher.with_fetch_http_client(fetch_client);
    }
    let dispatcher = Arc::new(dispatcher);
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

    PluginRuntimeHarness {
        runtime,
        manager,
        _temp: temp,
    }
}

fn build_production_web_search_harness(
    mut config: AppConfig,
    env_key: &str,
    models_toml: Option<String>,
) -> ProductionWebSearchHarness {
    let work_dir = tempfile::tempdir().expect("work dir");
    install_builtin_web_search_plugin(work_dir.path());
    if let Some(contents) = models_toml {
        std::fs::write(work_dir.path().join("models.toml"), contents).expect("write models.toml");
    }
    let workspace = tempfile::tempdir().expect("workspace");
    let cwd_guard = common::CwdGuard::set(workspace.path());
    config.storage.work_dir = Some(work_dir.path().to_string_lossy().to_string());
    config.llm.api_key_env = Some(env_key.to_string());
    let (runtime, ctx) =
        tomcat::api::cli::build_runtime_and_context(&config, SessionMode::Claw)
            .expect("chat context should be created with production runtime ordering");
    ProductionWebSearchHarness {
        _runtime: Some(runtime),
        ctx: Some(ctx),
        _work_dir: work_dir,
        _workspace: workspace,
        _cwd_guard: cwd_guard,
    }
}

fn current_session_id(ctx: &tomcat::api::chat::ChatContext) -> String {
    ctx.session_runtime
        .session
        .current_session_id()
        .expect("current_session_id")
        .expect("session id should exist")
}

async fn end_current_plugin_session(ctx: &tomcat::api::chat::ChatContext) {
    let Some(plugin_manager) = ctx.global_services.plugin_manager.as_ref() else {
        return;
    };
    let session_id = current_session_id(ctx);
    plugin_manager
        .end_session(&session_id)
        .await
        .expect("end current plugin session");
}

fn mimo_models_toml(model_id: &str, base_url: &str) -> String {
    format!(
        r#"
[[models]]
id = "{model_id}"
api = "openai"
provider = "mimo"
base_url = "{base_url}"

[models.capabilities]
tools = true
reasoning = true
"#
    )
}

fn hosted_openai_models_toml(base_url: &str) -> String {
    format!(
        r#"
[[models]]
id = "gpt-5.4-web"
api = "openai-responses"
provider = "openai"
base_url = "{base_url}"

[models.capabilities]
web_search = true
"#
    )
}

fn hosted_web_search_response() -> Vec<u8> {
    serde_json::to_vec(&json!({
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
    }))
    .expect("serialize hosted web_search response")
}

fn install_builtin_web_search_plugin(dest_root: &Path) -> PathBuf {
    let src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join("plugins")
        .join("web-search-backends");
    let dst = dest_root.join("plugins").join("web-search-backends");
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
