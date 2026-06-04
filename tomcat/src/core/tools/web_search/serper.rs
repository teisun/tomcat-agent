use async_trait::async_trait;
use serde::Deserialize;

use super::backend::{send_json, BackendFailure, BackendSearchResponse, WebSearchBackend};
use super::types::{RawHit, WebSearchRequest};

#[derive(Clone)]
pub struct SerperBackend {
    client: reqwest::Client,
    base_url: String,
}

impl SerperBackend {
    pub fn new(client: reqwest::Client, base_url: String) -> Self {
        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    fn api_key(&self) -> Result<String, BackendFailure> {
        std::env::var("SERPER_API_KEY")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| BackendFailure::missing_key_for("serper"))
    }
}

#[async_trait]
impl WebSearchBackend for SerperBackend {
    async fn search(
        &self,
        request: &WebSearchRequest,
    ) -> Result<BackendSearchResponse, BackendFailure> {
        let api_key = self.api_key()?;
        let mut body = serde_json::json!({
            "q": rewrite_query_with_domain_filter(&request.query, &request.domain_filter),
            "num": request.count,
        });
        if let Some(country) = request.country.as_deref() {
            body["gl"] = serde_json::json!(country);
        }
        if let Some(language) = request.language.as_deref() {
            body["hl"] = serde_json::json!(language);
        }
        if let Some(freshness) = request.freshness {
            body["tbs"] = serde_json::json!(freshness.as_serper_query());
        }

        let response: SerperResponse = send_json(
            self.client
                .post(format!("{}/search", self.base_url))
                .header("X-API-KEY", api_key)
                .header("Content-Type", "application/json")
                .json(&body),
        )
        .await?;

        Ok(BackendSearchResponse {
            raw_hits: response
                .organic
                .unwrap_or_default()
                .into_iter()
                .filter_map(|item| {
                    item.link.map(|url| RawHit {
                        title: item.title,
                        url,
                        snippet: item.snippet,
                        published_at: item.date,
                    })
                })
                .collect(),
            warnings: Vec::new(),
        })
    }
}

fn rewrite_query_with_domain_filter(query: &str, domains: &[String]) -> String {
    if domains.is_empty() {
        return query.to_string();
    }
    let filters = domains
        .iter()
        .map(|domain| format!("site:{domain}"))
        .collect::<Vec<_>>()
        .join(" OR ");
    format!("({query}) ({filters})")
}

#[derive(Debug, Deserialize)]
struct SerperResponse {
    #[serde(default)]
    organic: Option<Vec<SerperResult>>,
}

#[derive(Debug, Deserialize)]
struct SerperResult {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    link: Option<String>,
    #[serde(default)]
    snippet: Option<String>,
    #[serde(default)]
    date: Option<String>,
}
