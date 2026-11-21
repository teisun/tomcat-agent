//! Static backend declarations from the winning web_search host function.
use std::collections::BTreeSet;

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::core::llm::{
    admin::{inferred_api_key_env, is_valid_api_key_env_name},
    auth::{key_source_for_env, process_env_value, KeySource},
    ModelCatalog,
};
use crate::infra::config::is_search_backend_id;

pub(crate) const SEARCH_POINT: &str = "web_search.backend";

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackendDeclaration {
    pub id: String,
    pub label: String,
    pub secret: Option<String>,
    pub host: Option<String>,
    pub model: Option<String>,
    #[serde(default)]
    pub default_auto: bool,
}

impl BackendDeclaration {
    /// The key name this backend uses and where its value currently comes from.
    /// Model backends are called through the host LLM layer, so layer limits do not apply.
    /// `None` when the model is missing, or when a model at another address uses the
    /// same key name: that value belongs to the other address.
    pub(crate) fn key_status(
        &self,
        catalog: &ModelCatalog,
        saved_keys: bool,
    ) -> Option<(String, Option<KeySource>)> {
        let slot = match (&self.model, &self.secret, &self.host) {
            (Some(id), _, _) => inferred_api_key_env(catalog.lookup(id)?),
            (None, Some(secret), Some(host)) if !used_at_other_host(catalog, secret, host) => {
                secret.clone()
            }
            _ => return None,
        };
        let source = if saved_keys || self.model.is_some() {
            key_source_for_env(&slot)
        } else {
            process_env_value(&slot).map(|_| KeySource::Environment)
        };
        Some((slot, source))
    }
}

/// Credentials are bound to exact HTTPS DNS hosts on the default port.
fn credential_host(url: &reqwest::Url) -> Option<String> {
    if url.scheme() != "https"
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    let domain = url.domain()?.trim_end_matches('.');
    domain.contains('.').then(|| domain.to_ascii_lowercase())
}

/// A model without such an address counts as another host.
fn used_at_other_host(catalog: &ModelCatalog, slot: &str, host: &str) -> bool {
    catalog.iter_in_merge_order().any(|model| {
        inferred_api_key_env(model) == slot
            && model
                .base_url
                .as_deref()
                .and_then(|url| reqwest::Url::parse(url).ok())
                .as_ref()
                .and_then(credential_host)
                .as_deref()
                != Some(host)
    })
}

fn is_credential_host(host: &str) -> bool {
    reqwest::Url::parse(&format!("https://{host}/"))
        .ok()
        .and_then(|url| credential_host(&url))
        .is_some_and(|normalized| normalized == host)
}

pub(crate) fn parse_declarations(metadata: Option<&Map<String, Value>>) -> Vec<BackendDeclaration> {
    let Some(entries) = metadata.and_then(|data| data.get("backends")) else {
        return Vec::new();
    };
    let Some(entries) = entries.as_array() else {
        tracing::warn!("web_search.backend metadata.backends must be an array");
        return Vec::new();
    };
    let mut seen = BTreeSet::new();
    entries
        .iter()
        .enumerate()
        .filter_map(|(index, value)| {
            if let Ok(entry) = serde_json::from_value::<BackendDeclaration>(value.clone()) {
                let valid = is_search_backend_id(&entry.id)
                    && !entry.label.trim().is_empty()
                    && match (&entry.secret, &entry.model, &entry.host) {
                        (Some(secret), None, Some(host)) => {
                            is_valid_api_key_env_name(secret) && is_credential_host(host)
                        }
                        (None, Some(model), _) => !model.trim().is_empty(),
                        _ => false,
                    };
                if valid && seen.insert(entry.id.clone()) {
                    return Some(entry);
                }
            }
            tracing::warn!(index, "ignoring invalid web_search backend declaration");
            None
        })
        .collect()
}

pub(crate) fn effective_order(
    explicit: Option<&[String]>,
    declarations: &[BackendDeclaration],
) -> Vec<String> {
    explicit.map(<[String]>::to_vec).unwrap_or_else(|| {
        declarations
            .iter()
            .filter(|entry| entry.default_auto)
            .map(|entry| entry.id.clone())
            .collect()
    })
}
