use async_trait::async_trait;
use serde::Deserialize;

use super::backend::{send_json, BackendFailure, BackendSearchResponse, WebSearchBackend};
use super::types::{RawHit, WebSearchRequest};

#[derive(Clone)]
pub struct TavilyBackend {
    client: reqwest::Client,
    base_url: String,
}

impl TavilyBackend {
    pub fn new(client: reqwest::Client, base_url: String) -> Self {
        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    fn api_key(&self) -> Result<String, BackendFailure> {
        std::env::var("TAVILY_API_KEY")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| BackendFailure::missing_key_for("tavily"))
    }
}

#[async_trait]
impl WebSearchBackend for TavilyBackend {
    async fn search(
        &self,
        request: &WebSearchRequest,
    ) -> Result<BackendSearchResponse, BackendFailure> {
        let api_key = self.api_key()?;
        let mut body = serde_json::json!({
            "query": request.query,
            "max_results": request.count,
        });
        if let Some(freshness) = request.freshness {
            body["time_range"] = serde_json::json!(freshness.as_str());
        }
        if !request.domain_filter.is_empty() {
            body["include_domains"] = serde_json::json!(request.domain_filter);
        }

        let response: TavilyResponse = send_json(
            self.client
                .post(format!("{}/search", self.base_url))
                .header("Authorization", format!("Bearer {api_key}"))
                .json(&body),
        )
        .await?;

        let warnings = if request.country.is_some() || request.language.is_some() {
            vec!["tavily_ignores_country_language".to_string()]
        } else {
            Vec::new()
        };

        Ok(BackendSearchResponse {
            raw_hits: response
                .results
                .into_iter()
                .filter_map(|item| {
                    item.url.map(|url| RawHit {
                        title: item.title,
                        url,
                        snippet: item.content.or(item.snippet),
                        published_at: item.published_date.or(item.published_at),
                    })
                })
                .collect(),
            warnings,
        })
    }
}

#[derive(Debug, Deserialize)]
struct TavilyResponse {
    #[serde(default)]
    results: Vec<TavilyResult>,
}

#[derive(Debug, Deserialize)]
struct TavilyResult {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    snippet: Option<String>,
    #[serde(default)]
    published_date: Option<String>,
    #[serde(default)]
    published_at: Option<String>,
}
