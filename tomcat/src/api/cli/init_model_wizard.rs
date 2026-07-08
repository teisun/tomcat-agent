use std::collections::BTreeSet;
use std::path::Path;

use dialoguer::{Confirm, Password, Select};

use crate::core::llm::{env_name_for_provider, ModelCatalog, ModelEntry};
use crate::{AppConfig, AppError};

pub(crate) use crate::infra::config::{read_env_entries, write_env_entries};

#[derive(Debug, Clone)]
pub(crate) struct InitModelChoice {
    pub entry: ModelEntry,
    pub env_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyConfigStatus {
    AlreadyConfigured,
    Written,
    Skipped,
}

pub(crate) fn run_model_wizard(
    cfg: &mut AppConfig,
    catalog: &ModelCatalog,
) -> Result<InitModelChoice, AppError> {
    let entries = catalog.entries();
    if entries.is_empty() {
        return Err(AppError::Config(
            "模型 catalog 为空，无法执行 init 向导。".to_string(),
        ));
    }
    let default_index = entries
        .iter()
        .position(|entry| entry.id == cfg.llm.default_model)
        .unwrap_or(0);
    let labels: Vec<String> = entries
        .iter()
        .map(|entry| {
            format!(
                "{} (api={} provider={})",
                entry.id, entry.api, entry.provider
            )
        })
        .collect();
    let selected_index = Select::new()
        .with_prompt("  选择默认模型")
        .items(&labels)
        .default(default_index)
        .interact_opt()
        .unwrap_or(None)
        .unwrap_or(default_index);
    Ok(apply_model_choice(cfg, &entries[selected_index]))
}

pub(crate) fn apply_model_choice(cfg: &mut AppConfig, entry: &ModelEntry) -> InitModelChoice {
    let env_name = provider_env_name(entry);
    cfg.llm.default_model = entry.id.clone();
    cfg.context.compaction_model = entry.id.clone();
    InitModelChoice {
        entry: entry.clone(),
        env_name,
    }
}

fn provider_env_name(entry: &ModelEntry) -> String {
    entry
        .api_key_env
        .clone()
        .unwrap_or_else(|| env_name_for_provider(&entry.provider))
}

pub(crate) fn additional_provider_env_names(
    catalog: &ModelCatalog,
    selected_env_name: &str,
) -> Vec<String> {
    catalog
        .entries()
        .into_iter()
        .map(|entry| provider_env_name(&entry))
        .filter(|env_name| env_name != selected_env_name)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub(crate) fn prompt_additional_provider_keys(
    env_path: &Path,
    env_names: &[String],
) -> Result<Vec<(String, KeyConfigStatus)>, AppError> {
    let env_names: Vec<String> = env_names
        .iter()
        .map(|env_name| env_name.trim())
        .filter(|env_name| !env_name.is_empty())
        .map(str::to_string)
        .collect();
    if env_names.is_empty() {
        return Ok(Vec::new());
    }

    let should_prompt = Confirm::new()
        .with_prompt("  是否顺手补充其它 provider 的 API Key（便于后续 /model use）")
        .default(false)
        .interact_opt()
        .unwrap_or(None)
        .unwrap_or(false);
    if !should_prompt {
        return Ok(Vec::new());
    }

    let mut results = Vec::new();
    for env_name in env_names {
        let status = prompt_and_store_provider_key(env_path, &env_name)?;
        results.push((env_name, status));
    }
    Ok(results)
}

pub(crate) fn prompt_and_store_provider_key(
    env_path: &Path,
    env_name: &str,
) -> Result<KeyConfigStatus, AppError> {
    let mut vars = read_env_entries(env_path);
    if vars
        .get(env_name)
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
    {
        return Ok(KeyConfigStatus::AlreadyConfigured);
    }

    let value: String = Password::new()
        .with_prompt(format!("  输入 {}（回车跳过）", env_name))
        .allow_empty_password(true)
        .interact()
        .unwrap_or_default();
    if value.trim().is_empty() {
        return Ok(KeyConfigStatus::Skipped);
    }

    vars.insert(env_name.to_string(), value);
    write_env_entries(env_path, &vars)?;
    crate::core::llm::auth::refresh_managed_credentials(env_path)?;
    Ok(KeyConfigStatus::Written)
}

