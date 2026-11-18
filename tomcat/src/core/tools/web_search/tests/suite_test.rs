use async_trait::async_trait;
use serde_json::json;
use serial_test::serial;
use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::core::llm::ModelCatalog;
use crate::ext::PluginEngine;
use crate::infra::i18n::{tr_in, Locale};
use crate::infra::AppConfig;

use super::super::backend::{discover_hosted_candidate, BackendFailure, WebSearchBackend};
use super::super::cache::CacheKey;
use super::super::declarations::{parse_declarations, BackendDeclaration};
use super::super::openai_server::{build_hosted_request_body, parse_server_tool_blocks};
use super::super::plugin_backend::{PluginSearchInvoker, PluginWebSearchBackend};
use super::super::types::{normalize_hits, RawHit, WebSearchArgs};
use super::super::WebSearchRuntime;

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
    fn backend_declarations(&self) -> Vec<BackendDeclaration> {
        ["tavily", "brave", "mimo"]
            .into_iter()
            .map(|id| BackendDeclaration {
                id: id.into(),
                label: id.into(),
                secret: Some("TEST_WEB_SEARCH_KEY".into()),
                model: None,
                host: None,
                default_auto: true,
            })
            .collect()
    }

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

fn basic_web_search_request() -> super::super::types::WebSearchRequest {
    super::super::types::WebSearchRequest::from_tool_args(
        WebSearchArgs {
            query: "rust".into(),
            count: None,
            freshness: None,
            country: None,
            language: None,
            domain_filter: Vec::new(),
        },
        &AppConfig::default().tools.web_search,
    )
    .expect("request")
}

#[tokio::test]
async fn plugin_backend_maps_missing_key_warning_to_backend_failure() {
    let invoker = RecordingPluginInvoker::with_responses(vec![Ok(json!({
        "backend": "tavily",
        "hits": [],
        "warnings": ["__missing_key__:TAVILY_API_KEY"]
    }))]);
    let backend = PluginWebSearchBackend::new(invoker, "tavily", "test-session");
    let request = super::super::types::WebSearchRequest::from_tool_args(
        WebSearchArgs {
            query: "rust".into(),
            count: None,
            freshness: None,
            country: None,
            language: None,
            domain_filter: Vec::new(),
        },
        &AppConfig::default().tools.web_search,
    )
    .expect("request");

    let err = backend
        .search(&request)
        .await
        .expect_err("missing key should fail");
    assert!(matches!(
        err,
        BackendFailure::MissingKey { env_name } if env_name == "TAVILY_API_KEY"
    ));
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
var tooLong = await dispatchBackend({backend: 'deepseek', query: '🌱'.repeat(129)});
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
        actual["tooLong"]["warnings"],
        json!(["deepseek_query_too_long"])
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

#[tokio::test]
async fn plugin_backend_maps_unauthorized_warning_to_backend_failure() {
    let invoker = RecordingPluginInvoker::with_responses(vec![Ok(json!({
        "backend": "brave",
        "hits": [],
        "warnings": ["__unauthorized__:403"]
    }))]);
    let backend = PluginWebSearchBackend::new(invoker, "brave", "test-session");
    let request = super::super::types::WebSearchRequest::from_tool_args(
        WebSearchArgs {
            query: "rust".into(),
            count: None,
            freshness: None,
            country: None,
            language: None,
            domain_filter: Vec::new(),
        },
        &AppConfig::default().tools.web_search,
    )
    .expect("request");

    let err = backend
        .search(&request)
        .await
        .expect_err("unauthorized should fail");
    assert!(matches!(err, BackendFailure::Unauthorized { status } if status == 403));
}

#[tokio::test]
async fn plugin_backend_maps_plugin_backend_error_to_plugin_runtime_failure() {
    let invoker = RecordingPluginInvoker::with_responses(vec![Ok(json!({
        "backend": "mimo",
        "hits": [],
        "warnings": [
            "plugin_backend_error (backend=mimo): TypeError: async hostcall requires a Tokio runtime handle"
        ]
    }))]);
    let backend = PluginWebSearchBackend::new(invoker, "mimo", "test-session");

    let err = backend
        .search(&basic_web_search_request())
        .await
        .expect_err("plugin runtime warnings should fail loud");
    match err {
        BackendFailure::PluginRuntime { detail } => {
            assert!(detail.contains("plugin_backend_error (backend=mimo)"));
            assert!(detail.contains("async hostcall requires a Tokio runtime handle"));
        }
        other => panic!("expected PluginRuntime, got {other:?}"),
    }
}

#[test]
fn plugin_runtime_failure_formats_tool_error() {
    let failure = BackendFailure::PluginRuntime {
        detail: "plugin_backend_error (backend=mimo): synthetic runtime failure".to_string(),
    };
    let err = failure.to_tool_error("auto");
    let text = err.to_string();
    assert!(text.contains(&tr_in(
        Locale::En,
        "search.runtime",
        &[("backend", "auto"), ("detail", "")]
    )));
    assert!(text.contains("synthetic runtime failure"));
}

#[tokio::test]
async fn plugin_backend_plugin_runtime_warning_wins_over_missing_key() {
    let invoker = RecordingPluginInvoker::with_responses(vec![Ok(json!({
        "backend": "mimo",
        "hits": [],
        "warnings": [
            "__missing_key__:MIMO_API_KEY",
            "plugin_backend_error (backend=mimo): TypeError: async hostcall requires a Tokio runtime handle"
        ]
    }))]);
    let backend = PluginWebSearchBackend::new(invoker, "mimo", "test-session");

    let err = backend
        .search(&basic_web_search_request())
        .await
        .expect_err("plugin runtime warnings should outrank retryable sentinels");
    assert!(
        matches!(err, BackendFailure::PluginRuntime { .. }),
        "expected PluginRuntime precedence, got {err:?}"
    );
}

#[tokio::test]
async fn plugin_backend_timeout_warning_stays_retryable_on_exhausted_auto_response() {
    let invoker = RecordingPluginInvoker::with_responses(vec![Ok(json!({
        "backend": "auto",
        "hits": [],
        "warnings": [
            "backend_unavailable:tavily, fallback=brave",
            "plugin_backend_error (backend=tavily): Error: pi.fetch request timed out"
        ],
        "unsupported_backend": true
    }))]);
    let backend = PluginWebSearchBackend::new(invoker, "auto", "test-session");

    let err = backend
        .search(&basic_web_search_request())
        .await
        .expect_err("timeout warnings on exhausted auto should remain retryable");
    assert!(
        matches!(err, BackendFailure::Timeout),
        "expected Timeout, got {err:?}"
    );
}

#[tokio::test]
async fn plugin_backend_timeout_warning_allows_successful_auto_fallback() {
    let invoker = RecordingPluginInvoker::with_responses(vec![Ok(json!({
        "backend": "brave",
        "hits": [
            {
                "title": "reqwest",
                "url": "https://docs.rs/reqwest",
                "snippet": "HTTP client"
            }
        ],
        "warnings": [
            "backend_unavailable:tavily, fallback=brave",
            "plugin_backend_error (backend=tavily): Error: pi.fetch request timed out"
        ]
    }))]);
    let backend = PluginWebSearchBackend::new(invoker, "auto", "test-session");

    let output = backend
        .search(&basic_web_search_request())
        .await
        .expect("timeout warnings should not fail a later successful fallback");
    assert_eq!(output.backend_label.as_deref(), Some("brave"));
    assert_eq!(output.raw_hits.len(), 1);
    assert!(output
        .warnings
        .iter()
        .any(|warning| warning.contains("pi.fetch request timed out")));
}

#[tokio::test]
async fn plugin_backend_bare_unsupported_backend_includes_warning_summary() {
    let invoker = RecordingPluginInvoker::with_responses(vec![Ok(json!({
        "backend": "mimo",
        "hits": [],
        "warnings": ["shadowed_provider"],
        "unsupported_backend": true
    }))]);
    let backend = PluginWebSearchBackend::new(invoker, "mimo", "test-session");

    let err = backend
        .search(&basic_web_search_request())
        .await
        .expect_err("unsupported_backend should stay incompatible");
    match err {
        BackendFailure::Incompatible { detail } => {
            assert!(detail.contains("reported unsupported_backend"));
            assert!(detail.contains("shadowed_provider"));
        }
        other => panic!("expected Incompatible, got {other:?}"),
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
id = "gpt-5.6"

[models.capabilities]
web_search = true
"#,
    )
    .expect("write models.toml");

    let cfg = AppConfig::default();
    let catalog = ModelCatalog::load_from_path(&cfg, path).expect("load catalog");
    let candidate = discover_hosted_candidate(&catalog).expect("hosted candidate");
    assert_eq!(candidate.id, "gpt-5.6");
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
fn build_hosted_request_body_includes_filters_and_location() {
    let request = super::super::types::WebSearchRequest::from_tool_args(
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
fn declarations_ignore_invalid_and_duplicate_entries() {
    let metadata = json!({"backends": [
        {"id":"bespoke","label":"Bespoke","secret":"SEARCH_TEST_KEY","defaultAuto":true},
        {"id":"bespoke","label":"Duplicate","model":"id"},
        {"id":"openai","label":"Reserved","model":"id"},
        {"id":"bad","label":"Bad","secret":"lowercase"},
        {"id":"conflict","label":"Conflict","secret":"KEY","model":"id"},
        {"id":"no-key","label":"Missing dependency"}
    ]});
    let declarations = parse_declarations(metadata.as_object());
    assert_eq!(declarations.len(), 1);
    assert_eq!(
        super::super::declarations::effective_order(None, &declarations),
        ["openai", "bespoke"]
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
async fn ordered_route_skips_missing_and_stops_on_empty_with_hot_updates() {
    let _env = EnvGuard::set_many(&[("TEST_WEB_SEARCH_KEY", Some("fixture"))]);
    let invoker = RecordingPluginInvoker::with_responses(vec![
        Err(BackendFailure::RateLimited { status: 429 }),
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
    assert!(invoker
        .calls()
        .iter()
        .all(|call| call.1.get("tavilyBaseUrl").is_none()));
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
async fn explicit_plugin_backend_timeout_returns_tool_error() {
    let _env = EnvGuard::set_many(&[("TEST_WEB_SEARCH_KEY", Some("fixture"))]);
    let invoker = RecordingPluginInvoker::with_responses(vec![Err(BackendFailure::Timeout)]);

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
            "session-plugin-3",
        )
        .await
        .expect_err("timeout should surface as tool error");
    assert!(err
        .to_string()
        .contains(&tr_in(Locale::En, "search.timeout", &[("backend", "mimo")])));
}

#[tokio::test]
#[serial]
async fn explicit_plugin_backend_runtime_error_returns_original_detail() {
    let _env = EnvGuard::set_many(&[("TEST_WEB_SEARCH_KEY", Some("fixture"))]);
    let invoker =
        RecordingPluginInvoker::with_responses(vec![Err(BackendFailure::PluginRuntime {
            detail: "plugin_backend_error (backend=mimo): synthetic runtime failure".to_string(),
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
            "session-plugin-pluginruntime",
        )
        .await
        .expect_err("plugin runtime errors should preserve original detail");
    let text = err.to_string();
    assert!(text.contains(&tr_in(
        Locale::En,
        "search.runtime",
        &[("backend", "mimo"), ("detail", "")]
    )));
    assert!(text.contains("synthetic runtime failure"));
}

#[tokio::test]
#[serial]
async fn explicit_plugin_backend_unsupported_error_is_preserved() {
    let _env = EnvGuard::set_many(&[("TEST_WEB_SEARCH_KEY", Some("fixture"))]);
    let detail = "未找到名为 `mimo` 的 web_search 插件后端";
    let invoker = RecordingPluginInvoker::with_responses(vec![Err(BackendFailure::Incompatible {
        detail: detail.to_string(),
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

    assert!(err.to_string().contains(detail));
}

#[tokio::test]
#[serial]
async fn explicit_plugin_backend_rate_limit_returns_tool_error() {
    let _env = EnvGuard::set_many(&[("TEST_WEB_SEARCH_KEY", Some("fixture"))]);
    let invoker = RecordingPluginInvoker::with_responses(vec![Err(BackendFailure::RateLimited {
        status: 429,
    })]);

    let mut cfg = AppConfig::default();
    cfg.tools.web_search.backend = "tavily".into();
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
            "test-session",
        )
        .await
        .expect_err("rate-limited plugin search should fail clearly");
    assert!(err.to_string().contains(&tr_in(
        Locale::En,
        "search.unavailable",
        &[("backend", "tavily"), ("status", "429")]
    )));
}

#[tokio::test]
#[serial]
async fn auto_exhausted_returns_tool_error_and_does_not_cache() {
    let _env = EnvGuard::set_many(&[("TEST_WEB_SEARCH_KEY", Some("fixture"))]);
    let invoker = RecordingPluginInvoker::with_responses(vec![Err(BackendFailure::Timeout); 4]);
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

#[tokio::test]
#[serial]
async fn web_search_sends_request_model_name_not_id() {
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
        ("HTTPS_PROXY", None),
        ("HTTP_PROXY", None),
        ("ALL_PROXY", None),
        // Local mock traffic must also bypass proxies discovered from OS settings.
        ("NO_PROXY", Some("127.0.0.1,localhost,::1")),
        ("no_proxy", Some("127.0.0.1,localhost,::1")),
    ]);

    let mut cfg = AppConfig::default();
    cfg.tools.web_search.backend = "auto".into();
    let runtime = runtime_with_catalog(
        cfg,
        Some(format!(
            r#"
[[models]]
id = "tomcat-catalog-web-search-model"
model_name = "provider-facing-web-search-model"
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
    assert_eq!(body["model"], json!("provider-facing-web-search-model"));
    assert_eq!(body["tools"][0]["type"], json!("web_search"));
    assert_eq!(
        body["tools"][0]["filters"]["allowed_domains"],
        json!(["docs.rs"])
    );
    assert_eq!(body["tools"][0]["user_location"]["country"], json!("US"));
}

#[tokio::test]
#[serial]
async fn incompatible_hosted_candidate_falls_back_to_plugin_slot() {
    let _env = EnvGuard::set_many(&[("TEST_WEB_SEARCH_KEY", Some("fixture"))]);
    let invoker = RecordingPluginInvoker::with_responses(vec![Ok(json!({
        "backend": "tavily",
        "hits": [
            {
                "title": "Rust",
                "url": "https://www.rust-lang.org",
                "snippet": "Language homepage"
            }
        ],
        "warnings": []
    }))]);

    let mut cfg = AppConfig::default();
    cfg.tools.web_search.backend = "auto".into();
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
    runtime.set_plugin_invoker(invoker);

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
        .any(|w| w.starts_with("backend_unavailable:openai")));
}

#[tokio::test]
#[serial]
async fn explicit_openai_requires_project_candidate() {
    let _env = EnvGuard::set_many(&[
        ("TAVILY_API_KEY", None),
        ("BRAVE_API_KEY", None),
        ("SERPER_API_KEY", None),
        ("OPENAI_API_KEY", None),
        ("HTTPS_PROXY", None),
        ("HTTP_PROXY", None),
        ("ALL_PROXY", None),
        ("NO_PROXY", None),
        ("no_proxy", None),
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
        .to_lowercase()
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
