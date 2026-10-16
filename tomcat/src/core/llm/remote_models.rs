//! Lists the model names an upstream endpoint advertises for the provider dialog.
//! Only names are returned: capabilities still come from the catalog or the user.

use std::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::infra::config::LlmRuntimeConfig;

use super::auth::read_env_value;
use super::endpoint::build_path_aware_endpoint;
use super::http_client::build_http_client;

const LIST_TIMEOUT: Duration = Duration::from_secs(10);
const ANTHROPIC_PAGE_LIMIT: &str = "1000";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RemoteModelView {
    pub id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RemoteModelsFailureReason {
    /// 401/403: the key is wrong or lacks permission.
    Auth,
    /// The endpoint does not offer a model list (404/405 and other client errors).
    Unsupported,
    /// The address cannot be reached, or the server failed.
    Network,
    Timeout,
    /// The response is not a `{ data: [{ id }] }` model list.
    Format,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RemoteModelsFailure {
    pub reason: RemoteModelsFailureReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
}

impl RemoteModelsFailure {
    fn new(reason: RemoteModelsFailureReason, status: Option<u16>) -> Self {
        Self { reason, status }
    }

    /// Developer-facing summary; it never contains the request key.
    pub fn message(&self) -> String {
        let reason = match self.reason {
            RemoteModelsFailureReason::Auth => "remote_models_auth",
            RemoteModelsFailureReason::Unsupported => "remote_models_unsupported",
            RemoteModelsFailureReason::Network => "remote_models_network",
            RemoteModelsFailureReason::Timeout => "remote_models_timeout",
            RemoteModelsFailureReason::Format => "remote_models_format",
        };
        match self.status {
            Some(status) => format!("{reason}: HTTP {status}"),
            None => reason.to_string(),
        }
    }
}

/// One-off listing request. A key typed in the dialog is used only for this call;
/// otherwise the saved or environment key for `api_key_env` is read.
#[derive(Clone)]
pub struct RemoteModelsRequest {
    pub base_url: String,
    pub api: String,
    pub api_key: Option<String>,
    pub api_key_env: Option<String>,
}

pub async fn list_remote_models(
    runtime: &LlmRuntimeConfig,
    request: RemoteModelsRequest,
) -> Result<Vec<RemoteModelView>, RemoteModelsFailure> {
    list_remote_models_within(runtime, request, LIST_TIMEOUT).await
}

pub(crate) async fn list_remote_models_within(
    runtime: &LlmRuntimeConfig,
    request: RemoteModelsRequest,
    timeout: Duration,
) -> Result<Vec<RemoteModelView>, RemoteModelsFailure> {
    let network = || RemoteModelsFailure::new(RemoteModelsFailureReason::Network, None);
    let client = build_http_client(runtime, None).map_err(|_| network())?;
    let key = request
        .api_key
        .map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty())
        .or_else(|| {
            request
                .api_key_env
                .as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .and_then(read_env_value)
        });
    let url = build_path_aware_endpoint(&request.base_url, "models");
    let mut builder = client.get(url).timeout(timeout);
    if request.api == "anthropic-messages" {
        builder = builder
            .query(&[("limit", ANTHROPIC_PAGE_LIMIT)])
            .header("anthropic-version", "2023-06-01");
        if let Some(key) = key.as_deref() {
            builder = builder.header("x-api-key", key);
        }
    } else if let Some(key) = key.as_deref() {
        builder = builder.bearer_auth(key);
    }
    let response = builder.send().await.map_err(|error| {
        if error.is_timeout() {
            RemoteModelsFailure::new(RemoteModelsFailureReason::Timeout, None)
        } else {
            network()
        }
    })?;
    let status = response.status();
    if !status.is_success() {
        let reason = match status.as_u16() {
            401 | 403 => RemoteModelsFailureReason::Auth,
            400..=499 => RemoteModelsFailureReason::Unsupported,
            _ => RemoteModelsFailureReason::Network,
        };
        return Err(RemoteModelsFailure::new(reason, Some(status.as_u16())));
    }
    let body = response.bytes().await.map_err(|error| {
        if error.is_timeout() {
            RemoteModelsFailure::new(RemoteModelsFailureReason::Timeout, None)
        } else {
            network()
        }
    })?;
    parse_model_list(&body)
        .ok_or_else(|| RemoteModelsFailure::new(RemoteModelsFailureReason::Format, None))
}

fn parse_model_list(body: &[u8]) -> Option<Vec<RemoteModelView>> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    let mut ids = Vec::new();
    for item in value.get("data")?.as_array()? {
        let id = item.get("id")?.as_str()?.trim();
        if !id.is_empty() && !ids.iter().any(|known: &RemoteModelView| known.id == id) {
            ids.push(RemoteModelView { id: id.to_string() });
        }
    }
    Some(ids)
}
