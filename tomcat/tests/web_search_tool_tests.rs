use std::sync::Arc;

use serde_json::json;
use serial_test::serial;
use tomcat::core::tools::web_search::types::WebSearchArgs;
use tomcat::core::tools::web_search::WebSearchRuntime;
use tomcat::{AppConfig, ModelCatalog};
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
        .search(WebSearchArgs {
            query: "tokio rust".into(),
            count: Some(3),
            freshness: Some("week".into()),
            country: None,
            language: None,
            domain_filter: vec!["tokio.rs".into()],
        })
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
        .search(WebSearchArgs {
            query: "reqwest rust".into(),
            count: None,
            freshness: None,
            country: None,
            language: None,
            domain_filter: vec!["docs.rs".into()],
        })
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
        .search(WebSearchArgs {
            query: "rust book".into(),
            count: Some(5),
            freshness: Some("month".into()),
            country: Some("us".into()),
            language: Some("en".into()),
            domain_filter: vec!["doc.rust-lang.org".into()],
        })
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
        .search(WebSearchArgs {
            query: "rust language".into(),
            count: Some(3),
            freshness: None,
            country: None,
            language: None,
            domain_filter: vec!["rust-lang.org".into()],
        })
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
        .search(WebSearchArgs {
            query: "rust async runtime".into(),
            count: Some(3),
            freshness: Some("month".into()),
            country: None,
            language: Some("en".into()),
            domain_filter: vec!["tokio.rs".into(), "docs.rs".into()],
        })
        .await
        .expect("live tavily search");

    assert_eq!(output.backend, "tavily");
    assert!(
        !output.hits.is_empty(),
        "expected at least one Tavily hit when PI_LIVE_WEB_SEARCH=1"
    );
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
