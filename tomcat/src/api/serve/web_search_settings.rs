use std::path::Path;

use schemars::JsonSchema;
use serde::Serialize;

use crate::api::chat::collect_host_functions_from_catalog;
use crate::core::llm::admin::inferred_api_key_env;
use crate::core::llm::auth::KeySource;
use crate::core::tools::web_search::declarations::{
    effective_order, parse_declarations, BackendDeclaration, SEARCH_POINT,
};
use crate::ext::plugin::PluginCatalog;
use crate::infra::config::{validate_search_backend_order, with_config_lock};
use crate::{AppConfig, AppError};

use super::ServeState;

#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct SearchBackendView {
    id: String,
    label: String,
    host: Option<String>,
    model: Option<String>,
    key_env: Option<String>,
    key_source: Option<KeySource>,
    shared_model_ids: Vec<String>,
    can_save_key: bool,
}

#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct WebSearchSettingsPayload {
    backends: Vec<SearchBackendView>,
    order: Vec<String>,
    using_defaults: bool,
}

fn declarations(
    state: &ServeState,
    workspace: Option<&Path>,
) -> Result<(Vec<BackendDeclaration>, bool), AppError> {
    let catalog = match workspace {
        Some(root) => PluginCatalog::discover(&state.cfg, root)?,
        // Use the same agent and managed layers as chat, but no ambient project.
        None => PluginCatalog::discover_without_project(&state.cfg)?,
    };
    let (functions, _) = collect_host_functions_from_catalog(&catalog);
    let winner = functions
        .iter()
        .find(|function| function.point == SEARCH_POINT);
    let saved_keys = winner.is_some_and(|function| function.source.can_use_saved_keys());
    Ok((
        parse_declarations(winner.and_then(|f| f.metadata.as_ref())),
        saved_keys,
    ))
}

pub(super) fn read(
    state: &ServeState,
    workspace: Option<&Path>,
) -> Result<WebSearchSettingsPayload, AppError> {
    let (declarations, saved_keys) = declarations(state, workspace)?;
    let catalog = state.shared_model_catalog.snapshot();
    let models = catalog.entries_in_merge_order();
    let backends = declarations
        .iter()
        .map(|entry| {
            let (key_env, key_source) = entry
                .key_status(&catalog, saved_keys)
                .map(|(slot, source)| (Some(slot), source))
                .unwrap_or_default();
            let shared_model_ids = models
                .iter()
                .filter(|model| {
                    key_env
                        .as_ref()
                        .is_some_and(|slot| &inferred_api_key_env(model) == slot)
                })
                .map(|model| model.id.clone())
                .collect();
            SearchBackendView {
                id: entry.id.clone(),
                label: entry.label.clone(),
                host: entry.host.clone(),
                model: entry.model.clone(),
                key_env,
                key_source,
                shared_model_ids,
                can_save_key: saved_keys,
            }
        })
        .collect();
    let order = state.search_order.read();
    Ok(WebSearchSettingsPayload {
        backends,
        order: effective_order(order.as_deref(), &declarations),
        using_defaults: order.is_none(),
    })
}

pub(super) fn save_order(
    state: &ServeState,
    workspace: Option<&Path>,
    backends: Vec<String>,
) -> Result<(), AppError> {
    validate_search_backend_order(&backends)?;
    let (declarations, _) = declarations(state, workspace)?;
    if let Some(id) = backends
        .iter()
        .find(|id| !declarations.iter().any(|entry| &entry.id == *id))
    {
        return Err(AppError::Config(format!(
            "Unknown web search backend: {id}"
        )));
    }
    with_config_lock(&state.config_path, || {
        let content = match std::fs::read_to_string(&state.config_path) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(error.into()),
        };
        let mut table: toml::Table = content
            .parse()
            .map_err(|e: toml::de::Error| AppError::Config(e.to_string()))?;
        let mut section = &mut table;
        for name in ["tools", "web_search"] {
            section = section
                .entry(name)
                .or_insert_with(|| toml::Value::Table(toml::Table::new()))
                .as_table_mut()
                .ok_or_else(|| AppError::Config(format!("Expected TOML table: {name}")))?;
        }
        section.remove("backend");
        section.insert(
            "backends".into(),
            toml::Value::Array(backends.iter().cloned().map(toml::Value::String).collect()),
        );
        let rendered =
            toml::to_string_pretty(&table).map_err(|e| AppError::Config(e.to_string()))?;
        let checked: AppConfig =
            toml::from_str(&rendered).map_err(|e| AppError::Config(e.to_string()))?;
        crate::validate_config(&checked)?;
        crate::infra::platform::write_file_atomic(&state.config_path, rendered.as_bytes())?;
        *state.search_order.write() = Some(backends);
        Ok(())
    })
}
