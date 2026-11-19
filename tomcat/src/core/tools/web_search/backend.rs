use async_trait::async_trait;

use crate::core::llm::env_name_for_provider;
use crate::infra::AppError;

use super::types::{RawHit, WebSearchRequest};

#[derive(Debug, Clone)]
pub struct BackendSearchResponse {
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
