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
use crate::infra::AppConfig;

use super::backend::{
    discover_hosted_candidate, pick_backend, BackendFailure, BackendMode, BackendPlan,
    WebSearchBackend,
};
use super::cache::CacheKey;
use super::openai_server::{build_hosted_request_body, parse_server_tool_blocks};
use super::plugin_backend::{PluginSearchInvoker, PluginWebSearchBackend};
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

fn basic_web_search_request() -> super::types::WebSearchRequest {
    super::types::WebSearchRequest::from_tool_args(
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
    let request = super::types::WebSearchRequest::from_tool_args(
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
fn plugin_js_auto_backend_falls_through_after_retryable_warning() {
    let actual = eval_plugin_script_value(
        r#"
var calls = [];
backends.mimo = async function (req) {
  calls.push(req.backend);
  return {
    backend: "mimo",
    hits: [],
    warnings: ["__missing_key__:MIMO_API_KEY"]
  };
};
backends.tavily = async function (req) {
  calls.push(req.backend);
  return {
    backend: "tavily",
    hits: [{ title: "Reqwest", url: "https://docs.rs/reqwest", snippet: "HTTP client" }],
    warnings: []
  };
};
backends.brave = async function (req) {
  calls.push(req.backend);
  return { backend: "brave", hits: [], warnings: [] };
};
backends.serper = async function (req) {
  calls.push(req.backend);
  return { backend: "serper", hits: [], warnings: [] };
};
return {
  calls: calls,
  result: await dispatchBackend({ backend: "auto", query: "reqwest", count: 20 })
};
"#,
    );
    assert_eq!(actual["calls"], json!(["mimo", "tavily"]));
    assert_eq!(actual["result"]["backend"], json!("tavily"));
    assert_eq!(
        actual["result"]["hits"][0]["url"],
        json!("https://docs.rs/reqwest")
    );
    assert!(actual["result"]["warnings"]
        .as_array()
        .expect("warnings array")
        .iter()
        .any(|warning| warning == "backend_unavailable:mimo, fallback=tavily"));
}

#[tokio::test]
async fn plugin_backend_maps_unauthorized_warning_to_backend_failure() {
    let invoker = RecordingPluginInvoker::with_responses(vec![Ok(json!({
        "backend": "brave",
        "hits": [],
        "warnings": ["__unauthorized__:403"]
    }))]);
    let backend = PluginWebSearchBackend::new(invoker, "brave", "test-session");
    let request = super::types::WebSearchRequest::from_tool_args(
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
fn plugin_runtime_failure_is_non_retryable_and_formats_tool_error() {
    let failure = BackendFailure::PluginRuntime {
        detail: "plugin_backend_error (backend=mimo): synthetic runtime failure".to_string(),
    };
    assert!(
        !failure.is_retryable_unavailable(),
        "PluginRuntime should never enter auto fallback"
    );
    assert!(
        !failure.is_explicit_degraded(),
        "PluginRuntime should stay hard-fail even on explicit backends"
    );
    let err = failure.to_tool_error("auto");
    let text = err.to_string();
    assert!(text.contains("web_search backend `auto` 运行时错误"));
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
            plugin_slot,
        } => {
            assert!(hosted_candidate.is_none());
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
async fn explicit_builtin_aliases_route_to_plugin_by_default() {
    let invoker = RecordingPluginInvoker::with_responses(vec![
        Ok(json!({
            "backend": "tavily",
            "hits": [{ "title": "Tokio", "url": "https://tokio.rs" }],
            "warnings": []
        })),
        Ok(json!({
            "backend": "brave",
            "hits": [{ "title": "Reqwest", "url": "https://docs.rs/reqwest" }],
            "warnings": []
        })),
        Ok(json!({
            "backend": "serper",
            "hits": [{ "title": "Rust", "url": "https://www.rust-lang.org" }],
            "warnings": []
        })),
    ]);

    for backend in ["tavily", "brave", "serper"] {
        let mut cfg = AppConfig::default();
        cfg.tools.web_search.backend = backend.to_string();
        cfg.tools.web_search.tavily_base_url = "https://tavily.example.test".to_string();
        cfg.tools.web_search.brave_base_url = "https://brave.example.test".to_string();
        cfg.tools.web_search.serper_base_url = "https://serper.example.test".to_string();
        let runtime = runtime_with_catalog(cfg, None);
        runtime.set_plugin_invoker(invoker.clone());

        let output = runtime
            .search(
                WebSearchArgs {
                    query: "rust".into(),
                    count: Some(3),
                    freshness: None,
                    country: None,
                    language: None,
                    domain_filter: Vec::new(),
                },
                "session-plugin-alias",
            )
            .await
            .expect("plugin alias search");
        assert_eq!(output.backend, backend);
        assert_eq!(output.hits.len(), 1);
    }

    let calls = invoker.calls();
    assert_eq!(
        calls.iter().map(|call| call.0.as_str()).collect::<Vec<_>>(),
        vec!["tavily", "brave", "serper"]
    );
    for (_, payload, _) in &calls {
        assert_eq!(
            payload["tavilyBaseUrl"],
            json!("https://tavily.example.test")
        );
        assert_eq!(payload["braveBaseUrl"], json!("https://brave.example.test"));
        assert_eq!(
            payload["serperBaseUrl"],
            json!("https://serper.example.test")
        );
    }
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

    assert!(err
        .to_string()
        .contains("web_search plugin backend invoker not configured"));
}

#[tokio::test]
async fn explicit_plugin_backend_timeout_returns_tool_error() {
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
        .contains("web_search backend `mimo` 请求超时"));
}

#[tokio::test]
async fn explicit_plugin_backend_runtime_error_returns_original_detail() {
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
    assert!(text.contains("web_search backend `mimo` 运行时错误"));
    assert!(text.contains("synthetic runtime failure"));
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
async fn explicit_plugin_backend_rate_limit_returns_tool_error() {
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
    assert!(err
        .to_string()
        .contains("web_search backend `tavily` 暂不可用（status=429）"));
}

#[tokio::test]
#[serial]
async fn auto_exhausted_returns_tool_error_and_does_not_cache() {
    let invoker = RecordingPluginInvoker::with_responses(vec![
        Err(BackendFailure::Timeout),
        Err(BackendFailure::Timeout),
    ]);

    let mut cfg = AppConfig::default();
    cfg.tools.web_search.backend = "auto".into();
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
        assert!(text.contains("web_search 查询 `rust` 所有后端均不可用"));
        assert!(text.contains("backend_unavailable:auto"));
        assert!(text.contains("timeout (backend=auto)"));
    }
    assert_eq!(
        invoker.calls().len(),
        2,
        "exhausted auto errors should not be cached"
    );
}

#[tokio::test]
#[serial]
async fn auto_plugin_runtime_failure_fails_loud_without_all_backends_unavailable() {
    let _env = EnvGuard::set_many(&[
        ("OPENAI_API_KEY", None),
        ("TAVILY_API_KEY", None),
        ("BRAVE_API_KEY", None),
        ("SERPER_API_KEY", None),
    ]);
    let invoker = RecordingPluginInvoker::with_responses(vec![Ok(json!({
        "backend": "mimo",
        "hits": [],
        "warnings": [
            "plugin_backend_error (backend=mimo): TypeError: async hostcall requires a Tokio runtime handle"
        ],
        "unsupported_backend": true
    }))]);

    let mut cfg = AppConfig::default();
    cfg.tools.web_search.backend = "auto".into();
    let runtime = runtime_with_catalog(cfg, None);
    runtime.set_plugin_invoker(invoker);

    let err = runtime
        .search(
            WebSearchArgs {
                query: "rust".into(),
                count: Some(3),
                freshness: None,
                country: None,
                language: None,
                domain_filter: Vec::new(),
            },
            "auto-pluginruntime-1",
        )
        .await
        .expect_err("plugin runtime warnings should fail loud");
    let text = err.to_string();
    assert!(text.contains("web_search backend `auto` 运行时错误"));
    assert!(text.contains("async hostcall requires a Tokio runtime handle"));
    assert!(
        !text.contains("所有后端均不可用"),
        "plugin runtime failures should not be flattened into exhausted auto: {text}"
    );
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
        ("HTTPS_PROXY", None),
        ("HTTP_PROXY", None),
        ("ALL_PROXY", None),
        ("NO_PROXY", None),
        ("no_proxy", None),
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
async fn incompatible_hosted_candidate_falls_back_to_plugin_slot() {
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
        .any(|w| w == "hosted_candidate_unavailable, fallback=auto"));
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
