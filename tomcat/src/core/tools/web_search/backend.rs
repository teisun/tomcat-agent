use async_trait::async_trait;
use serde::de::DeserializeOwned;

use crate::core::llm::{env_name_for_provider, ModelCatalog};
use crate::infra::AppError;

use super::types::{RawHit, WebSearchRequest};

pub const HTTP_AUTO_CHAIN: [BackendName; 3] =
    [BackendName::Tavily, BackendName::Brave, BackendName::Serper];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BackendName {
    Openai,
    Tavily,
    Brave,
    Serper,
}

impl BackendName {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Openai => "openai",
            Self::Tavily => "tavily",
            Self::Brave => "brave",
            Self::Serper => "serper",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum BackendMode {
    Auto,
    Openai,
    Tavily,
    Brave,
    Serper,
    Plugin(String),
}

impl BackendMode {
    pub fn parse(raw: &str) -> Result<Self, AppError> {
        let normalized = raw.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "" | "auto" => Ok(Self::Auto),
            "openai" => Ok(Self::Openai),
            "tavily" => Ok(Self::Tavily),
            "brave" => Ok(Self::Brave),
            "serper" => Ok(Self::Serper),
            _ => Ok(Self::Plugin(normalized)),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Auto => "auto",
            Self::Openai => "openai",
            Self::Tavily => "tavily",
            Self::Brave => "brave",
            Self::Serper => "serper",
            Self::Plugin(name) => name.as_str(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct HostedCandidateModel {
    pub id: String,
    pub api: String,
    pub provider: String,
    pub base_url: Option<String>,
}

pub fn discover_hosted_candidate(catalog: &ModelCatalog) -> Option<HostedCandidateModel> {
    catalog
        .entries_in_merge_order()
        .into_iter()
        .find(|entry| entry.capabilities.web_search)
        .map(|entry| HostedCandidateModel {
            id: entry.id,
            api: entry.api,
            provider: entry.provider,
            base_url: entry.base_url,
        })
}

#[derive(Debug, Clone)]
pub enum BackendPlan {
    Auto {
        hosted_candidate: Option<HostedCandidateModel>,
        http_chain: Vec<BackendName>,
        plugin_slot: bool,
    },
    ExplicitHttp(BackendName),
    ExplicitPlugin(String),
    HostedOnly(HostedCandidateModel),
}

pub fn pick_backend(
    backend: BackendMode,
    hosted_candidate: Option<HostedCandidateModel>,
) -> Result<BackendPlan, AppError> {
    match backend {
        BackendMode::Auto => Ok(BackendPlan::Auto {
            hosted_candidate,
            http_chain: HTTP_AUTO_CHAIN.to_vec(),
            plugin_slot: true,
        }),
        BackendMode::Openai => hosted_candidate
            .map(BackendPlan::HostedOnly)
            .ok_or_else(|| {
                AppError::Tool(
                    "no hosted web_search model configured; set capabilities.web_search=true on one models.toml entry".to_string(),
                )
            }),
        BackendMode::Tavily => Ok(BackendPlan::ExplicitHttp(BackendName::Tavily)),
        BackendMode::Brave => Ok(BackendPlan::ExplicitHttp(BackendName::Brave)),
        BackendMode::Serper => Ok(BackendPlan::ExplicitHttp(BackendName::Serper)),
        BackendMode::Plugin(name) => Ok(BackendPlan::ExplicitPlugin(name)),
    }
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

    pub fn is_retryable_unavailable(&self) -> bool {
        matches!(
            self,
            Self::MissingKey { .. }
                | Self::Incompatible { .. }
                | Self::Unauthorized { .. }
                | Self::RateLimited { .. }
                | Self::ServerError { .. }
                | Self::Timeout
                | Self::Transport { .. }
        )
    }

    pub fn is_explicit_degraded(&self) -> bool {
        matches!(
            self,
            Self::Unauthorized { .. }
                | Self::RateLimited { .. }
                | Self::ServerError { .. }
                | Self::Timeout
                | Self::Transport { .. }
        )
    }

    pub fn to_tool_error(&self, backend: &str) -> AppError {
        match self {
            Self::MissingKey { env_name } => AppError::Tool(format!(
                "web_search backend `{}` 未配置凭证；请设置 `{}`。",
                backend, env_name
            )),
            Self::Incompatible { detail } => AppError::Tool(detail.clone()),
            Self::InvalidRequest { status, detail } => AppError::Tool(format!(
                "web_search backend `{}` 请求不合法（status={}）：{}",
                backend, status, detail
            )),
            Self::Parse { detail } => AppError::Tool(format!(
                "web_search backend `{}` 返回解析失败：{}",
                backend, detail
            )),
            Self::Unauthorized { status } => AppError::Tool(format!(
                "web_search backend `{}` 鉴权失败（status={}）。",
                backend, status
            )),
            Self::RateLimited { status } | Self::ServerError { status } => AppError::Tool(format!(
                "web_search backend `{}` 暂不可用（status={}）。",
                backend, status
            )),
            Self::Timeout => AppError::Tool(format!("web_search backend `{}` 请求超时。", backend)),
            Self::Transport { detail } => AppError::Tool(format!(
                "web_search backend `{}` 网络错误：{}",
                backend, detail
            )),
        }
    }

    pub fn explicit_degraded_warnings(&self, backend: &str) -> Vec<String> {
        let mut warnings = vec![format!("backend_unavailable:{backend}")];
        match self {
            Self::RateLimited { status } | Self::ServerError { status } => warnings.push(format!(
                "rate_limited (backend={},status={status})",
                backend
            )),
            Self::Timeout => warnings.push(format!("timeout (backend={backend})")),
            _ => {}
        }
        warnings
    }

    pub fn auto_fallback_warnings(&self, backend: &str, fallback: Option<&str>) -> Vec<String> {
        let mut warnings = vec![match fallback {
            Some(next) => format!("backend_unavailable:{}, fallback={}", backend, next),
            None => format!("backend_unavailable:{backend}"),
        }];
        match self {
            Self::RateLimited { status } | Self::ServerError { status } => warnings.push(format!(
                "rate_limited (backend={},status={status})",
                backend
            )),
            Self::Timeout => warnings.push(format!("timeout (backend={backend})")),
            _ => {}
        }
        warnings
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
