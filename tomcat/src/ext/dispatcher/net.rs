use std::sync::LazyLock;

use regex::Regex;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::Method;
use serde_json::Value;
use tracing::debug;

use super::types::HostApiDispatcher;
use crate::core::llm::auth::{process_env_value, read_env_value};
use crate::core::tools::web_fetch::types::MAX_URL_LENGTH;
use crate::ext::host_binding::HostResponse;
use crate::infra::net_guard::{
    read_body_limited, validate_http_url, HttpSchemePolicy, UrlValidationOptions,
};
use crate::infra::AppError;

static SECRET_PLACEHOLDER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\{\{secret:([A-Za-z0-9_]+)\}\}").expect("valid secret placeholder regex")
});

pub(super) const FETCH_ERROR_PREFIX: &str = "pi.fetch";

impl HostApiDispatcher {
    pub(super) async fn do_fetch(
        &self,
        instance_id: &str,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let manager = self.plugin_manager.as_ref().and_then(|weak| weak.upgrade());
        let plugin = manager
            .as_ref()
            .and_then(|manager| manager.instance_manifest(instance_id))
            .ok_or_else(|| {
                fetch_error("dispatcher_unavailable", "pi.fetch runtime is unavailable")
            })?;
        if !plugin
            .required_permissions
            .iter()
            .any(|perm| perm == "net:fetch")
        {
            return Err(fetch_error(
                "permission_denied",
                "pi.fetch requires manifest.requiredPermissions to include net:fetch",
            ));
        }

        let saved_keys =
            manager.is_some_and(|manager| manager.instance_can_use_saved_keys(instance_id));
        let spec = parse_fetch_spec(params, &plugin.required_secrets, saved_keys)?;
        let validated = validate_http_url(
            &spec.url,
            UrlValidationOptions {
                max_url_length: MAX_URL_LENGTH,
                error_prefix: FETCH_ERROR_PREFIX,
                scheme_policy: HttpSchemePolicy::RequireHttps,
            },
        )
        .map_err(|err| fetch_error("ssrf_rejected", err.to_string()))?;
        let host = validated
            .url
            .host_str()
            .map(|value| value.trim_end_matches('.').to_ascii_lowercase())
            .ok_or_else(|| {
                fetch_error("invalid_url", "pi.fetch target URL is missing a valid host")
            })?;
        if !is_allowed_host(&host, &plugin.allowed_hosts) {
            return Err(fetch_error(
                "host_not_allowed",
                format!("pi.fetch target host `{host}` is not in manifest.allowedHosts"),
            ));
        }

        let _permit = self.fetch_semaphore.acquire().await.map_err(|_| {
            fetch_error(
                "dispatcher_unavailable",
                "pi.fetch semaphore is unavailable",
            )
        })?;

        let mut request = self.fetch_client.request(spec.method, validated.url);
        if !spec.query.is_empty() {
            request = request.query(&spec.query);
        }
        if !spec.headers.is_empty() {
            request = request.headers(spec.headers);
        }
        if let Some(body) = spec.body {
            request = request.body(body);
        }

        debug!(
            target: "tomcat::pi_fetch",
            host = %host,
            proxy_mode = self.fetch_proxy_mode_label,
            timeout_ms = self.fetch_timeout.as_millis(),
            "dispatching pi.fetch request"
        );
        let response = request.send().await.map_err(map_fetch_transport_error)?;
        let status = response.status().as_u16();
        if response.status().is_redirection() {
            return Err(fetch_error(
                "redirect_not_allowed",
                "pi.fetch rejects redirect responses by default",
            ));
        }
        let body = read_body_limited(response, self.fetch_max_body_bytes, FETCH_ERROR_PREFIX)
            .await
            .map_err(|_| {
                fetch_error(
                    "transport_error",
                    "pi.fetch failed while reading response body",
                )
            })?;
        if body.timed_out {
            return Err(fetch_error("timeout", "pi.fetch request timed out"));
        }
        if body.truncated {
            return Err(fetch_error(
                "response_too_large",
                format!(
                    "pi.fetch response exceeded {} bytes",
                    self.fetch_max_body_bytes
                ),
            ));
        }

        Ok(HostResponse::ok(serde_json::json!({
            "status": status,
            "body": String::from_utf8_lossy(&body.bytes).into_owned(),
        })))
    }
}

#[derive(Debug)]
struct FetchSpec {
    method: Method,
    url: String,
    headers: HeaderMap,
    query: Vec<(String, String)>,
    body: Option<String>,
}

fn parse_fetch_spec(
    params: &Value,
    required_secrets: &[String],
    saved_keys: bool,
) -> Result<FetchSpec, AppError> {
    let url = params
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| fetch_error("invalid_request", "pi.fetch requires a string `url`"))?;
    if contains_secret_placeholder(url) {
        return Err(fetch_error(
            "forbidden_secret",
            "pi.fetch only expands secret placeholders in header values",
        ));
    }

    let method_raw = params
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("GET")
        .trim()
        .to_ascii_uppercase();
    let method = Method::from_bytes(method_raw.as_bytes()).map_err(|_| {
        fetch_error(
            "invalid_request",
            format!("pi.fetch received unsupported HTTP method `{method_raw}`"),
        )
    })?;

    let required_secrets = required_secrets
        .iter()
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>();

    let lookup = if saved_keys {
        read_env_value
    } else {
        process_env_value
    };
    let headers = resolve_headers(params.get("headers"), &required_secrets, &lookup)?;
    let query = resolve_query(params.get("query"))?;
    let body = resolve_body(params.get("body"))?;

    Ok(FetchSpec {
        method,
        url: url.to_string(),
        headers,
        query,
        body,
    })
}

fn resolve_headers(
    raw: Option<&Value>,
    required_secrets: &[String],
    lookup: &impl Fn(&str) -> Option<String>,
) -> Result<HeaderMap, AppError> {
    let Some(headers) = raw else {
        return Ok(HeaderMap::new());
    };
    let object = headers.as_object().ok_or_else(|| {
        fetch_error(
            "invalid_request",
            "pi.fetch `headers` must be a JSON object",
        )
    })?;
    let mut out = HeaderMap::new();
    for (name, value) in object {
        if contains_secret_placeholder(name) {
            return Err(fetch_error(
                "forbidden_secret",
                "pi.fetch only expands secret placeholders in header values",
            ));
        }
        let header_name = HeaderName::try_from(name.as_str()).map_err(|_| {
            fetch_error(
                "invalid_request",
                format!("pi.fetch received invalid header name `{name}`"),
            )
        })?;
        let raw = stringify_scalar(value).ok_or_else(|| {
            fetch_error(
                "invalid_request",
                format!("pi.fetch header `{name}` must be a string/number/bool/null"),
            )
        })?;
        let expanded = resolve_secret_text(&raw, required_secrets, lookup)?;
        let mut header_value = HeaderValue::from_str(&expanded).map_err(|_| {
            fetch_error(
                "invalid_request",
                format!("pi.fetch header `{name}` contains invalid bytes"),
            )
        })?;
        header_value.set_sensitive(contains_secret_placeholder(&raw));
        out.insert(header_name, header_value);
    }
    Ok(out)
}

fn resolve_query(raw: Option<&Value>) -> Result<Vec<(String, String)>, AppError> {
    let Some(query) = raw else {
        return Ok(Vec::new());
    };
    let object = query
        .as_object()
        .ok_or_else(|| fetch_error("invalid_request", "pi.fetch `query` must be a JSON object"))?;
    let mut out = Vec::new();
    for (name, value) in object {
        if contains_secret_placeholder(name) || contains_secret_placeholder_in_value(value) {
            return Err(fetch_error(
                "forbidden_secret",
                "pi.fetch only expands secret placeholders in header values",
            ));
        }
        match value {
            Value::Array(items) => {
                for item in items {
                    let rendered = stringify_scalar(item).ok_or_else(|| {
                        fetch_error(
                            "invalid_request",
                            format!(
                                "pi.fetch query `{name}` array items must be string/number/bool/null"
                            ),
                        )
                    })?;
                    out.push((name.clone(), rendered));
                }
            }
            _ => {
                let rendered = stringify_scalar(value).ok_or_else(|| {
                    fetch_error(
                        "invalid_request",
                        format!("pi.fetch query `{name}` must be string/number/bool/null"),
                    )
                })?;
                out.push((name.clone(), rendered));
            }
        }
    }
    Ok(out)
}

// Bodies may include untrusted query text. Leave placeholders literal for every source.
fn resolve_body(raw: Option<&Value>) -> Result<Option<String>, AppError> {
    match raw {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(other) => serde_json::to_string(other).map(Some).map_err(|err| {
            fetch_error(
                "invalid_request",
                format!("pi.fetch body serialize failed: {err}"),
            )
        }),
    }
}

fn resolve_secret_text(
    text: &str,
    required_secrets: &[String],
    lookup: &impl Fn(&str) -> Option<String>,
) -> Result<String, AppError> {
    let mut result = String::new();
    let mut last = 0usize;
    for capture in SECRET_PLACEHOLDER_RE.captures_iter(text) {
        let whole = capture.get(0).expect("whole match");
        let secret_name = capture.get(1).expect("secret name").as_str();
        if !required_secrets
            .iter()
            .any(|allowed| allowed == secret_name)
        {
            return Err(fetch_error_with_details(
                "forbidden_secret",
                format!("pi.fetch secret `{secret_name}` is not in manifest.requiredSecrets"),
                serde_json::json!({ "secretName": secret_name }),
            ));
        }
        let secret_value = lookup(secret_name).ok_or_else(|| {
            fetch_error_with_details(
                "missing_secret",
                format!("pi.fetch secret `{secret_name}` is not available for this request"),
                serde_json::json!({ "secretName": secret_name }),
            )
        })?;
        result.push_str(&text[last..whole.start()]);
        result.push_str(&secret_value);
        last = whole.end();
    }
    result.push_str(&text[last..]);
    Ok(result)
}

fn is_allowed_host(host: &str, allowed_hosts: &[String]) -> bool {
    let normalized_host = host.trim_end_matches('.').to_ascii_lowercase();
    allowed_hosts
        .iter()
        .map(|item| item.trim().trim_end_matches('.').to_ascii_lowercase())
        .filter(|item| !item.is_empty())
        .any(|item| item == normalized_host)
}

fn stringify_scalar(value: &Value) -> Option<String> {
    match value {
        Value::Null => Some(String::new()),
        Value::String(text) => Some(text.clone()),
        Value::Bool(boolean) => Some(boolean.to_string()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

fn contains_secret_placeholder(text: &str) -> bool {
    SECRET_PLACEHOLDER_RE.is_match(text)
}

fn contains_secret_placeholder_in_value(value: &Value) -> bool {
    match value {
        Value::String(text) => contains_secret_placeholder(text),
        Value::Array(items) => items.iter().any(contains_secret_placeholder_in_value),
        Value::Object(map) => map.values().any(contains_secret_placeholder_in_value),
        _ => false,
    }
}

fn map_fetch_transport_error(err: reqwest::Error) -> AppError {
    if err.is_timeout() {
        fetch_error("timeout", "pi.fetch request timed out")
    } else {
        fetch_error("transport_error", "pi.fetch request failed")
    }
}

fn fetch_error(code: &str, message: impl Into<String>) -> AppError {
    fetch_error_with_optional_details(code, message, None)
}

fn fetch_error_with_details(
    code: &str,
    message: impl Into<String>,
    details: serde_json::Value,
) -> AppError {
    fetch_error_with_optional_details(code, message, Some(details))
}

fn fetch_error_with_optional_details(
    code: &str,
    message: impl Into<String>,
    details: Option<serde_json::Value>,
) -> AppError {
    AppError::Plugin(
        serde_json::json!({
            "code": code,
            "message": message.into(),
            "details": details,
        })
        .to_string(),
    )
}

#[cfg(test)]
#[path = "tests/net_test.rs"]
mod tests;
