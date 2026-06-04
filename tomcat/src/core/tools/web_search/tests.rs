use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use serial_test::serial;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::core::llm::ModelCatalog;
use crate::infra::AppConfig;

use super::backend::discover_hosted_candidate;
use super::openai_server::{build_hosted_request_body, parse_server_tool_blocks};
use super::types::{normalize_hits, RawHit, WebSearchArgs};
use super::WebSearchRuntime;

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
        .search(WebSearchArgs {
            query: "reqwest rust".into(),
            count: Some(3),
            freshness: Some("day".into()),
            country: Some("us".into()),
            language: Some("en".into()),
            domain_filter: vec!["docs.rs".into()],
        })
        .await
        .expect("tavily search");

    assert_eq!(output.backend, "tavily");
    assert_eq!(output.hits.len(), 1);
    assert!(output
        .warnings
        .iter()
        .any(|w| w == "country_ignored:backend=tavily"));
    assert!(output
        .warnings
        .iter()
        .any(|w| w == "language_ignored:backend=tavily"));

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
        .search(WebSearchArgs {
            query: "reqwest".into(),
            count: None,
            freshness: None,
            country: None,
            language: None,
            domain_filter: Vec::new(),
        })
        .await
        .expect("first auto search");
    assert_eq!(first.backend, "brave");
    assert!(!first.stats.cached);
    assert!(first
        .warnings
        .iter()
        .any(|w| w == "backend_unavailable:tavily, fallback=brave"));

    let second = runtime
        .search(WebSearchArgs {
            query: "reqwest".into(),
            count: None,
            freshness: None,
            country: None,
            language: None,
            domain_filter: Vec::new(),
        })
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
                .set_delay(Duration::from_millis(50))
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
    cfg.tools.web_search.timeout_ms = 5;
    cfg.tools.web_search.brave_base_url = brave.uri();
    cfg.tools.web_search.serper_base_url = serper.uri();
    let runtime = runtime_with_catalog(cfg, None);

    let output = runtime
        .search(WebSearchArgs {
            query: "rust".into(),
            count: None,
            freshness: None,
            country: None,
            language: None,
            domain_filter: Vec::new(),
        })
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
        .search(WebSearchArgs {
            query: "reqwest".into(),
            count: None,
            freshness: None,
            country: Some("us".into()),
            language: None,
            domain_filter: vec!["docs.rs".into()],
        })
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
        .search(WebSearchArgs {
            query: "rust".into(),
            count: None,
            freshness: None,
            country: None,
            language: None,
            domain_filter: Vec::new(),
        })
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
        .search(WebSearchArgs {
            query: "rust".into(),
            count: None,
            freshness: None,
            country: None,
            language: None,
            domain_filter: Vec::new(),
        })
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
        .search(WebSearchArgs {
            query: "rust".into(),
            count: None,
            freshness: None,
            country: None,
            language: None,
            domain_filter: Vec::new(),
        })
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
