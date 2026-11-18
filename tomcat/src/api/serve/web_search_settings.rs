use std::path::Path;

use schemars::JsonSchema;
use serde::Serialize;

use crate::api::chat::collect_host_functions_from_catalog;
use crate::core::llm::admin::inferred_api_key_env;
use crate::core::llm::auth::{key_source_for_env, process_env_value, KeySource};
use crate::core::tools::web_search::backend::discover_hosted_candidate;
use crate::core::tools::web_search::declarations::{
    effective_order, parse_declarations, BackendDeclaration, OPENAI, SEARCH_POINT,
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
    let hosted = discover_hosted_candidate(&catalog).filter(|m| m.api == "openai-responses");
    let models = catalog.entries_in_merge_order();
    let to_view =
        |id, label, host, model: Option<String>, key_env: Option<String>, can_save_key| {
            let (key_source, shared_model_ids) = key_env
                .as_deref()
                .map(|slot| {
                    let source = if can_save_key || model.is_some() {
                        key_source_for_env(slot)
                    } else {
                        process_env_value(slot).map(|_| KeySource::Environment)
                    };
                    let ids = models
                        .iter()
                        .filter(|entry| inferred_api_key_env(entry) == slot)
                        .map(|entry| entry.id.clone())
                        .collect();
                    (source, ids)
                })
                .unwrap_or_default();
            SearchBackendView {
                id,
                label,
                host,
                model,
                key_env,
                key_source,
                shared_model_ids,
                can_save_key,
            }
        };
    let order = state.search_order.read();
    let mut backends = vec![to_view(
        OPENAI.into(),
        "OpenAI".into(),
        None,
        hosted.as_ref().map(|m| m.id.clone()),
        hosted.map(|m| m.key_env),
        true,
    )];
    for entry in &declarations {
        backends.push(to_view(
            entry.id.clone(),
            entry.label.clone(),
            entry.host.clone(),
            entry.model.clone(),
            entry.key_slot(&catalog),
            saved_keys,
        ));
    }
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
        .find(|id| id.as_str() != OPENAI && !declarations.iter().any(|entry| &entry.id == *id))
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
