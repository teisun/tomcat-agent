use std::collections::BTreeMap;
use std::time::Duration;

use serial_test::serial;

use super::mocks::{MockHttpServer, ScriptedHttpResponse};
use crate::core::llm::auth::{
    clear_managed_credentials_for_test, refresh_managed_credentials_from_entries,
};
use crate::core::llm::remote_models::{
    list_remote_models, list_remote_models_within, RemoteModelsFailure, RemoteModelsFailureReason,
    RemoteModelsRequest,
};
use crate::infra::config::LlmRuntimeConfig;

const TEMP_KEY: &str = "sk-temp-dialog-key-123456";
const SAVED_KEY: &str = "sk-saved-slot-key-654321";

fn request(base_url: &str, api: &str) -> RemoteModelsRequest {
    RemoteModelsRequest {
        base_url: base_url.to_string(),
        api: api.to_string(),
        api_key: Some(TEMP_KEY.to_string()),
        api_key_env: None,
    }
}

fn header_value<'a>(request_text: &'a str, name: &str) -> Option<&'a str> {
    request_text.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim().eq_ignore_ascii_case(name).then(|| value.trim())
    })
}

fn request_line(request_text: &str) -> &str {
    request_text.lines().next().unwrap_or_default()
}

fn assert_failure(
    result: Result<Vec<impl std::fmt::Debug>, RemoteModelsFailure>,
    reason: RemoteModelsFailureReason,
) -> RemoteModelsFailure {
    let failure = result.expect_err("listing should fail");
    assert_eq!(failure.reason, reason);
    let serialized = serde_json::to_string(&failure).unwrap();
    assert!(
        !serialized.contains(TEMP_KEY),
        "failure payload leaked the key: {serialized}"
    );
    assert!(!failure.message().contains(TEMP_KEY));
    failure
}

#[tokio::test]
async fn openai_compatible_listing_uses_bearer_and_returns_deduplicated_names() {
    let server = MockHttpServer::start(vec![ScriptedHttpResponse::json(
        200,
        r#"{"object":"list","data":[{"id":"deepseek-v4-pro"},{"id":"deepseek-v4-flash"},{"id":"deepseek-v4-pro"},{"id":"  "}]}"#,
    )])
    .await;

    let models = list_remote_models(
        &LlmRuntimeConfig::default(),
        request(&server.base_url, "openai"),
    )
    .await
    .expect("listing succeeds");

    let ids: Vec<_> = models.iter().map(|model| model.id.as_str()).collect();
    assert_eq!(ids, ["deepseek-v4-pro", "deepseek-v4-flash"]);
    let texts = server.request_texts();
    assert_eq!(request_line(&texts[0]), "GET /v1/models HTTP/1.1");
    assert_eq!(
        header_value(&texts[0], "authorization"),
        Some(format!("Bearer {TEMP_KEY}").as_str())
    );
    assert_eq!(header_value(&texts[0], "x-api-key"), None);
    server.shutdown().await;
}

#[tokio::test]
async fn anthropic_listing_uses_x_api_key_version_and_page_limit() {
    let server = MockHttpServer::start(vec![ScriptedHttpResponse::json(
        200,
        r#"{"data":[{"id":"claude-opus-5-5","type":"model"}],"has_more":false}"#,
    )])
    .await;

    let models = list_remote_models(
        &LlmRuntimeConfig::default(),
        request(&server.base_url, "anthropic-messages"),
    )
    .await
    .expect("listing succeeds");

    assert_eq!(models.len(), 1);
    let texts = server.request_texts();
    assert_eq!(
        request_line(&texts[0]),
        "GET /v1/models?limit=1000 HTTP/1.1"
    );
    assert_eq!(header_value(&texts[0], "x-api-key"), Some(TEMP_KEY));
    assert_eq!(
        header_value(&texts[0], "anthropic-version"),
        Some("2023-06-01")
    );
    assert_eq!(header_value(&texts[0], "authorization"), None);
    server.shutdown().await;
}

#[tokio::test]
async fn path_aware_base_url_keeps_its_path() {
    let server =
        MockHttpServer::start(vec![ScriptedHttpResponse::json(200, r#"{"data":[]}"#)]).await;

    let models = list_remote_models(
        &LlmRuntimeConfig::default(),
        request(&format!("{}/coding/v1/", server.base_url), "openai"),
    )
    .await
    .expect("empty list is still a list");

    assert!(models.is_empty());
    assert_eq!(
        request_line(&server.request_texts()[0]),
        "GET /coding/v1/models HTTP/1.1"
    );
    server.shutdown().await;
}

#[tokio::test]
async fn http_failures_are_classified_without_echoing_the_key() {
    let server = MockHttpServer::start(vec![
        ScriptedHttpResponse::json(401, r#"{"error":"invalid key"}"#),
        ScriptedHttpResponse::json(403, r#"{"error":"forbidden"}"#),
        ScriptedHttpResponse::json(404, r#"{"error":"not found"}"#),
        ScriptedHttpResponse::json(405, r#"{"error":"method"}"#),
        ScriptedHttpResponse::json(502, r#"{"error":"bad gateway"}"#),
        ScriptedHttpResponse::json(200, "<html>not json</html>"),
        ScriptedHttpResponse::json(200, r#"{"models":["a"]}"#),
    ])
    .await;
    let runtime = LlmRuntimeConfig::default();
    let list = || list_remote_models(&runtime, request(&server.base_url, "openai"));

    let auth = assert_failure(list().await, RemoteModelsFailureReason::Auth);
    assert_eq!(auth.status, Some(401));
    assert_eq!(auth.message(), "remote_models_auth: HTTP 401");
    assert_failure(list().await, RemoteModelsFailureReason::Auth);
    let unsupported = assert_failure(list().await, RemoteModelsFailureReason::Unsupported);
    assert_eq!(unsupported.status, Some(404));
    assert_failure(list().await, RemoteModelsFailureReason::Unsupported);
    assert_failure(list().await, RemoteModelsFailureReason::Network);
    assert_failure(list().await, RemoteModelsFailureReason::Format);
    assert_failure(list().await, RemoteModelsFailureReason::Format);
    server.shutdown().await;
}

#[tokio::test]
async fn slow_endpoint_is_reported_as_timeout() {
    let mut slow = ScriptedHttpResponse::json(200, r#"{"data":[]}"#);
    slow.delay_ms = 2_000;
    let server = MockHttpServer::start(vec![slow]).await;

    let result = list_remote_models_within(
        &LlmRuntimeConfig::default(),
        request(&server.base_url, "openai"),
        Duration::from_millis(200),
    )
    .await;

    assert_failure(result, RemoteModelsFailureReason::Timeout);
    server.shutdown().await;
}

#[tokio::test]
async fn unreachable_address_is_a_network_failure() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);

    let result = list_remote_models(
        &LlmRuntimeConfig::default(),
        request(&format!("http://{address}"), "openai"),
    )
    .await;

    assert_failure(result, RemoteModelsFailureReason::Network);
}

#[tokio::test]
#[serial(env_lock)]
async fn saved_key_is_read_from_slot_when_no_temporary_key_is_given() {
    clear_managed_credentials_for_test();
    refresh_managed_credentials_from_entries(&BTreeMap::from([(
        "REMOTE_MODELS_TEST_API_KEY".to_string(),
        SAVED_KEY.to_string(),
    )]));
    let server = MockHttpServer::start(vec![ScriptedHttpResponse::json(
        200,
        r#"{"data":[{"id":"m"}]}"#,
    )])
    .await;

    let result = list_remote_models(
        &LlmRuntimeConfig::default(),
        RemoteModelsRequest {
            base_url: server.base_url.clone(),
            api: "openai".to_string(),
            api_key: Some("   ".to_string()),
            api_key_env: Some("REMOTE_MODELS_TEST_API_KEY".to_string()),
        },
    )
    .await;

    clear_managed_credentials_for_test();
    assert_eq!(result.expect("listing succeeds").len(), 1);
    assert_eq!(
        header_value(&server.request_texts()[0], "authorization"),
        Some(format!("Bearer {SAVED_KEY}").as_str())
    );
    server.shutdown().await;
}
