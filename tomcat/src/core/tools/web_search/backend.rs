use async_trait::async_trait;
use serde::de::DeserializeOwned;

use crate::core::llm::{env_name_for_provider, ModelCatalog};
use crate::infra::AppError;

use super::types::{RawHit, WebSearchRequest};

#[derive(Debug, Clone)]
pub struct HostedCandidateModel {
    /// Tomcat catalog identifier, for diagnostics only.
    pub id: String,
    /// Provider-facing model name. This is what must be sent in an OpenAI request.
    pub model_name: String,
    pub api: String,
    pub provider: String,
    pub base_url: Option<String>,
    pub key_env: String,
}

pub fn discover_hosted_candidate(catalog: &ModelCatalog) -> Option<HostedCandidateModel> {
    catalog
        .entries_in_merge_order()
        .into_iter()
        .find(|entry| entry.capabilities.web_search)
        .map(|entry| HostedCandidateModel {
            model_name: entry.request_model_name().to_string(),
            key_env: crate::core::llm::admin::inferred_api_key_env(&entry),
            id: entry.id,
            api: entry.api,
            provider: entry.provider,
            base_url: entry.base_url,
        })
}

#[derive(Debug, Clone)]
pub struct BackendSearchResponse {
    pub backend_label: Option<String>,
    pub raw_hits: Vec<RawHit>,
    pub warnings: Vec<String>,
}

#[async_trait]
pub trait WebSearchBackend: Send + Sync {
    async fn search(
        &self,
        request: &WebSearchRequest,
    ) -> Result<BackendSearchResponse, BackendFailure>;
}

#[derive(Debug, Clone)]
pub enum BackendFailure {
    MissingKey { env_name: String },
    Incompatible { detail: String },
    PluginRuntime { detail: String },
    Unauthorized { status: u16 },
    RateLimited { status: u16 },
    ServerError { status: u16 },
    Timeout,
    Transport { detail: String },
    InvalidRequest { status: u16, detail: String },
    Parse { detail: String },
}

impl BackendFailure {
    pub fn missing_key_for(provider: &str) -> Self {
        Self::MissingKey {
            env_name: env_name_for_provider(provider),
        }
    }

    pub fn to_tool_error(&self, backend: &str) -> AppError {
        match self {
            Self::MissingKey { env_name } => AppError::Tool(crate::infra::i18n::tr(
                "search.missingKey",
                &[("backend", backend), ("env", env_name)],
            )),
            Self::Incompatible { detail } => AppError::Tool(detail.clone()),
            Self::PluginRuntime { detail } => AppError::Tool(crate::infra::i18n::tr(
                "search.runtime",
                &[("backend", backend), ("detail", detail)],
            )),
            Self::InvalidRequest { status, detail } => AppError::Tool(crate::infra::i18n::tr(
                "search.invalidRequest",
                &[
                    ("backend", backend),
                    ("status", &status.to_string()),
                    ("detail", detail),
                ],
            )),
            Self::Parse { detail } => AppError::Tool(crate::infra::i18n::tr(
                "search.parse",
                &[("backend", backend), ("detail", detail)],
            )),
            Self::Unauthorized { status } => AppError::Tool(crate::infra::i18n::tr(
                "search.auth",
                &[("backend", backend), ("status", &status.to_string())],
            )),
            Self::RateLimited { status } | Self::ServerError { status } => {
                AppError::Tool(crate::infra::i18n::tr(
                    "search.unavailable",
                    &[("backend", backend), ("status", &status.to_string())],
                ))
            }
            Self::Timeout => AppError::Tool(crate::infra::i18n::tr(
                "search.timeout",
                &[("backend", backend)],
            )),
            Self::Transport { detail } => AppError::Tool(crate::infra::i18n::tr(
                "search.network",
                &[("backend", backend), ("detail", detail)],
            )),
        }
    }
}

pub(crate) fn map_reqwest_error(err: reqwest::Error) -> BackendFailure {
    if err.is_timeout() {
        BackendFailure::Timeout
    } else {
        BackendFailure::Transport {
            detail: err.to_string(),
        }
    }
}

pub(crate) fn classify_http_status(status: reqwest::StatusCode, body: &str) -> BackendFailure {
    match status.as_u16() {
        401 | 403 => BackendFailure::Unauthorized {
            status: status.as_u16(),
        },
        429 => BackendFailure::RateLimited {
            status: status.as_u16(),
        },
        400 | 404 | 422 => BackendFailure::InvalidRequest {
            status: status.as_u16(),
            detail: body.trim().to_string(),
        },
        code if status.is_server_error() => BackendFailure::ServerError { status: code },
        code if status.is_client_error() => BackendFailure::InvalidRequest {
            status: code,
            detail: body.trim().to_string(),
        },
        code => BackendFailure::Transport {
            detail: format!("unexpected http status {code}"),
        },
    }
}

pub(crate) async fn send_json<T: DeserializeOwned>(
    request: reqwest::RequestBuilder,
) -> Result<T, BackendFailure> {
    let response = request.send().await.map_err(map_reqwest_error)?;
    let status = response.status();
    let body = response.text().await.map_err(map_reqwest_error)?;
    if !status.is_success() {
        return Err(classify_http_status(status, &body));
    }
    serde_json::from_str(&body).map_err(|err| BackendFailure::Parse {
        detail: err.to_string(),
    })
}
