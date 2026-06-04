use std::time::Duration;

use reqwest::redirect::Policy;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::super::fetcher::fetch_url;
use super::super::types::{WebFetchFormat, WebFetchRequest};
use super::{test_config, validated_url};

fn mapped_url(server: &MockServer, host: &str, path_suffix: &str) -> String {
    format!("http://{host}:{}{}", server.address().port(), path_suffix)
}

fn client_for_server(
    cfg: &crate::AppConfig,
    server: &MockServer,
    hosts: &[&str],
) -> reqwest::Client {
    let mut builder = reqwest::Client::builder()
        .no_proxy()
        .redirect(Policy::none())
        .timeout(Duration::from_millis(cfg.tools.web_fetch.fetch_timeout_ms));
    for host in hosts {
        builder = builder.resolve(host, *server.address());
    }
    builder.build().expect("build test client")
}

fn markdown_request(url: &str) -> WebFetchRequest {
    WebFetchRequest {
        raw_url: url.to_string(),
        prompt: None,
        format: WebFetchFormat::Markdown,
    }
}

#[tokio::test]
async fn redirect_same_host_followed() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/start"))
        .respond_with(ResponseTemplate::new(301).insert_header("Location", "/landing"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/landing"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/html; charset=utf-8")
                .set_body_string(
                    "<html><body><article><h1>Hello</h1><p>World</p></article></body></html>",
                ),
        )
        .mount(&server)
        .await;

    let cfg = test_config();
    let client = client_for_server(&cfg, &server, &["example.test"]);
    let persist_dir = tempfile::tempdir().unwrap();
    let url = mapped_url(&server, "example.test", "/start");
    let output = fetch_url(
        &client,
        persist_dir.path(),
        &cfg.tools.web_fetch,
        &markdown_request(&url),
        &validated_url(&url),
    )
    .await
    .expect("fetch");

    assert_eq!(output.url, mapped_url(&server, "example.test", "/landing"));
    assert!(output.result.contains("Hello"));
    assert!(output.redirect.is_none());
}

#[tokio::test]
async fn redirect_apex_to_www_followed() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/start"))
        .respond_with(ResponseTemplate::new(302).insert_header(
            "Location",
            mapped_url(&server, "www.example.test", "/landing"),
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/landing"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/html; charset=utf-8")
                .set_body_string(
                    "<html><body><article><p>www followed</p></article></body></html>",
                ),
        )
        .mount(&server)
        .await;

    let cfg = test_config();
    let client = client_for_server(&cfg, &server, &["example.test", "www.example.test"]);
    let persist_dir = tempfile::tempdir().unwrap();
    let start_url = mapped_url(&server, "example.test", "/start");
    let output = fetch_url(
        &client,
        persist_dir.path(),
        &cfg.tools.web_fetch,
        &markdown_request(&start_url),
        &validated_url(&start_url),
    )
    .await
    .expect("fetch");

    assert_eq!(
        output.url,
        mapped_url(&server, "www.example.test", "/landing")
    );
    assert!(output.result.contains("www followed"));
}

#[tokio::test]
async fn redirect_www_to_apex_followed() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/start"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("Location", mapped_url(&server, "example.test", "/landing")),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/landing"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/html; charset=utf-8")
                .set_body_string(
                    "<html><body><article><p>apex followed</p></article></body></html>",
                ),
        )
        .mount(&server)
        .await;

    let cfg = test_config();
    let client = client_for_server(&cfg, &server, &["example.test", "www.example.test"]);
    let persist_dir = tempfile::tempdir().unwrap();
    let start_url = mapped_url(&server, "www.example.test", "/start");
    let output = fetch_url(
        &client,
        persist_dir.path(),
        &cfg.tools.web_fetch,
        &markdown_request(&start_url),
        &validated_url(&start_url),
    )
    .await
    .expect("fetch");

    assert_eq!(output.url, mapped_url(&server, "example.test", "/landing"));
    assert!(output.result.contains("apex followed"));
}

#[tokio::test]
async fn redirect_off_host_returns_structured() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/start"))
        .respond_with(
            ResponseTemplate::new(301)
                .insert_header("Location", "https://newsite.example/new-path"),
        )
        .mount(&server)
        .await;

    let cfg = test_config();
    let client = client_for_server(&cfg, &server, &["example.test"]);
    let persist_dir = tempfile::tempdir().unwrap();
    let url = mapped_url(&server, "example.test", "/start");
    let output = fetch_url(
        &client,
        persist_dir.path(),
        &cfg.tools.web_fetch,
        &markdown_request(&url),
        &validated_url(&url),
    )
    .await
    .expect("fetch");

    assert_eq!(output.url, url);
    assert!(output.result.is_empty());
    assert!(output
        .warnings
        .iter()
        .any(|warning| warning == "redirect_off_host"));
    let redirect = output.redirect.expect("redirect info");
    assert_eq!(redirect.original_url, url);
    assert_eq!(redirect.redirect_url, "https://newsite.example/new-path");
}

#[tokio::test]
async fn redirect_loop_over_10_returns_err() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/loop"))
        .respond_with(ResponseTemplate::new(302).insert_header("Location", "/loop"))
        .mount(&server)
        .await;

    let mut cfg = test_config();
    cfg.tools.web_fetch.max_redirects = 3;
    let client = client_for_server(&cfg, &server, &["example.test"]);
    let persist_dir = tempfile::tempdir().unwrap();
    let url = mapped_url(&server, "example.test", "/loop");
    let err = fetch_url(
        &client,
        persist_dir.path(),
        &cfg.tools.web_fetch,
        &markdown_request(&url),
        &validated_url(&url),
    )
    .await
    .expect_err("loop should fail");
    assert!(err.to_string().contains("redirect loop"));
}

#[tokio::test]
async fn markdown_under_threshold_inlined() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/page"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/html; charset=utf-8")
                .set_body_string(
                    "<html><body><article><h1>Hello</h1><p>tiny body</p></article></body></html>",
                ),
        )
        .mount(&server)
        .await;

    let cfg = test_config();
    let client = client_for_server(&cfg, &server, &["example.test"]);
    let persist_dir = tempfile::tempdir().unwrap();
    let url = mapped_url(&server, "example.test", "/page");
    let output = fetch_url(
        &client,
        persist_dir.path(),
        &cfg.tools.web_fetch,
        &markdown_request(&url),
        &validated_url(&url),
    )
    .await
    .expect("fetch");

    assert!(output.result.contains("Hello"));
    assert!(output.persisted_output_path.is_none());
    assert_eq!(output.total_chars, output.result.chars().count() as u64);
}

#[tokio::test]
async fn markdown_over_threshold_persisted_with_head() {
    let server = MockServer::start().await;
    let body = format!(
        "<html><body><article><p>{}</p></article></body></html>",
        "very long markdown body ".repeat(20)
    );
    Mock::given(method("GET"))
        .and(path("/long"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/html; charset=utf-8")
                .set_body_string(body),
        )
        .mount(&server)
        .await;

    let mut cfg = test_config();
    cfg.tools.web_fetch.max_markdown_chars = 32;
    cfg.tools.web_fetch.markdown_head_chars = 12;
    let client = client_for_server(&cfg, &server, &["example.test"]);
    let persist_dir = tempfile::tempdir().unwrap();
    let url = mapped_url(&server, "example.test", "/long");
    let output = fetch_url(
        &client,
        persist_dir.path(),
        &cfg.tools.web_fetch,
        &markdown_request(&url),
        &validated_url(&url),
    )
    .await
    .expect("fetch");

    let path = output.persisted_output_path.expect("persisted path");
    assert!(path.ends_with(".md"));
    assert!(std::path::Path::new(&path).exists());
    assert!(output
        .warnings
        .iter()
        .any(|warning| warning == "markdown_persisted"));
    assert!(output.result.contains("...full markdown persisted to"));
    assert!(!output.truncated);
}

#[tokio::test]
async fn pdf_persisted_to_tool_results() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/paper.pdf"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "application/pdf")
                .set_body_bytes(b"%PDF-1.7\nfake pdf".to_vec()),
        )
        .mount(&server)
        .await;

    let cfg = test_config();
    let client = client_for_server(&cfg, &server, &["example.test"]);
    let persist_dir = tempfile::tempdir().unwrap();
    let url = mapped_url(&server, "example.test", "/paper.pdf");
    let output = fetch_url(
        &client,
        persist_dir.path(),
        &cfg.tools.web_fetch,
        &markdown_request(&url),
        &validated_url(&url),
    )
    .await
    .expect("fetch");

    let path = output.persisted_output_path.expect("persisted path");
    assert!(path.ends_with(".pdf"));
    assert!(output.result.is_empty());
    assert!(output
        .warnings
        .iter()
        .any(|warning| warning == "binary_persisted"));
}

#[tokio::test]
async fn magic_overrides_content_type_when_mismatch() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/wrong-header"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/plain")
                .set_body_bytes(b"%PDF-1.7\nfake pdf".to_vec()),
        )
        .mount(&server)
        .await;

    let cfg = test_config();
    let client = client_for_server(&cfg, &server, &["example.test"]);
    let persist_dir = tempfile::tempdir().unwrap();
    let url = mapped_url(&server, "example.test", "/wrong-header");
    let output = fetch_url(
        &client,
        persist_dir.path(),
        &cfg.tools.web_fetch,
        &markdown_request(&url),
        &validated_url(&url),
    )
    .await
    .expect("fetch");

    assert_eq!(output.content_type, "application/pdf");
    let path = output.persisted_output_path.expect("persisted path");
    assert!(path.ends_with(".pdf"));
}

#[tokio::test]
async fn http_429_returns_warning_not_err() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/429"))
        .respond_with(ResponseTemplate::new(429))
        .mount(&server)
        .await;

    let cfg = test_config();
    let client = client_for_server(&cfg, &server, &["example.test"]);
    let persist_dir = tempfile::tempdir().unwrap();
    let url = mapped_url(&server, "example.test", "/429");
    let output = fetch_url(
        &client,
        persist_dir.path(),
        &cfg.tools.web_fetch,
        &markdown_request(&url),
        &validated_url(&url),
    )
    .await
    .expect("fetch");

    assert_eq!(output.code, 429);
    assert!(output.truncated);
    assert!(output
        .warnings
        .iter()
        .any(|warning| warning.starts_with("rate_limited")));
}

#[tokio::test]
async fn http_5xx_returns_warning_not_err() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/500"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;

    let cfg = test_config();
    let client = client_for_server(&cfg, &server, &["example.test"]);
    let persist_dir = tempfile::tempdir().unwrap();
    let url = mapped_url(&server, "example.test", "/500");
    let output = fetch_url(
        &client,
        persist_dir.path(),
        &cfg.tools.web_fetch,
        &markdown_request(&url),
        &validated_url(&url),
    )
    .await
    .expect("fetch");

    assert_eq!(output.code, 503);
    assert!(output.truncated);
    assert!(output
        .warnings
        .iter()
        .any(|warning| warning.starts_with("server_error")));
}

#[tokio::test]
async fn fetch_timeout_returns_truncated_warning() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/slow"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_millis(200))
                .set_body_string("<html><body><p>slow</p></body></html>"),
        )
        .mount(&server)
        .await;

    let mut cfg = test_config();
    cfg.tools.web_fetch.fetch_timeout_ms = 50;
    let client = client_for_server(&cfg, &server, &["example.test"]);
    let persist_dir = tempfile::tempdir().unwrap();
    let url = mapped_url(&server, "example.test", "/slow");
    let output = fetch_url(
        &client,
        persist_dir.path(),
        &cfg.tools.web_fetch,
        &markdown_request(&url),
        &validated_url(&url),
    )
    .await
    .expect("fetch");

    assert!(output.truncated);
    assert!(output.warnings.iter().any(|warning| warning == "timeout"));
}
