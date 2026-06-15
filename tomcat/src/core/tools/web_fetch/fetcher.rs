use std::path::Path;
use std::time::Instant;

use reqwest::header::{ACCEPT, CONTENT_TYPE, LOCATION, USER_AGENT};
use reqwest::{Client, Url};

use crate::infra::net_guard::read_body_limited;
use crate::infra::{AppError, ToolsWebFetchConfig};

use super::elapsed_ms;
use super::markdownify::{normalized_content_type, render_textual_body};
use super::persist::{effective_content_type, persist_binary, persist_text};
use super::redirect::is_permitted_redirect;
use super::types::{WebFetchFormat, WebFetchOutput, WebFetchRequest};
use super::validate::{validate_redirect_url, ValidatedUrl};

const ACCEPT_HEADER: &str =
    "text/markdown, text/html;q=0.9, application/xhtml+xml;q=0.8, */*;q=0.5";

/// 执行一次真实的 HTTP 抓取。
pub(crate) async fn fetch_url(
    client: &Client,
    persist_dir: &Path,
    config: &ToolsWebFetchConfig,
    request: &WebFetchRequest,
    validated: &ValidatedUrl,
) -> Result<WebFetchOutput, AppError> {
    let start = Instant::now();
    let mut current_url = validated.url.clone();
    let mut warnings = validated.warnings.clone();

    for hops in 0..=config.max_redirects {
        let response = match send_request(client, &current_url).await {
            Ok(response) => response,
            Err(err) if err.is_timeout() => {
                warnings.push("timeout".to_string());
                return Ok(WebFetchOutput::degraded(
                    current_url.to_string(),
                    0,
                    "Timeout".to_string(),
                    String::new(),
                    0,
                    elapsed_ms(start),
                    true,
                    warnings,
                ));
            }
            Err(err) => {
                return Err(AppError::Tool(format!(
                    "web_fetch: 请求 {} 失败: {}",
                    current_url, err
                )));
            }
        };

        let status = response.status();
        let code = status.as_u16();
        let code_text = status.canonical_reason().unwrap_or("").to_string();
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .trim()
            .to_string();

        if status.is_redirection() {
            if hops >= config.max_redirects {
                return Err(AppError::Tool(format!(
                    "web_fetch: redirect loop exceeds {} hops",
                    config.max_redirects
                )));
            }
            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| {
                    AppError::Tool(format!(
                        "web_fetch: redirect response missing Location for {}",
                        current_url
                    ))
                })?;
            let next = current_url.join(location).map_err(|err| {
                AppError::Tool(format!(
                    "web_fetch: redirect Location 非法 `{}`: {}",
                    location, err
                ))
            })?;
            let validated_next = validate_redirect_url(next.as_str())?;
            extend_unique(&mut warnings, validated_next.warnings.clone());
            if is_permitted_redirect(&current_url, &validated_next.url) {
                current_url = validated_next.url;
                continue;
            }
            warnings.push("redirect_off_host".to_string());
            return Ok(WebFetchOutput::redirect(
                current_url.to_string(),
                request.raw_url.clone(),
                validated_next.url.to_string(),
                code,
                code_text,
                elapsed_ms(start),
                warnings,
            ));
        }

        if code == 429 {
            warnings.push(format!("rate_limited (status={code})"));
            return Ok(WebFetchOutput::degraded(
                current_url.to_string(),
                code,
                code_text,
                content_type,
                0,
                elapsed_ms(start),
                true,
                warnings,
            ));
        }
        if status.is_server_error() {
            warnings.push(format!("server_error (status={code})"));
            return Ok(WebFetchOutput::degraded(
                current_url.to_string(),
                code,
                code_text,
                content_type,
                0,
                elapsed_ms(start),
                true,
                warnings,
            ));
        }

        let body = read_body_limited(response, config.max_http_content_bytes, "web_fetch").await?;
        if body.timed_out {
            warnings.push("timeout".to_string());
        }
        if body.truncated {
            warnings.push("http_oversize".to_string());
        }
        let content_type = effective_content_type(&content_type, &body.bytes);
        let normalized = normalized_content_type(&content_type);
        let duration_ms = elapsed_ms(start);
        let truncated = body.truncated || body.timed_out;

        if is_binary_content_type(&normalized, &body.bytes) {
            let persisted_output_path =
                persist_binary(persist_dir, &current_url, &content_type, &body.bytes).await?;
            warnings.push("binary_persisted".to_string());
            return Ok(WebFetchOutput::new(
                current_url.to_string(),
                code,
                code_text,
                content_type,
                body.bytes.len() as u64,
                String::new(),
                0,
                duration_ms,
                Some(persisted_output_path),
                None,
                truncated,
                warnings,
            ));
        }

        let full_text = render_textual_body(&body.bytes, &content_type, request.format);
        let total_chars = full_text.chars().count() as u64;
        let (result, persisted_output_path) = maybe_persist_large_text(
            persist_dir,
            &current_url,
            request.format,
            &full_text,
            config,
        )
        .await?;
        if persisted_output_path.is_some() {
            warnings.push(match request.format {
                WebFetchFormat::Markdown => "markdown_persisted".to_string(),
                WebFetchFormat::Text => "text_persisted".to_string(),
            });
        }

        return Ok(WebFetchOutput::new(
            current_url.to_string(),
            code,
            code_text,
            content_type,
            body.bytes.len() as u64,
            result,
            total_chars,
            duration_ms,
            persisted_output_path,
            None,
            truncated,
            warnings,
        ));
    }

    Err(AppError::Tool(format!(
        "web_fetch: redirect loop exceeds {} hops",
        config.max_redirects
    )))
}

async fn send_request(client: &Client, url: &Url) -> Result<reqwest::Response, reqwest::Error> {
    client
        .get(url.clone())
        .header(
            USER_AGENT,
            format!("pi/{} (web_fetch)", env!("CARGO_PKG_VERSION")),
        )
        .header(ACCEPT, ACCEPT_HEADER)
        .send()
        .await
}

async fn maybe_persist_large_text(
    persist_dir: &Path,
    current_url: &Url,
    format: WebFetchFormat,
    full_text: &str,
    config: &ToolsWebFetchConfig,
) -> Result<(String, Option<String>), AppError> {
    if full_text.chars().count() <= config.max_markdown_chars {
        return Ok((full_text.to_string(), None));
    }

    let persisted_output_path = persist_text(
        persist_dir,
        current_url,
        format.persisted_extension(),
        full_text,
    )
    .await?;
    Ok((
        build_persisted_head(
            full_text,
            config.markdown_head_chars,
            &persisted_output_path,
            full_text.chars().count() as u64,
            format,
        ),
        Some(persisted_output_path),
    ))
}

fn build_persisted_head(
    full_text: &str,
    max_chars: usize,
    path: &str,
    total_chars: u64,
    format: WebFetchFormat,
) -> String {
    let head = truncate_chars(full_text, max_chars);
    let noun = match format {
        WebFetchFormat::Markdown => "markdown",
        WebFetchFormat::Text => "text",
    };
    if head.is_empty() {
        return format!(
            "...full {noun} persisted to {path} (total {total_chars} chars); use search_files(target=content) to locate then read(offset=...) to page through"
        );
    }
    format!(
        "{head}\n\n...full {noun} persisted to {path} (total {total_chars} chars); use search_files(target=content) to locate then read(offset=...) to page through"
    )
}

fn truncate_chars(input: &str, max_chars: usize) -> String {
    input.chars().take(max_chars).collect()
}

fn is_binary_content_type(content_type: &str, body: &[u8]) -> bool {
    if matches!(
        content_type,
        value if value.starts_with("text/")
            || value == "application/json"
            || value == "application/xml"
            || value == "text/xml"
            || value.ends_with("+xml")
    ) {
        return false;
    }
    if content_type.is_empty() {
        return std::str::from_utf8(body).is_err();
    }
    true
}

fn extend_unique(target: &mut Vec<String>, extra: Vec<String>) {
    for warning in extra {
        if !target.iter().any(|existing| existing == &warning) {
            target.push(warning);
        }
    }
}
