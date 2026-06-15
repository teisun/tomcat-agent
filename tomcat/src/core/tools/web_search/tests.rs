use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;
use serial_test::serial;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::core::llm::ModelCatalog;
use crate::infra::AppConfig;

use super::backend::{
    discover_hosted_candidate, pick_backend, BackendFailure, BackendMode, BackendPlan,
    HTTP_AUTO_CHAIN,
};
use super::cache::CacheKey;
use super::openai_server::{build_hosted_request_body, parse_server_tool_blocks};
use super::plugin_backend::PluginSearchInvoker;
use super::types::{normalize_hits, RawHit, WebSearchArgs};
use super::WebSearchRuntime;

struct RecordingPluginInvoker {
    calls: Mutex<Vec<(String, serde_json::Value, String)>>,
    responses: Mutex<VecDeque<Result<serde_json::Value, BackendFailure>>>,
}

impl RecordingPluginInvoker {
    fn with_responses(responses: Vec<Result<serde_json::Value, BackendFailure>>) -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            responses: Mutex::new(VecDeque::from(responses)),
        })
    }

    fn calls(&self) -> Vec<(String, serde_json::Value, String)> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl PluginSearchInvoker for RecordingPluginInvoker {
    async fn search(
        &self,
        backend: &str,
        params: serde_json::Value,
        session_id: &str,
    ) -> Result<serde_json::Value, BackendFailure> {
        self.calls.lock().unwrap().push((
            backend.to_string(),
            params.clone(),
            session_id.to_string(),
        ));
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| {
                Ok(json!({
                    "backend": backend,
                    "hits": [],
                    "warnings": [],
                }))
            })
    }
}

#[test]
fn discover_hosted_candidate_uses_merged_catalog_order() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("models.toml");
    std::fs::write(
        &path,
        r#"
[[models]]
id = "custom-hosted"
api = "openai-responses"
provider = "openai"

[models.capabilities]
web_search = true

[[models]]
id = "gpt-5.4"

[models.capabilities]
web_search = true
"#,
    )
    .expect("write models.toml");

    let cfg = AppConfig::default();
    let catalog = ModelCatalog::load_from_path(&cfg, path).expect("load catalog");
    let candidate = discover_hosted_candidate(&catalog).expect("hosted candidate");
    assert_eq!(candidate.id, "gpt-5.4");
}

#[test]
fn normalize_hits_filters_private_hosts_and_domain_rules() {
    let normalized = normalize_hits(
        vec![
            RawHit {
                title: Some("Private".into()),
                url: "http://127.0.0.1:8080".into(),
                snippet: Some("nope".into()),
                published_at: None,
            },
            RawHit {
                title: Some("Blocked".into()),
                url: "https://blocked.example.com/x".into(),
                snippet: Some("blocked".into()),
                published_at: None,
            },
            RawHit {
                title: Some("Allowed".into()),
                url: "https://docs.rs/reqwest".into(),
                snippet: Some("HTTP client".into()),
                published_at: None,
            },
            RawHit {
                title: Some("Other".into()),
                url: "https://example.org".into(),
                snippet: Some("filtered".into()),
                published_at: None,
            },
            RawHit {
                title: Some("PublicIp".into()),
                url: "https://8.8.8.8/dns".into(),
                snippet: Some("ip literal".into()),
                published_at: None,
            },
            RawHit {
                title: Some("InternalDns".into()),
                url: "https://metadata.google.internal".into(),
                snippet: Some("internal".into()),
                published_at: None,
            },
        ],
        10,
        &["docs.rs".to_string()],
        &["blocked.example.com".to_string()],
    );

    assert_eq!(normalized.hits.len(), 1);
    assert_eq!(normalized.hits[0].url, "https://docs.rs/reqwest");
    assert!(normalized.warnings.iter().any(|w| w == "ssrf_filtered"));
    assert!(normalized
        .warnings
        .iter()
        .any(|w| w == "domain_blocked:blocked.example.com"));
    assert!(normalized
        .warnings
        .iter()
        .any(|w| w == "domain_filtered:example.org"));
}

#[test]
fn cache_key_tracks_allowed_and_blocked_domains() {
    let cfg = AppConfig::default();
    let req_a = super::types::WebSearchRequest::from_tool_args(
        WebSearchArgs {
            query: "rust async".into(),
            count: Some(5),
            freshness: None,
            country: None,
            language: None,
            domain_filter: vec!["docs.rs".into()],
        },
        &cfg.tools.web_search,
    )
    .expect("request a");
    let mut cfg_b = cfg.clone();
    cfg_b.tools.web_search.blocked_domains = vec!["docs.rs".into()];
    let req_b = super::types::WebSearchRequest::from_tool_args(
        WebSearchArgs {
            query: "rust async".into(),
            count: Some(5),
            freshness: None,
            country: None,
            language: None,
            domain_filter: vec!["docs.rs".into()],
        },
        &cfg_b.tools.web_search,
    )
    .expect("request b");

    assert_ne!(
        CacheKey::from_request(&req_a),
        CacheKey::from_request(&req_b),
        "cache key must change when config-level allow/block filters change"
    );
}

#[test]
fn build_hosted_request_body_includes_filters_and_location() {
    let request = super::types::WebSearchRequest::from_tool_args(
        WebSearchArgs {
            query: "rust async".into(),
            count: Some(5),
            freshness: None,
            country: Some("us".into()),
            language: None,
            domain_filter: vec!["docs.rs".into()],
        },
        &AppConfig::default().tools.web_search,
    )
    .expect("request");

    let body = build_hosted_request_body("gpt-5.4", &request);
    assert_eq!(body["model"], json!("gpt-5.4"));
    assert_eq!(body["tool_choice"], json!("required"));
    assert_eq!(body["tools"][0]["type"], json!("web_search"));
    assert_eq!(
        body["tools"][0]["filters"]["allowed_domains"],
        json!(["docs.rs"])
    );
    assert_eq!(body["tools"][0]["user_location"]["country"], json!("US"));
}

#[test]
fn parse_server_tool_blocks_handles_openai_and_server_tool_shapes() {
    let parsed = parse_server_tool_blocks(&json!({
        "output": [
            {
                "type": "web_search_call",
                "results": [
                    {
                        "url": "https://docs.rs/reqwest",
                        "title": "reqwest",
                        "snippet": "HTTP client"
                    }
                ]
            },
            {
                "type": "message",
                "content": [
                    {
                        "type": "output_text",
                        "text": "reqwest is an HTTP client",
                        "annotations": [
                            {
                                "type": "url_citation",
                                "url": "https://docs.rs/reqwest",
                                "title": "reqwest",
                                "start_index": 0,
                                "end_index": 7
                            }
                        ]
                    }
                ]
            },
            {
                "type": "web_search_tool_result",
                "content": [
                    {
                        "type": "web_search_result",
                        "url": "https://www.rust-lang.org",
                        "title": "Rust",
                        "snippet": "Language homepage"
                    }
                ]
            }
        ]
    }))
    .expect("parse output");

    assert_eq!(parsed.raw_hits.len(), 2);
    assert_eq!(parsed.raw_hits[0].url, "https://docs.rs/reqwest");
    assert_eq!(parsed.raw_hits[1].url, "https://www.rust-lang.org");
}

#[test]
fn backend_mode_parse_supports_builtin_and_plugin_names() {
    assert_eq!(BackendMode::parse("auto").unwrap(), BackendMode::Auto);
    assert_eq!(BackendMode::parse("openai").unwrap(), BackendMode::Openai);
    assert_eq!(BackendMode::parse("tavily").unwrap(), BackendMode::Tavily);
    assert_eq!(BackendMode::parse("brave").unwrap(), BackendMode::Brave);
    assert_eq!(BackendMode::parse("serper").unwrap(), BackendMode::Serper);
    assert_eq!(
        BackendMode::parse("MiMo").unwrap(),
        BackendMode::Plugin("mimo".to_string())
    );
    assert_eq!(
        BackendMode::Plugin("mimo".to_string()).clone().as_str(),
        "mimo"
    );
}

#[test]
fn auto_backend_plan_contains_single_plugin_slot() {
    match pick_backend(BackendMode::Auto, None).expect("auto backend plan") {
        BackendPlan::Auto {
            hosted_candidate,
            http_chain,
            plugin_slot,
        } => {
            assert!(hosted_candidate.is_none());
            assert_eq!(http_chain, HTTP_AUTO_CHAIN.to_vec());
            assert!(plugin_slot, "auto path should end with one plugin slot");
        }
        other => panic!("unexpected backend plan: {other:?}"),
    }
}

#[tokio::test]
async fn explicit_plugin_backend_roundtrips_and_normalizes_hits() {
    let invoker = RecordingPluginInvoker::with_responses(vec![Ok(json!({
        "backend": "mimo",
        "hits": [
            {
                "title": "Reqwest",
                "url": "https://docs.rs/reqwest",
                "snippet": "HTTP client"
            },
            {
                "title": "Discard Missing Url"
            },
            {
                "title": "Discard Private Url",
                "url": "http://127.0.0.1/private",
                "snippet": "private"
            }
        ],
        "warnings": ["mimo_ignores_language"]
    }))]);

    let mut cfg = AppConfig::default();
    cfg.tools.web_search.backend = "mimo".into();
    let runtime = runtime_with_catalog(cfg, None);
    runtime.set_plugin_invoker(invoker.clone());

    let output = runtime
        .search(
            WebSearchArgs {
                query: "reqwest rust".into(),
                count: Some(5),
                freshness: Some("day".into()),
                country: Some("us".into()),
                language: Some("en".into()),
                domain_filter: vec!["docs.rs".into()],
            },
            "session-plugin-1",
        )
        .await
        .expect("plugin search");

    assert_eq!(output.backend, "mimo");
    assert_eq!(output.hits.len(), 1);
    assert_eq!(output.hits[0].url, "https://docs.rs/reqwest");
    assert!(output.warnings.iter().any(|w| w == "mimo_ignores_language"));
    assert!(output.warnings.iter().any(|w| w == "ssrf_filtered"));

    let calls = invoker.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "mimo");
    assert_eq!(calls[0].2, "session-plugin-1");
    assert_eq!(calls[0].1["query"], json!("reqwest rust"));
    assert_eq!(calls[0].1["count"], json!(5));
    assert_eq!(calls[0].1["freshness"], json!("day"));
    assert_eq!(calls[0].1["country"], json!("US"));
    assert_eq!(calls[0].1["language"], json!("en"));
    assert_eq!(calls[0].1["domainFilter"], json!(["docs.rs"]));
}

#[tokio::test]
async fn explicit_plugin_backend_without_invoker_returns_clear_error() {
    let mut cfg = AppConfig::default();
    cfg.tools.web_search.backend = "mimo".into();
    let runtime = runtime_with_catalog(cfg, None);

    let err = runtime
        .search(
            WebSearchArgs {
                query: "reqwest".into(),
                count: Some(3),
                freshness: None,
                country: None,
                language: None,
                domain_filter: Vec::new(),
            },
            "session-plugin-2",
        )
        .await
        .expect_err("missing plugin invoker should fail");

    assert!(err
        .to_string()
        .contains("web_search plugin backend invoker not configured"));
}

#[tokio::test]
async fn explicit_plugin_backend_timeout_returns_degraded_output() {
    let invoker = RecordingPluginInvoker::with_responses(vec![Err(BackendFailure::Timeout)]);

    let mut cfg = AppConfig::default();
    cfg.tools.web_search.backend = "mimo".into();
    let runtime = runtime_with_catalog(cfg, None);
    runtime.set_plugin_invoker(invoker);

    let output = runtime
        .search(
            WebSearchArgs {
                query: "reqwest".into(),
                count: Some(3),
                freshness: None,
                country: None,
                language: None,
                domain_filter: Vec::new(),
            },
            "session-plugin-3",
        )
        .await
        .expect("timeout should degrade, not hard fail");

    assert_eq!(output.backend, "mimo");
    assert!(output.hits.is_empty());
    assert!(output.truncated);
    assert!(output
        .warnings
        .iter()
        .any(|w| w == "backend_unavailable:mimo"));
    assert!(output
        .warnings
        .iter()
        .any(|w| w == "timeout (backend=mimo)"));
}

#[tokio::test]
async fn explicit_plugin_backend_unsupported_error_is_preserved() {
    let invoker = RecordingPluginInvoker::with_responses(vec![Err(BackendFailure::Incompatible {
        detail: "未找到名为 `mimo` 的 web_search 插件后端".to_string(),
    })]);

    let mut cfg = AppConfig::default();
    cfg.tools.web_search.backend = "mimo".into();
    let runtime = runtime_with_catalog(cfg, None);
    runtime.set_plugin_invoker(invoker);

    let err = runtime
        .search(
            WebSearchArgs {
                query: "reqwest".into(),
                count: Some(3),
                freshness: None,
                country: None,
                language: None,
                domain_filter: Vec::new(),
            },
            "session-plugin-4",
        )
        .await
        .expect_err("unsupported plugin backend should fail clearly");

    assert!(err
        .to_string()
        .contains("未找到名为 `mimo` 的 web_search 插件后端"));
}

#[tokio::test]
#[serial]
async fn auto_plugin_slot_calls_invoker_once_and_then_hits_cache() {
    let _env = EnvGuard::set_many(&[
        ("TAVILY_API_KEY", None),
        ("BRAVE_API_KEY", None),
        ("SERPER_API_KEY", None),
        ("OPENAI_API_KEY", None),
    ]);
    let invoker = RecordingPluginInvoker::with_responses(vec![Ok(json!({
        "backend": "mimo",
        "hits": [
            {
                "title": "Tokio",
                "url": "https://tokio.rs",
                "snippet": "Async runtime"
            }
        ],
        "warnings": ["plugin_auto_used"]
    }))]);

    let mut cfg = AppConfig::default();
    cfg.tools.web_search.backend = "auto".into();
    cfg.tools.web_search.cache_capacity = 8;
    cfg.tools.web_search.cache_ttl_secs = 60;
    let runtime = runtime_with_catalog(cfg, None);
    runtime.set_plugin_invoker(invoker.clone());

    let first = runtime
        .search(
            WebSearchArgs {
                query: "tokio rust".into(),
                count: Some(3),
                freshness: None,
                country: None,
                language: None,
                domain_filter: vec!["tokio.rs".into()],
            },
            "session-plugin-auto-1",
        )
        .await
        .expect("first auto plugin search");
    let second = runtime
        .search(
            WebSearchArgs {
                query: "tokio rust".into(),
                count: Some(3),
                freshness: None,
                country: None,
                language: None,
                domain_filter: vec!["tokio.rs".into()],
            },
            "session-plugin-auto-2",
        )
        .await
        .expect("second auto plugin search");

    assert_eq!(first.backend, "mimo");
    assert!(!first.stats.cached);
    assert_eq!(second.backend, "mimo");
    assert!(second.stats.cached);
    assert_eq!(
        invoker.calls().len(),
        1,
        "cache hit should skip plugin invoker"
    );
}

#[tokio::test]
#[serial]
async fn tavily_runtime_maps_request_and_normalizes_hits() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [
                {
                    "title": "reqwest",
                    "url": "https://docs.rs/reqwest/latest/reqwest/",
                    "content": "An ergonomic HTTP Client for Rust.",
                    "published_date": "2026-06-01"
                }
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let _env = EnvGuard::set_many(&[
        ("TAVILY_API_KEY", Some("tavily-test-key")),
        ("BRAVE_API_KEY", None),
        ("SERPER_API_KEY", None),
        ("OPENAI_API_KEY", None),
    ]);

    let mut cfg = AppConfig::default();
    cfg.tools.web_search.backend = "tavily".into();
    cfg.tools.web_search.tavily_base_url = server.uri();
    let runtime = runtime_with_catalog(cfg, None);
    let output = runtime
        .search(
            WebSearchArgs {
                query: "reqwest rust".into(),
                count: Some(3),
                freshness: Some("day".into()),
                country: Some("us".into()),
                language: Some("en".into()),
                domain_filter: vec!["docs.rs".into()],
            },
            "test-session",
        )
        .await
        .expect("tavily search");

    assert_eq!(output.backend, "tavily");
    assert_eq!(output.hits.len(), 1);
    assert!(output
        .warnings
        .iter()
        .any(|w| w == "tavily_ignores_country_language"));

    let requests = server.received_requests().await.expect("requests");
    let request = requests.last().expect("single request");
    assert_eq!(
        request
            .headers
            .get("authorization")
            .and_then(|value| value.to_str().ok()),
        Some("Bearer tavily-test-key")
    );
    let body: serde_json::Value = serde_json::from_slice(&request.body).expect("json body");
    assert_eq!(body["query"], json!("reqwest rust"));
    assert_eq!(body["max_results"], json!(3));
    assert_eq!(body["time_range"], json!("day"));
    assert_eq!(body["include_domains"], json!(["docs.rs"]));
}

#[tokio::test]
#[serial]
async fn auto_backend_falls_back_to_brave_and_then_hits_cache() {
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
        ("OPENAI_API_KEY", None),
    ]);

    let mut cfg = AppConfig::default();
    cfg.tools.web_search.backend = "auto".into();
    cfg.tools.web_search.brave_base_url = brave.uri();
    cfg.tools.web_search.cache_capacity = 8;
    cfg.tools.web_search.cache_ttl_secs = 60;
    let runtime = runtime_with_catalog(cfg, None);

    let first = runtime
        .search(
            WebSearchArgs {
                query: "reqwest".into(),
                count: None,
                freshness: None,
                country: None,
                language: None,
                domain_filter: vec!["docs.rs".into()],
            },
            "test-session",
        )
        .await
        .expect("first auto search");
    assert_eq!(first.backend, "brave");
    assert!(!first.stats.cached);
    assert!(first
        .warnings
        .iter()
        .any(|w| w == "backend_unavailable:tavily, fallback=brave"));
    assert!(first
        .warnings
        .iter()
        .any(|w| w == "brave_domain_filter_via_query_rewrite"));

    let second = runtime
        .search(
            WebSearchArgs {
                query: "reqwest".into(),
                count: None,
                freshness: None,
                country: None,
                language: None,
                domain_filter: vec!["docs.rs".into()],
            },
            "test-session",
        )
        .await
        .expect("second auto search");
    assert_eq!(second.backend, "brave");
    assert!(second.stats.cached);

    let requests = brave.received_requests().await.expect("requests");
    assert_eq!(requests.len(), 1);
}

#[tokio::test]
#[serial]
async fn auto_backend_falls_back_after_brave_timeout() {
    let brave = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/res/v1/web/search"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_millis(750))
                .set_body_json(json!({
                    "web": {
                        "results": [
                            {
                                "title": "slow brave",
                                "url": "https://search.brave.com",
                                "description": "slow response"
                            }
                        ]
                    }
                })),
        )
        .expect(1)
        .mount(&brave)
        .await;

    let serper = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "organic": [
                {
                    "title": "Rust",
                    "link": "https://www.rust-lang.org",
                    "snippet": "Language homepage"
                }
            ]
        })))
        .expect(1)
        .mount(&serper)
        .await;

    let _env = EnvGuard::set_many(&[
        ("TAVILY_API_KEY", None),
        ("BRAVE_API_KEY", Some("brave-test-key")),
        ("SERPER_API_KEY", Some("serper-test-key")),
        ("OPENAI_API_KEY", None),
    ]);

    let mut cfg = AppConfig::default();
    cfg.tools.web_search.backend = "auto".into();
    cfg.tools.web_search.timeout_ms = 200;
    cfg.tools.web_search.brave_base_url = brave.uri();
    cfg.tools.web_search.serper_base_url = serper.uri();
    let runtime = runtime_with_catalog(cfg, None);

    let output = runtime
        .search(
            WebSearchArgs {
                query: "rust".into(),
                count: None,
                freshness: None,
                country: None,
                language: None,
                domain_filter: Vec::new(),
            },
            "test-session",
        )
        .await
        .expect("timeout fallback search");

    assert_eq!(output.backend, "serper");
    assert_eq!(output.hits.len(), 1);
    assert!(output
        .warnings
        .iter()
        .any(|w| w == "backend_unavailable:brave, fallback=serper"));
    assert!(output
        .warnings
        .iter()
        .any(|w| w == "timeout (backend=brave)"));
}

#[tokio::test]
#[serial]
async fn auto_backend_uses_project_hosted_candidate() {
    let hosted = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "output": [
                {
                    "type": "web_search_call",
                    "results": [
                        {
                            "url": "https://docs.rs/reqwest",
                            "title": "reqwest",
                            "snippet": "HTTP client"
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
    cfg.tools.web_search.backend = "auto".into();
    let runtime = runtime_with_catalog(
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
                query: "reqwest".into(),
                count: None,
                freshness: None,
                country: Some("us".into()),
                language: None,
                domain_filter: vec!["docs.rs".into()],
            },
            "test-session",
        )
        .await
        .expect("hosted auto search");
    assert_eq!(output.backend, "openai");
    assert_eq!(output.hits.len(), 1);

    let requests = hosted.received_requests().await.expect("requests");
    let request = requests.last().expect("hosted request");
    let body: serde_json::Value = serde_json::from_slice(&request.body).expect("json body");
    assert_eq!(body["model"], json!("gpt-5.4-web"));
    assert_eq!(body["tools"][0]["type"], json!("web_search"));
    assert_eq!(
        body["tools"][0]["filters"]["allowed_domains"],
        json!(["docs.rs"])
    );
    assert_eq!(body["tools"][0]["user_location"]["country"], json!("US"));
}

#[tokio::test]
#[serial]
async fn explicit_tavily_rate_limit_returns_degraded_output() {
    let tavily = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(429).set_body_string("rate limited"))
        .expect(1)
        .mount(&tavily)
        .await;

    let _env = EnvGuard::set_many(&[
        ("TAVILY_API_KEY", Some("tavily-test-key")),
        ("BRAVE_API_KEY", None),
        ("SERPER_API_KEY", None),
        ("OPENAI_API_KEY", None),
    ]);

    let mut cfg = AppConfig::default();
    cfg.tools.web_search.backend = "tavily".into();
    cfg.tools.web_search.tavily_base_url = tavily.uri();
    let runtime = runtime_with_catalog(cfg, None);

    let output = runtime
        .search(
            WebSearchArgs {
                query: "rust".into(),
                count: None,
                freshness: None,
                country: None,
                language: None,
                domain_filter: Vec::new(),
            },
            "test-session",
        )
        .await
        .expect("rate-limited search");

    assert_eq!(output.backend, "tavily");
    assert!(output.hits.is_empty());
    assert!(output.truncated);
    assert!(output
        .warnings
        .iter()
        .any(|w| w == "backend_unavailable:tavily"));
    assert!(output
        .warnings
        .iter()
        .any(|w| w == "rate_limited (backend=tavily,status=429)"));
}

#[tokio::test]
#[serial]
async fn incompatible_hosted_candidate_falls_back_to_tavily() {
    let tavily = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [
                {
                    "title": "Rust",
                    "url": "https://www.rust-lang.org",
                    "content": "Language homepage"
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
        ("OPENAI_API_KEY", None),
    ]);

    let mut cfg = AppConfig::default();
    cfg.tools.web_search.backend = "auto".into();
    cfg.tools.web_search.tavily_base_url = tavily.uri();
    let runtime = runtime_with_catalog(
        cfg,
        Some(
            r#"
[[models]]
id = "hosted-ish"
api = "openai"
provider = "openai"

[models.capabilities]
web_search = true
"#
            .to_string(),
        ),
    );

    let output = runtime
        .search(
            WebSearchArgs {
                query: "rust".into(),
                count: None,
                freshness: None,
                country: None,
                language: None,
                domain_filter: Vec::new(),
            },
            "test-session",
        )
        .await
        .expect("fallback search");
    assert_eq!(output.backend, "tavily");
    assert!(output
        .warnings
        .iter()
        .any(|w| w == "hosted_candidate_unavailable, fallback=tavily"));
}

#[tokio::test]
#[serial]
async fn explicit_openai_requires_project_candidate() {
    let _env = EnvGuard::set_many(&[
        ("TAVILY_API_KEY", None),
        ("BRAVE_API_KEY", None),
        ("SERPER_API_KEY", None),
        ("OPENAI_API_KEY", None),
    ]);

    let mut cfg = AppConfig::default();
    cfg.tools.web_search.backend = "openai".into();
    let runtime = runtime_with_catalog(cfg, None);
    let error = runtime
        .search(
            WebSearchArgs {
                query: "rust".into(),
                count: None,
                freshness: None,
                country: None,
                language: None,
                domain_filter: Vec::new(),
            },
            "test-session",
        )
        .await
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("no hosted web_search model configured"));
}

fn runtime_with_catalog(config: AppConfig, models_toml: Option<String>) -> WebSearchRuntime {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("models.toml");
    if let Some(contents) = models_toml {
        std::fs::write(&path, contents).expect("write models.toml");
    }
    let catalog =
        Arc::new(ModelCatalog::load_from_path(&config, path).expect("load model catalog"));
    WebSearchRuntime::new(&config, catalog).expect("build runtime")
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
