use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::backend::{send_json, BackendFailure, BackendSearchResponse};
use super::types::{RawHit, WebSearchRequest};

pub async fn search_openai_hosted(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    model: &str,
    request: &WebSearchRequest,
) -> Result<BackendSearchResponse, BackendFailure> {
    let payload: Value = send_json(
        client
            .post(format!("{}/v1/responses", base_url.trim_end_matches('/')))
            .header("Authorization", format!("Bearer {api_key}"))
            .header("Content-Type", "application/json")
            .json(&build_hosted_request_body(model, request)),
    )
    .await?;
    parse_server_tool_blocks(&payload)
}

pub fn build_hosted_request_body(model: &str, request: &WebSearchRequest) -> Value {
    let mut tool = json!({
        "type": "web_search",
        "search_context_size": "medium",
    });

    let mut filters = serde_json::Map::new();
    if !request.allowed_domains.is_empty() {
        filters.insert(
            "allowed_domains".to_string(),
            Value::Array(
                request
                    .allowed_domains
                    .iter()
                    .cloned()
                    .map(Value::String)
                    .collect(),
            ),
        );
    }
    if !request.blocked_domains.is_empty() {
        filters.insert(
            "blocked_domains".to_string(),
            Value::Array(
                request
                    .blocked_domains
                    .iter()
                    .cloned()
                    .map(Value::String)
                    .collect(),
            ),
        );
    }
    if !filters.is_empty() {
        tool["filters"] = Value::Object(filters);
    }
    if let Some(country) = request.country.as_deref() {
        tool["user_location"] = json!({
            "type": "approximate",
            "country": country,
        });
    }

    json!({
        "model": model,
        "input": request.query,
        "store": false,
        "tool_choice": "required",
        "include": [
            "web_search_call.action.sources",
            "web_search_call.results"
        ],
        "tools": [tool]
    })
}

pub fn parse_server_tool_blocks(payload: &Value) -> Result<BackendSearchResponse, BackendFailure> {
    if let Some(error) = payload.get("error") {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .or_else(|| error.as_str())
            .unwrap_or("hosted web_search request failed")
            .to_string();
        return Err(BackendFailure::Transport { detail: message });
    }

    let output = payload
        .get("output")
        .and_then(Value::as_array)
        .ok_or_else(|| BackendFailure::Parse {
            detail: "responses payload missing `output` array".to_string(),
        })?;

    let mut hits = Vec::new();
    for item in output {
        match item.get("type").and_then(Value::as_str).unwrap_or("") {
            "web_search_call" => hits.extend(parse_web_search_call(item)),
            "message" => hits.extend(parse_message_annotations(item)),
            "server_tool_use" => {}
            "web_search_tool_result" => hits.extend(parse_server_tool_result(item)),
            _ => {}
        }
    }

    Ok(BackendSearchResponse {
        backend_label: None,
        raw_hits: dedupe_hits(hits),
        warnings: Vec::new(),
    })
}

fn parse_web_search_call(item: &Value) -> Vec<RawHit> {
    let mut hits = Vec::new();
    if let Some(results) = item.get("results").and_then(Value::as_array) {
        hits.extend(results.iter().filter_map(parse_generic_result));
    }
    if let Some(sources) = item
        .get("action")
        .and_then(|action| action.get("sources"))
        .and_then(Value::as_array)
    {
        hits.extend(sources.iter().filter_map(|source| {
            source.get("url").and_then(Value::as_str).map(|url| RawHit {
                title: source
                    .get("title")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                url: url.to_string(),
                snippet: source
                    .get("snippet")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                published_at: None,
            })
        }));
    }
    hits
}

fn parse_server_tool_result(item: &Value) -> Vec<RawHit> {
    item.get("content")
        .and_then(Value::as_array)
        .map(|content| content.iter().filter_map(parse_generic_result).collect())
        .unwrap_or_default()
}

fn parse_message_annotations(item: &Value) -> Vec<RawHit> {
    let Some(content) = item.get("content").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut hits = Vec::new();
    for part in content {
        if part.get("type").and_then(Value::as_str) != Some("output_text") {
            continue;
        }
        let text = part.get("text").and_then(Value::as_str).unwrap_or("");
        let annotations = part
            .get("annotations")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for annotation in annotations {
            let annotation_type = annotation.get("type").and_then(Value::as_str).unwrap_or("");
            if !annotation_type.contains("citation") {
                continue;
            }
            let Some(url) = annotation.get("url").and_then(Value::as_str) else {
                continue;
            };
            let snippet = annotation
                .get("cited_text")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| extract_annotation_text(text, &annotation));
            hits.push(RawHit {
                title: annotation
                    .get("title")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                url: url.to_string(),
                snippet,
                published_at: None,
            });
        }
    }
    hits
}

fn extract_annotation_text(text: &str, annotation: &Value) -> Option<String> {
    let start = annotation.get("start_index").and_then(Value::as_u64)? as usize;
    let end = annotation.get("end_index").and_then(Value::as_u64)? as usize;
    if start >= end {
        return None;
    }
    let chars: Vec<char> = text.chars().collect();
    if end > chars.len() {
        return None;
    }
    Some(chars[start..end].iter().collect())
}

fn parse_generic_result(value: &Value) -> Option<RawHit> {
    let url = value
        .get("url")
        .and_then(Value::as_str)
        .or_else(|| value.get("link").and_then(Value::as_str))?;
    Some(RawHit {
        title: value
            .get("title")
            .and_then(Value::as_str)
            .map(str::to_string),
        url: url.to_string(),
        snippet: value
            .get("snippet")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                value
                    .get("content")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .or_else(|| {
                value
                    .get("text")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .or_else(|| {
                value
                    .get("summary")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            }),
        published_at: value
            .get("page_age")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                value
                    .get("published_at")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .or_else(|| {
                value
                    .get("date")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            }),
    })
}

fn dedupe_hits(hits: Vec<RawHit>) -> Vec<RawHit> {
    let mut merged: BTreeMap<String, RawHit> = BTreeMap::new();
    for hit in hits {
        let key = hit.url.clone();
        match merged.get_mut(&key) {
            Some(existing) => {
                if existing.title.as_deref().unwrap_or("").is_empty() {
                    existing.title = hit.title.clone();
                }
                if existing.snippet.as_deref().unwrap_or("").is_empty() {
                    existing.snippet = hit.snippet.clone();
                }
                if existing.published_at.as_deref().unwrap_or("").is_empty() {
                    existing.published_at = hit.published_at.clone();
                }
            }
            None => {
                merged.insert(key, hit);
            }
        }
    }
    merged.into_values().collect()
}
