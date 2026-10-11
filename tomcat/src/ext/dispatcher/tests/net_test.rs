use super::{
    fetch_error_with_optional_details, is_allowed_host, parse_fetch_spec, resolve_secret_value,
};
use crate::infra::AppError;
use serde_json::{json, Value};
use std::sync::{LazyLock, Mutex};

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

struct EnvGuard {
    saved: Vec<(String, Option<String>)>,
}

impl EnvGuard {
    fn set_many(entries: &[(&str, Option<&str>)]) -> Self {
        let saved = entries
            .iter()
            .map(|(key, value)| {
                let previous = std::env::var(key).ok();
                match value {
                    Some(next) => std::env::set_var(key, next),
                    None => std::env::remove_var(key),
                }
                ((*key).to_string(), previous)
            })
            .collect();
        Self { saved }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        while let Some((key, value)) = self.saved.pop() {
            match value {
                Some(previous) => std::env::set_var(&key, previous),
                None => std::env::remove_var(&key),
            }
        }
    }
}

fn error_json(err: AppError) -> Value {
    match err {
        AppError::Plugin(raw) => serde_json::from_str(&raw).expect("plugin errors should be JSON"),
        other => panic!("expected AppError::Plugin JSON, got {other:?}"),
    }
}

#[test]
fn resolve_secret_value_replaces_nested_placeholders() {
    let _guard = ENV_LOCK.lock().unwrap();
    let _env = EnvGuard::set_many(&[
        ("BRAVE_API_KEY", Some("brave-secret")),
        ("SERPER_API_KEY", Some("serper-secret")),
    ]);
    let value = json!({
        "headers": {
            "Authorization": "Bearer {{secret:BRAVE_API_KEY}}"
        },
        "body": [
            "prefix-{{secret:SERPER_API_KEY}}-suffix",
            3,
            true
        ]
    });

    let resolved = resolve_secret_value(
        &value,
        &["BRAVE_API_KEY".to_string(), "SERPER_API_KEY".to_string()],
    )
    .expect("secrets should resolve");

    assert_eq!(
        resolved,
        json!({
            "headers": {
                "Authorization": "Bearer brave-secret"
            },
            "body": [
                "prefix-serper-secret-suffix",
                3,
                true
            ]
        })
    );
}

#[test]
fn resolve_secret_value_rejects_unlisted_secret_with_details() {
    let err = resolve_secret_value(
        &json!("Bearer {{secret:TAVILY_API_KEY}}"),
        &["BRAVE_API_KEY".to_string()],
    )
    .expect_err("unlisted secret should be rejected");
    let parsed = error_json(err);

    assert_eq!(parsed["code"], json!("forbidden_secret"));
    assert_eq!(parsed["details"]["secretName"], json!("TAVILY_API_KEY"));
}

#[test]
fn resolve_secret_value_rejects_missing_secret_with_details() {
    let _guard = ENV_LOCK.lock().unwrap();
    let _env = EnvGuard::set_many(&[("SERPER_API_KEY", None)]);
    let err = resolve_secret_value(
        &json!("{{secret:SERPER_API_KEY}}"),
        &["SERPER_API_KEY".to_string()],
    )
    .expect_err("missing env secret should be rejected");
    let parsed = error_json(err);

    assert_eq!(parsed["code"], json!("missing_secret"));
    assert_eq!(parsed["details"]["secretName"], json!("SERPER_API_KEY"));
}

#[test]
fn parse_fetch_spec_rejects_secret_placeholders_in_url_and_query() {
    let url_err = parse_fetch_spec(
        &json!({
            "url": "https://api.example.com/{{secret:BAD}}"
        }),
        &[],
    )
    .err()
    .expect("url should not accept secret placeholders");
    assert_eq!(error_json(url_err)["code"], json!("forbidden_secret"));

    let query_err = parse_fetch_spec(
        &json!({
            "url": "https://api.example.com/search",
            "query": {
                "q": "{{secret:BAD}}"
            }
        }),
        &[],
    )
    .err()
    .expect("query should not accept secret placeholders");
    assert_eq!(error_json(query_err)["code"], json!("forbidden_secret"));
}

#[test]
fn is_allowed_host_is_case_insensitive_and_trims_trailing_dots() {
    assert!(is_allowed_host(
        "API.Search.Brave.Com.",
        &["api.search.brave.com".to_string()]
    ));
    assert!(!is_allowed_host(
        "api.search.brave.com.evil.example",
        &["api.search.brave.com".to_string()]
    ));
}

#[test]
fn fetch_error_json_keeps_optional_details_shape() {
    let parsed = error_json(fetch_error_with_optional_details(
        "missing_secret",
        "secret missing",
        Some(json!({ "secretName": "TAVILY_API_KEY" })),
    ));
    assert_eq!(parsed["code"], json!("missing_secret"));
    assert_eq!(parsed["message"], json!("secret missing"));
    assert_eq!(parsed["details"]["secretName"], json!("TAVILY_API_KEY"));
}
