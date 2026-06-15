use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;

use super::backend::{BackendFailure, BackendSearchResponse, WebSearchBackend};
use super::types::{RawHit, WebSearchRequest};

#[async_trait]
pub trait PluginSearchInvoker: Send + Sync {
    async fn search(
        &self,
        backend: &str,
        params: serde_json::Value,
        session_id: &str,
    ) -> Result<serde_json::Value, BackendFailure>;
}

#[derive(Clone)]
pub struct PluginWebSearchBackend {
    invoker: Arc<dyn PluginSearchInvoker>,
    backend: String,
    session_id: String,
}

impl PluginWebSearchBackend {
    pub fn new(
        invoker: Arc<dyn PluginSearchInvoker>,
        backend: impl Into<String>,
        session_id: impl Into<String>,
    ) -> Self {
        Self {
            invoker,
            backend: backend.into(),
            session_id: session_id.into(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PluginSearchResponse {
    #[serde(default)]
    backend: Option<String>,
    #[serde(default)]
    hits: Vec<PluginSearchHit>,
    #[serde(default)]
    warnings: Vec<String>,
    #[serde(default)]
    unsupported_backend: bool,
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

#[async_trait]
impl WebSearchBackend for PluginWebSearchBackend {
    async fn search(
        &self,
        request: &WebSearchRequest,
    ) -> Result<BackendSearchResponse, BackendFailure> {
        let payload = serde_json::json!({
            "backend": self.backend,
            "query": request.query,
            "count": request.count,
            "freshness": request.freshness.map(|value| value.as_str()),
            "country": request.country,
            "language": request.language,
            "domainFilter": request.domain_filter,
        });
        let raw = self
            .invoker
            .search(&self.backend, payload, &self.session_id)
            .await?;
        let parsed: PluginSearchResponse =
            serde_json::from_value(raw).map_err(|err| BackendFailure::Parse {
                detail: err.to_string(),
            })?;
        if parsed.unsupported_backend {
            return Err(BackendFailure::Incompatible {
                detail: format!(
                    "web_search plugin backend `{}` reported unsupported_backend",
                    self.backend
                ),
            });
        }
        Ok(BackendSearchResponse {
            backend_label: parsed.backend,
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
}
