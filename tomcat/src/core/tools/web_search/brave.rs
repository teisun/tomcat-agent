use async_trait::async_trait;
use serde::Deserialize;

use super::backend::{send_json, BackendFailure, BackendSearchResponse, WebSearchBackend};
use super::types::{RawHit, WebSearchRequest};

#[derive(Clone)]
pub struct BraveBackend {
    client: reqwest::Client,
    base_url: String,
}

impl BraveBackend {
    pub fn new(client: reqwest::Client, base_url: String) -> Self {
        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    fn api_key(&self) -> Result<String, BackendFailure> {
        std::env::var("BRAVE_API_KEY")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| BackendFailure::missing_key_for("brave"))
    }
}

#[async_trait]
impl WebSearchBackend for BraveBackend {
    async fn search(
        &self,
        request: &WebSearchRequest,
    ) -> Result<BackendSearchResponse, BackendFailure> {
        let api_key = self.api_key()?;
        let rewrote_domain_filter = !request.domain_filter.is_empty();
        let mut params = vec![
            (
                "q".to_string(),
                rewrite_query_with_domain_filter(&request.query, &request.domain_filter),
            ),
            ("count".to_string(), request.count.to_string()),
        ];
        if let Some(country) = request.country.as_deref() {
            params.push(("country".to_string(), country.to_string()));
        }
        if let Some(language) = request.language.as_deref() {
            params.push(("search_lang".to_string(), language.to_string()));
        }
        if let Some(freshness) = request.freshness {
            params.push((
                "freshness".to_string(),
                freshness.as_brave_query().to_string(),
            ));
        }

        let response: BraveResponse = send_json(
            self.client
                .get(format!("{}/res/v1/web/search", self.base_url))
                .header("Accept", "application/json")
                .header("X-Subscription-Token", api_key)
                .query(&params),
        )
        .await?;

        Ok(BackendSearchResponse {
            backend_label: None,
            raw_hits: response
                .web
                .and_then(|web| web.results)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|item| {
                    item.url.map(|url| RawHit {
                        title: item.title,
                        url,
                        snippet: item.description.or(item.snippet),
                        published_at: item.age.or(item.page_age),
                    })
                })
                .collect(),
            warnings: if rewrote_domain_filter {
                vec!["brave_domain_filter_via_query_rewrite".to_string()]
            } else {
                Vec::new()
            },
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
struct BraveResponse {
    #[serde(default)]
    web: Option<BraveWebSection>,
}

#[derive(Debug, Deserialize)]
struct BraveWebSection {
    #[serde(default)]
    results: Option<Vec<BraveResult>>,
}

#[derive(Debug, Deserialize)]
struct BraveResult {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    snippet: Option<String>,
    #[serde(default)]
    age: Option<String>,
    #[serde(default)]
    page_age: Option<String>,
}
