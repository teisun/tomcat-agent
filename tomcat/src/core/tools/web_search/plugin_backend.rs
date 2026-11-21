use async_trait::async_trait;
use serde::Deserialize;

use crate::infra::i18n::tr;
use crate::infra::AppError;

use super::declarations::BackendDeclaration;
use super::types::{RawHit, WebSearchRequest};

#[derive(Default)]
pub struct PluginSearchCatalog {
    pub declarations: Vec<BackendDeclaration>,
    pub saved_keys: bool,
}

#[async_trait]
pub trait PluginSearchInvoker: Send + Sync {
    fn search_catalog(&self) -> PluginSearchCatalog;

    async fn search(
        &self,
        backend: &str,
        params: serde_json::Value,
        session_id: &str,
    ) -> Result<serde_json::Value, AppError>;
}

#[derive(Debug, Deserialize)]
struct PluginSearchResponse {
    #[serde(default)]
    hits: Vec<PluginSearchHit>,
    #[serde(default)]
    warnings: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct PluginSearchHit {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    snippet: Option<String>,
    #[serde(default)]
    published_at: Option<String>,
}

pub(super) struct BackendSearchResponse {
    pub raw_hits: Vec<RawHit>,
    pub warnings: Vec<String>,
}

pub(super) async fn invoke_backend(
    invoker: &dyn PluginSearchInvoker,
    declaration: &BackendDeclaration,
    request: &WebSearchRequest,
    session_id: &str,
) -> Result<BackendSearchResponse, AppError> {
    let backend = declaration.id.as_str();
    let payload = serde_json::json!({
        "backend": backend,
        "query": request.query,
        "count": request.count,
        "freshness": request.freshness.map(|value| value.as_str()),
        "country": request.country,
        "language": request.language,
        "domainFilter": request.domain_filter,
        "model": declaration.model,
    });
    let raw = invoker.search(backend, payload, session_id).await?;
    let parsed: PluginSearchResponse = serde_json::from_value(raw).map_err(|err| {
        AppError::Tool(tr(
            "search.runtime",
            &[("backend", backend), ("detail", &err.to_string())],
        ))
    })?;
    Ok(BackendSearchResponse {
        raw_hits: parsed
            .hits
            .into_iter()
            .filter_map(|hit| {
                Some(RawHit {
                    title: hit.title,
                    url: hit.url?,
                    snippet: hit.snippet,
                    published_at: hit.published_at,
                })
            })
            .collect(),
        warnings: parsed.warnings,
    })
}
