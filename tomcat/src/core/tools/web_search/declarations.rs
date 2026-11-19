//! Static backend declarations from the winning web_search host function.
use std::collections::BTreeSet;

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::core::llm::{
    admin::{credential_host, inferred_api_key_env, resolve_host_key},
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
    pub(crate) fn uses_host_key(&self) -> bool {
        self.secret.is_none() && self.model.is_none()
    }

    /// The key name this backend uses and where its value currently comes from.
    /// Model backends are called through the host LLM layer, so layer limits do not apply.
    pub(crate) fn key_status(
        &self,
        catalog: &ModelCatalog,
        saved_keys: bool,
    ) -> Option<(String, Option<KeySource>)> {
        let slot = match (&self.model, &self.secret) {
            (Some(id), _) => inferred_api_key_env(catalog.lookup(id)?),
            (None, Some(secret)) => secret.clone(),
            (None, None) if saved_keys => {
                return resolve_host_key(catalog, self.host.as_deref()?, key_source_for_env);
            }
            (None, None) => return None,
        };
        let source = if saved_keys || self.model.is_some() {
            key_source_for_env(&slot)
        } else {
            process_env_value(&slot).map(|_| KeySource::Environment)
        };
        Some((slot, source))
    }
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
                    && match (&entry.secret, &entry.model) {
                        (Some(secret), None) => {
                            crate::core::llm::admin::is_valid_api_key_env_name(secret)
                        }
                        (None, Some(model)) => !model.trim().is_empty(),
                        (None, None) => entry.host.as_deref().is_some_and(|host| {
                            reqwest::Url::parse(&format!("https://{host}/"))
                                .ok()
                                .and_then(|url| credential_host(&url))
                                .is_some_and(|normalized| normalized == host)
                        }),
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
