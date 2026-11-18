//! Static backend declarations from the winning web_search host function.
use std::collections::BTreeSet;

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::core::llm::{admin::inferred_api_key_env, ModelCatalog};
use crate::infra::config::is_search_backend_id;

pub(crate) const SEARCH_POINT: &str = "web_search.backend";
pub(crate) const OPENAI: &str = "openai";

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
    pub(crate) fn key_slot(&self, catalog: &ModelCatalog) -> Option<String> {
        match self.model.as_deref() {
            Some(id) => catalog.lookup(id).map(inferred_api_key_env),
            None => self.secret.clone(),
        }
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
                    && entry.id != OPENAI
                    && !entry.label.trim().is_empty()
                    && match (&entry.secret, &entry.model) {
                        (Some(secret), None) => {
                            crate::core::llm::admin::is_valid_api_key_env_name(secret)
                        }
                        (None, Some(model)) => !model.trim().is_empty(),
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
        std::iter::once(OPENAI.to_owned())
            .chain(
                declarations
                    .iter()
                    .filter(|entry| entry.default_auto)
                    .map(|entry| entry.id.clone()),
            )
            .collect()
    })
}
