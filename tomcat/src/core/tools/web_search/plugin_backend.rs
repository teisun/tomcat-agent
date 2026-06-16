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

    fn classify_warning_failure(&self, warnings: &[String]) -> Option<BackendFailure> {
        for warning in warnings {
            if let Some(raw_env_name) = warning.strip_prefix("__missing_key__:") {
                let env_name = raw_env_name.trim();
                return Some(if env_name.is_empty() {
                    BackendFailure::missing_key_for(&self.backend)
                } else {
                    BackendFailure::MissingKey {
                        env_name: env_name.to_string(),
                    }
                });
            }
            if let Some(raw_status) = warning.strip_prefix("__unauthorized__:") {
                let status = raw_status.trim().parse::<u16>().unwrap_or(401);
                return Some(BackendFailure::Unauthorized { status });
            }
        }
        None
    }

    fn classify_plugin_runtime_failure(&self, warnings: &[String]) -> Option<BackendFailure> {
        let plugin_runtime_warnings: Vec<String> = warnings
            .iter()
            .filter(|warning| {
                warning.starts_with("plugin_backend_error")
                    && !is_retryable_timeout_warning(warning)
            })
            .cloned()
            .collect();
        if plugin_runtime_warnings.is_empty() {
            None
        } else {
            Some(BackendFailure::PluginRuntime {
                detail: plugin_runtime_warnings.join("; "),
            })
        }
    }

    fn unsupported_backend_failure(&self, warnings: &[String]) -> BackendFailure {
        let mut detail = format!(
            "web_search plugin backend `{}` reported unsupported_backend",
            self.backend
        );
        if !warnings.is_empty() {
            detail.push('：');
            detail.push_str(&warnings.join("; "));
        }
        BackendFailure::Incompatible { detail }
    }
}

fn is_retryable_timeout_warning(warning: &str) -> bool {
    warning.starts_with("plugin_backend_error")
        && (warning.contains("pi.fetch request timed out")
            || warning.contains("request timed out")
            || warning.contains("请求超时"))
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
    #[serde(default, alias = "unsupported_backend")]
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
            "tavilyBaseUrl": request.tavily_base_url,
            "braveBaseUrl": request.brave_base_url,
            "serperBaseUrl": request.serper_base_url,
        });
        let raw = self
            .invoker
            .search(&self.backend, payload, &self.session_id)
            .await?;
        let parsed: PluginSearchResponse =
            serde_json::from_value(raw).map_err(|err| BackendFailure::Parse {
                detail: err.to_string(),
            })?;
        if let Some(failure) = self.classify_plugin_runtime_failure(&parsed.warnings) {
            return Err(failure);
        }
        if let Some(failure) = self.classify_warning_failure(&parsed.warnings) {
            return Err(failure);
        }
        if parsed.unsupported_backend {
            if parsed
                .warnings
                .iter()
                .any(|warning| is_retryable_timeout_warning(warning))
            {
                return Err(BackendFailure::Timeout);
            }
            return Err(self.unsupported_backend_failure(&parsed.warnings));
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
