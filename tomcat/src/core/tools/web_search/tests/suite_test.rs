use async_trait::async_trait;
use serde_json::json;
use serial_test::serial;
use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::core::llm::ModelCatalog;
use crate::ext::PluginEngine;
use crate::infra::i18n::{tr_in, Locale};
use crate::infra::{AppConfig, AppError};

use super::super::cache::CacheKey;
use super::super::declarations::{parse_declarations, BackendDeclaration};
use super::super::plugin_backend::{PluginSearchCatalog, PluginSearchInvoker};
use super::super::types::{normalize_hits, RawHit, WebSearchArgs};
use super::super::WebSearchRuntime;

struct RecordingPluginInvoker {
    calls: Mutex<Vec<(String, serde_json::Value, String)>>,
    responses: Mutex<VecDeque<Result<serde_json::Value, AppError>>>,
}

impl RecordingPluginInvoker {
    fn with_responses(responses: Vec<Result<serde_json::Value, AppError>>) -> Arc<Self> {
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
    fn search_catalog(&self) -> PluginSearchCatalog {
        let declarations = ["tavily", "brave", "mimo"]
            .into_iter()
            .map(|id| BackendDeclaration {
                id: id.into(),
                label: id.into(),
                secret: Some("TEST_WEB_SEARCH_KEY".into()),
                model: None,
                host: Some(format!("{id}.example")),
                default_auto: true,
            })
            .collect();
        PluginSearchCatalog {
            declarations,
            saved_keys: false,
        }
    }

    async fn search(
        &self,
        backend: &str,
        params: serde_json::Value,
        session_id: &str,
    ) -> Result<serde_json::Value, AppError> {
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

fn plugin_backend_main_js() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join("plugins")
        .join("web-search-backends")
        .join("main.js");
    std::fs::read_to_string(path).expect("read builtin web-search-backends main.js")
}

fn assert_js_parser_matches_expected(
    parser_name: &str,
    response_body: &serde_json::Value,
    expected_hits: &serde_json::Value,
) {
    let engine = PluginEngine::global(None).expect("create quickjs engine");
    let mut instance = engine
        .create_instance(&format!("parser-parity-{parser_name}"))
        .expect("create parser parity instance");
    let captured = Arc::new(Mutex::new(None::<serde_json::Value>));
    let captured_for_host = Arc::clone(&captured);
    instance
        .register_host_binding(move |request_json| {
            let request: serde_json::Value =
                serde_json::from_str(request_json).expect("host request should be JSON");
            if let Some(actual) = request.get("actual").cloned() {
                *captured_for_host.lock().unwrap() = Some(actual);
            }
            Ok(json!({ "ok": true, "data": null }).to_string())
        })
        .expect("register host binding");
    let plugin_code = plugin_backend_main_js();
    let body_json = serde_json::to_string(response_body).expect("serialize parser fixture");
    let script = format!(
        r#"
pi.registerFunction = function () {{}};
{plugin_code}
(function () {{
  var actual = {parser_name}(JSON.parse({body_json:?}));
  __pi_host_call(JSON.stringify({{ actual: actual }}));
}})();
"#
    );
    instance
        .run_script(&script)
        .expect("js parser script should run");
    let actual = captured
        .lock()
        .unwrap()
        .clone()
        .expect("js parser should emit actual hits");
    assert_eq!(
        actual, *expected_hits,
        "{parser_name} parity mismatch between JS parser and legacy Rust backend"
    );
}

fn eval_plugin_script_value(script_body: &str) -> serde_json::Value {
    let engine = PluginEngine::global(None).expect("create quickjs engine");
    let mut instance = engine
        .create_instance("web-search-plugin-js-eval")
        .expect("create plugin js eval instance");
    let captured = Arc::new(Mutex::new(None::<serde_json::Value>));
    let captured_for_host = Arc::clone(&captured);
    instance
        .register_host_binding(move |request_json| {
            let request: serde_json::Value =
                serde_json::from_str(request_json).expect("host request should be JSON");
            if let Some(actual) = request.get("actual").cloned() {
                *captured_for_host.lock().unwrap() = Some(actual);
            }
            Ok(json!({ "ok": true, "data": null }).to_string())
        })
        .expect("register host binding");
    let plugin_code = plugin_backend_main_js();
    let script = format!(
        r#"
pi.registerFunction = function () {{}};
{plugin_code}
(async function () {{
  var actual = await (async function () {{
{script_body}
  }})();
  __pi_host_call(JSON.stringify({{ actual: actual }}));
}})();
"#
    );
    instance
        .run_script(&script)
        .expect("plugin js evaluation should run");
    let actual = captured
        .lock()
        .unwrap()
        .clone()
        .expect("plugin js evaluation should emit a value");
    actual
}

#[test]
fn plugin_js_normalize_count_allows_twenty_results() {
    let actual = eval_plugin_script_value("return normalizeCount(20);");
    assert_eq!(actual, json!(20));
}

#[test]
fn deepseek_request_maps_filters_and_counts_unicode_points() {
    let actual = eval_plugin_script_value(
        r#"
var calls = [];
Date.now = function () { return Date.parse('2026-11-18T03:25:00Z'); };
tomcat.fetch = async function (request) {
  calls.push(request);
  return { status: 200, body: '{"results":[]}' };
};
var empty = await dispatchBackend({backend: 'deepseek', query: 'Rust', domainFilter: ['docs.rs'], language: 'en', country: 'US', freshness: 'week'});
await dispatchBackend({backend: 'deepseek', query: '🌱'.repeat(128)});
var tooLong;
try { await dispatchBackend({backend: 'deepseek', query: '🌱'.repeat(129)}); } catch (err) { tooLong = err.message; }
return {calls: calls, empty: empty, tooLong: tooLong};
"#,
    );
    assert_eq!(actual["calls"].as_array().unwrap().len(), 2);
    let request = &actual["calls"][0];
    assert_eq!(request["url"], "https://api.deepseek.com/v1/tools/search");
    assert_eq!(
        request["headers"]["Authorization"],
        "Bearer {{secret:DEEPSEEK_API_KEY}}"
    );
    assert_eq!(
        request["body"],
        json!({"query": "(Rust) (site:docs.rs) lang:en after:2026-11-11", "count": 5, "need_content": false})
    );
    assert_eq!(actual["empty"]["hits"], json!([]));
    assert_eq!(
        actual["empty"]["warnings"],
        json!(["deepseek_ignores_country"])
    );
    assert_eq!(
        actual["tooLong"],
        "DeepSeek search query must be 1-128 characters after filters"
    );
}

#[test]
fn deepseek_maps_hits_and_rejects_billing_and_malformed_responses() {
    let actual = eval_plugin_script_value(
        r#"
var replies = [
  { status: 200, body: JSON.stringify({results: [null, {title:'no url'}, {url:17}, {url:'  '},
    {title:42, url:'https://rust-lang.org', snippet:'', summary:'Language', date:false},
    {title:'Cargo', url:'https://doc.rust-lang.org/cargo/', snippet:'Manual', date:'2026-11-18'},
    {url:'https://docs.rs'}]}) },
  { status: 402, body: '{}' },
  { status: 200, body: '{}' }
];
tomcat.fetch = async function () { return replies.shift(); };
var req = {backend:'deepseek', query:'rust'};
var found = await dispatchBackend(req);
var errors = [];
for (var i = 0; i < 2; i++) {
  try { await dispatchBackend(req); } catch (err) { errors.push(err.message); }
}
return {hits:found.hits, errors:errors};
"#,
    );
    assert_eq!(
        actual["hits"],
        json!([
            { "title": "https://rust-lang.org", "url": "https://rust-lang.org", "snippet": "Language", "published_at": null },
            { "title": "Cargo", "url": "https://doc.rust-lang.org/cargo/", "snippet": "Manual", "published_at": "2026-11-18" },
            { "title": "https://docs.rs", "url": "https://docs.rs", "snippet": null, "published_at": null }
        ])
    );
    assert_eq!(
        actual["errors"],
        json!([
            "DeepSeek search: insufficient balance (HTTP 402)",
            "DeepSeek search response is missing a results array"
        ])
    );
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
    let req_a = super::super::types::WebSearchRequest::from_tool_args(
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
    let req_b = super::super::types::WebSearchRequest::from_tool_args(
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
        CacheKey::from_request(&req_a, Vec::new()),
        CacheKey::from_request(&req_b, Vec::new()),
        "cache key must change when config-level allow/block filters change"
    );
}

#[test]
fn declarations_ignore_invalid_and_duplicate_entries() {
    let metadata = json!({"backends": [
        {"id":"bespoke","label":"Bespoke","secret":"SEARCH_TEST_KEY","host":"api.bespoke.example","defaultAuto":true},
        {"id":"bespoke","label":"Duplicate","model":"id"},
        {"id":"modelled","label":"Model","model":"id","defaultAuto":true},
        {"id":"host-only","label":"Host only","host":"api.openai.com"},
        {"id":"no-host","label":"No host","secret":"SEARCH_TEST_KEY"},
        {"id":"bad-host","label":"Bad host","secret":"SEARCH_TEST_KEY","host":"localhost"},
        {"id":"bad","label":"Bad","secret":"lowercase","host":"api.bad.example"},
        {"id":"conflict","label":"Conflict","secret":"KEY","model":"id"},
        {"id":"no-key","label":"Missing dependency"}
    ]});
    let declarations = parse_declarations(metadata.as_object());
    assert_eq!(
        super::super::declarations::effective_order(None, &declarations),
        ["bespoke", "modelled"]
    );
}

#[test]
fn secret_backend_is_blocked_when_a_model_at_another_host_shares_its_key() {
    let catalog = test_catalog(
        r#"
[[models]]
id = "gateway"
api = "openai-responses"
provider = "openai"
base_url = "https://gateway.example"
api_key_env = " GATEWAY_SHARED_KEY "
[[models]]
id = "port"
api = "openai-responses"
provider = "openai"
base_url = "https://api.openai.com:444/v1"
api_key_env = "OTHER_PORT_KEY"
[[models]]
id = "official"
api = "openai-responses"
provider = "openai"
base_url = "https://API.OPENAI.COM:443/v1"
api_key_env = "OFFICIAL_KEY"
"#,
    );
    let backend = |secret: &str| BackendDeclaration {
        id: "openai".into(),
        label: "OpenAI".into(),
        secret: Some(secret.into()),
        host: Some("api.openai.com".into()),
        model: None,
        default_auto: true,
    };
    let slot = |secret: &str| {
        backend(secret)
            .key_status(&catalog, true)
            .map(|(slot, _)| slot)
    };
    assert_eq!(slot("GATEWAY_SHARED_KEY"), None);
    assert_eq!(slot("OTHER_PORT_KEY"), None);
    assert_eq!(slot("OFFICIAL_KEY").as_deref(), Some("OFFICIAL_KEY"));
    assert_eq!(slot("OPENAI_API_KEY").as_deref(), Some("OPENAI_API_KEY"));
    assert_eq!(
        slot("UNUSED_SEARCH_KEY").as_deref(),
        Some("UNUSED_SEARCH_KEY")
    );
}

#[tokio::test]
#[serial]
async fn explicit_plugin_backend_roundtrips_and_normalizes_hits() {
    let _env = EnvGuard::set_many(&[("TEST_WEB_SEARCH_KEY", Some("fixture"))]);
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
#[serial]
async fn ordered_route_skips_failures_and_stops_on_empty_with_hot_updates() {
    let _env = EnvGuard::set_many(&[("TEST_WEB_SEARCH_KEY", Some("fixture"))]);
    let invoker = RecordingPluginInvoker::with_responses(vec![
        Err(AppError::Tool("HTTP status 429".into())),
        Ok(json!({"hits": []})),
        Ok(json!({"hits": []})),
    ]);
    let order = Arc::new(parking_lot::RwLock::new(Some(vec![
        "tavily".into(),
        "unknown".into(),
        "brave".into(),
        "mimo".into(),
    ])));
    let runtime =
        runtime_with_catalog(AppConfig::default(), None).with_search_order(Some(order.clone()));
    runtime.set_plugin_invoker(invoker.clone());
    let args = WebSearchArgs {
        query: "rust".into(),
        count: None,
        freshness: None,
        country: None,
        language: None,
        domain_filter: Vec::new(),
    };
    let first = runtime.search(args.clone(), "same").await.unwrap();
    assert_eq!(first.backend, "brave");
    assert!(first.hits.is_empty());
    assert!(first.warnings.iter().any(|w| w.contains("429")));
    assert!(first
        .warnings
        .iter()
        .any(|w| w.contains("backend_unknown:unknown")));
    assert!(
        runtime
            .search(args.clone(), "same")
            .await
            .unwrap()
            .stats
            .cached
    );
    *order.write() = Some(vec!["mimo".into()]);
    let updated = runtime.search(args, "same").await.unwrap();
    assert_eq!(updated.backend, "mimo");
    assert!(!updated.stats.cached);
    assert_eq!(
        invoker
            .calls()
            .iter()
            .map(|call| call.0.as_str())
            .collect::<Vec<_>>(),
        ["tavily", "brave", "mimo"]
    );
}

#[tokio::test]
#[serial]
async fn cache_follows_key_availability_but_not_key_values() {
    let _env = EnvGuard::set_many(&[("TEST_WEB_SEARCH_KEY", Some("first"))]);
    let invoker = RecordingPluginInvoker::with_responses(Vec::new());
    let mut cfg = AppConfig::default();
    cfg.tools.web_search.backends = Some(vec!["tavily".into(), "brave".into()]);
    let runtime = runtime_with_catalog(cfg, None);
    runtime.set_plugin_invoker(invoker.clone());
    let args = WebSearchArgs {
        query: "rust".into(),
        count: None,
        freshness: None,
        country: None,
        language: None,
        domain_filter: Vec::new(),
    };
    assert!(
        !runtime
            .search(args.clone(), "s")
            .await
            .unwrap()
            .stats
            .cached
    );
    std::env::set_var("TEST_WEB_SEARCH_KEY", "rotated");
    assert!(
        runtime
            .search(args.clone(), "s")
            .await
            .unwrap()
            .stats
            .cached
    );
    std::env::remove_var("TEST_WEB_SEARCH_KEY");
    let missing = runtime.search(args.clone(), "s").await.unwrap_err();
    assert!(
        missing.to_string().contains("TEST_WEB_SEARCH_KEY"),
        "{missing}"
    );
    std::env::set_var("TEST_WEB_SEARCH_KEY", "restored");
    assert!(runtime.search(args, "s").await.unwrap().stats.cached);
    assert_eq!(invoker.calls().len(), 1);
}

#[tokio::test]
#[serial]
async fn malformed_plugin_output_is_a_backend_failure() {
    let _env = EnvGuard::set_many(&[("TEST_WEB_SEARCH_KEY", Some("fixture"))]);
    let invoker = RecordingPluginInvoker::with_responses(vec![Ok(json!({"hits": "not-a-list"}))]);
    let mut cfg = AppConfig::default();
    cfg.tools.web_search.backends = Some(vec!["tavily".into()]);
    let runtime = runtime_with_catalog(cfg, None);
    runtime.set_plugin_invoker(invoker);
    let err = runtime
        .search(
            WebSearchArgs {
                query: "rust".into(),
                count: None,
                freshness: None,
                country: None,
                language: None,
                domain_filter: Vec::new(),
            },
            "s",
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains(&tr_in(
        Locale::En,
        "search.runtime",
        &[("backend", "tavily"), ("detail", "")]
    )));
}

#[tokio::test]
#[serial]
async fn tavily_parser_fixture_matches_expected_hits() {
    let fixture = json!({
        "results": [
            {
                "title": "Reqwest",
                "url": "https://docs.rs/reqwest/latest/reqwest/",
                "content": "An ergonomic HTTP client for Rust.",
                "published_date": "2026-06-01"
            },
            {
                "title": "Tokio",
                "url": "https://tokio.rs",
                "snippet": "Async runtime"
            },
            {
                "title": "Drop Missing Url"
            }
        ]
    });
    assert_js_parser_matches_expected(
        "parseTavilyResponse",
        &fixture,
        &json!([
            {
                "title": "Reqwest",
                "url": "https://docs.rs/reqwest/latest/reqwest/",
                "snippet": "An ergonomic HTTP client for Rust.",
                "published_at": "2026-06-01"
            },
            {
                "title": "Tokio",
                "url": "https://tokio.rs",
                "snippet": "Async runtime",
                "published_at": null
            }
        ]),
    );
}

#[tokio::test]
#[serial]
async fn brave_parser_fixture_matches_expected_hits() {
    let fixture = json!({
        "web": {
            "results": [
                {
                    "title": "Reqwest",
                    "url": "https://docs.rs/reqwest",
                    "description": "HTTP client"
                },
                {
                    "title": "Rust",
                    "url": "https://www.rust-lang.org",
                    "page_age": "2 days ago"
                },
                {
                    "title": "Drop Missing Url"
                }
            ]
        }
    });
    assert_js_parser_matches_expected(
        "parseBraveResponse",
        &fixture,
        &json!([
            {
                "title": "Reqwest",
                "url": "https://docs.rs/reqwest",
                "snippet": "HTTP client",
                "published_at": null
            },
            {
                "title": "Rust",
                "url": "https://www.rust-lang.org",
                "snippet": null,
                "published_at": "2 days ago"
            }
        ]),
    );
}

#[tokio::test]
#[serial]
async fn serper_parser_fixture_matches_expected_hits() {
    let fixture = json!({
        "organic": [
            {
                "title": "Rust Book",
                "link": "https://doc.rust-lang.org/book/",
                "snippet": "The Rust Programming Language",
                "date": "Jun 1, 2026"
            },
            {
                "title": "Tokio",
                "link": "https://tokio.rs"
            },
            {
                "title": "Drop Missing Link"
            }
        ]
    });
    assert_js_parser_matches_expected(
        "parseSerperResponse",
        &fixture,
        &json!([
            {
                "title": "Rust Book",
                "url": "https://doc.rust-lang.org/book/",
                "snippet": "The Rust Programming Language",
                "published_at": "Jun 1, 2026"
            },
            {
                "title": "Tokio",
                "url": "https://tokio.rs",
                "snippet": null,
                "published_at": null
            }
        ]),
    );
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

    assert!(err.to_string().contains("backend_unknown:mimo"));
}

#[tokio::test]
#[serial]
async fn single_backend_returns_its_error_unchanged() {
    let _env = EnvGuard::set_many(&[("TEST_WEB_SEARCH_KEY", Some("fixture"))]);
    let invoker = RecordingPluginInvoker::with_responses(vec![Err(AppError::Tool(
        "synthetic runtime failure".into(),
    ))]);

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
            "session-plugin-pluginruntime",
        )
        .await
        .expect_err("a pinned backend should fail without fallback");
    let text = err.to_string();
    assert!(text.contains("synthetic runtime failure"));
    assert!(!text.contains(&tr_in(
        Locale::En,
        "search.allUnavailableDetails",
        &[("query", "reqwest"), ("details", "")]
    )));
}

#[tokio::test]
#[serial]
async fn auto_exhausted_returns_tool_error_and_does_not_cache() {
    let _env = EnvGuard::set_many(&[("TEST_WEB_SEARCH_KEY", Some("fixture"))]);
    let invoker = RecordingPluginInvoker::with_responses(
        (0..4)
            .map(|_| Err(AppError::Tool("synthetic timeout".into())))
            .collect(),
    );
    let mut cfg = AppConfig::default();
    cfg.tools.web_search.backends = Some(vec!["tavily".into(), "brave".into()]);
    cfg.tools.web_search.cache_capacity = 8;
    cfg.tools.web_search.cache_ttl_secs = 60;
    let runtime = runtime_with_catalog(cfg, None);
    runtime.set_plugin_invoker(invoker.clone());

    let args = WebSearchArgs {
        query: "rust".into(),
        count: Some(3),
        freshness: None,
        country: None,
        language: None,
        domain_filter: Vec::new(),
    };
    let first = runtime
        .search(args.clone(), "auto-exhausted-1")
        .await
        .expect_err("first exhausted search should error");
    let second = runtime
        .search(args, "auto-exhausted-2")
        .await
        .expect_err("second exhausted search should also error");

    for err in [first, second] {
        let text = err.to_string();
        assert!(text.contains(&tr_in(
            Locale::En,
            "search.allUnavailableDetails",
            &[("query", "rust"), ("details", "")]
        )));
        assert!(text.contains("backend_unavailable:tavily"));
        assert!(text.contains("backend_unavailable:brave"));
    }
    assert_eq!(
        invoker.calls().len(),
        4,
        "exhausted auto errors should not be cached"
    );
}

#[test]
#[serial]
fn openai_bundle_sends_official_request_and_keeps_citations_first() {
    let actual = eval_plugin_script_value(
        r#"
      var request;
      tomcat.fetch = async function (args) {
        request = args;
        return {status: 200, body: JSON.stringify({output: [
          {type: "web_search_call", action: {sources: [{url: "https://docs.rs/cited"}, {url: "https://docs.rs/source"}]}},
          {type: "message", content: [{type: "output_text", text: "🦀Reqwest", annotations: [
            {type: "url_citation", title: "Reqwest", url: "https://docs.rs/cited", start_index: 1, end_index: 8}
          ]}]}
        ]})};
      };
      var result = await searchWithOpenAI({query: "rust", domainFilter: ["docs.rs"], country: "us"});
      return {request: request, hits: result.hits};
    "#,
    );
    assert_eq!(
        actual["request"],
        json!({
            "url": "https://api.openai.com/v1/responses", "method": "POST",
            "headers": {"Authorization": "Bearer {{secret:OPENAI_API_KEY}}", "Content-Type": "application/json"},
            "body": {"model": "gpt-6.2-sol", "input": "rust", "store": false, "tool_choice": "required",
                "include": ["web_search_call.action.sources"], "tools": [{"type": "web_search",
                "search_context_size": "medium", "filters": {"allowed_domains": ["docs.rs"]},
                "user_location": {"type": "approximate", "country": "US"}}]}
        })
    );
    assert_eq!(actual["hits"][0]["title"], "Reqwest");
    assert_eq!(actual["hits"][0]["snippet"], "Reqwest");
    assert_eq!(actual["hits"][1]["url"], "https://docs.rs/source");
    assert_eq!(actual["hits"].as_array().unwrap().len(), 2);
}

#[test]
#[serial]
fn openai_bundle_keeps_partial_links_and_fails_only_empty_unfinished_output() {
    let actual = eval_plugin_script_value(
        r#"
      async function run(payload) {
        tomcat.fetch = async function () { return {status: 200, body: JSON.stringify(payload)}; };
        try {
          return (await searchWithOpenAI({query: "rust"})).hits.map(function (hit) { return hit.url; });
        } catch (err) {
          return "error";
        }
      }
      return {
        partial: await run({status: "incomplete", output: [
          {type: "web_search_call", status: "failed", action: {sources: [{url: "https://docs.rs/source"}]}}
        ]}),
        unfinished: await run({status: "incomplete", output: []}),
        failedCall: await run({status: "completed", output: [{type: "web_search_call", status: "failed"}]}),
        empty: await run({status: "completed", output: []})
      };
    "#,
    );
    assert_eq!(
        actual,
        json!({
            "partial": ["https://docs.rs/source"],
            "unfinished": "error",
            "failedCall": "error",
            "empty": []
        })
    );
}

fn test_catalog(models_toml: &str) -> ModelCatalog {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("models.toml");
    std::fs::write(&path, models_toml).expect("write models.toml");
    ModelCatalog::load_from_path(&AppConfig::default(), path).expect("load model catalog")
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
