use std::collections::BTreeMap;
use std::path::Path;
use std::sync::OnceLock;

use parking_lot::RwLock;

use crate::infra::config::read_env_entries;
use crate::infra::error::AppError;

use super::catalog::ModelEntry;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credential {
    pub provider: String,
    pub env_name: String,
    pub value: String,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct AuthStore;

#[derive(Debug, Default)]
struct ManagedCredentialState {
    values: BTreeMap<String, String>,
    generations: BTreeMap<String, u64>,
}

fn managed_credential_state() -> &'static RwLock<ManagedCredentialState> {
    static STATE: OnceLock<RwLock<ManagedCredentialState>> = OnceLock::new();
    STATE.get_or_init(|| RwLock::new(ManagedCredentialState::default()))
}

impl AuthStore {
    pub fn get(
        &self,
        entry: &ModelEntry,
        fallback_env: Option<&str>,
    ) -> Result<Credential, AppError> {
        self.get_for_provider(&entry.provider, entry.api_key_env.as_deref(), fallback_env)
    }

    pub fn get_for_provider(
        &self,
        provider: &str,
        preferred_env: Option<&str>,
        fallback_env: Option<&str>,
    ) -> Result<Credential, AppError> {
        let provider = provider.trim();
        let preferred_env = preferred_env
            .map(str::trim)
            .filter(|env| !env.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| env_name_for_provider(provider));
        if let Some(value) = read_env_value(&preferred_env) {
            return Ok(Credential {
                provider: provider.to_string(),
                env_name: preferred_env.clone(),
                value,
            });
        }

        if let Some(fallback_env) = fallback_env
            .map(str::trim)
            .filter(|env| !env.is_empty() && *env != preferred_env.as_str())
        {
            if let Some(value) = read_env_value(fallback_env) {
                return Ok(Credential {
                    provider: provider.to_string(),
                    env_name: fallback_env.to_string(),
                    value,
                });
            }
        }

        Err(AppError::Config(missing_key_message(
            provider,
            &preferred_env,
            fallback_env,
        )))
    }
}

pub fn env_name_for_provider(provider: &str) -> String {
    let normalized = provider
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    format!("{}_API_KEY", normalized)
}

pub fn missing_key_message(
    provider: &str,
    inferred_env: &str,
    fallback_env: Option<&str>,
) -> String {
    match fallback_env
        .map(str::trim)
        .filter(|env| !env.is_empty() && *env != inferred_env)
    {
        Some(fallback) => format!(
            "未找到 provider=`{}` 的凭证，请设置 `{}`（或兼容回退 `{}`）。",
            provider.trim(),
            inferred_env,
            fallback
        ),
        None => format!(
            "未找到 provider=`{}` 的凭证，请设置 `{}`。",
            provider.trim(),
            inferred_env
        ),
    }
}

pub fn refresh_managed_credentials(env_path: &Path) -> Result<(), AppError> {
    let vars = read_env_entries(env_path);
    refresh_managed_credentials_from_entries(&vars);
    Ok(())
}

pub fn refresh_managed_credentials_from_entries(vars: &BTreeMap<String, String>) {
    let mut next_values = BTreeMap::new();
    for (key, value) in vars {
        let key = key.trim();
        let value = value.trim();
        if !key.is_empty() && !value.is_empty() {
            next_values.insert(key.to_string(), value.to_string());
        }
    }

    let mut state = managed_credential_state().write();
    for (key, value) in &next_values {
        if state.values.get(key) != Some(value) {
            *state.generations.entry(key.clone()).or_insert(0) += 1;
        }
    }
    let removed_keys: Vec<String> = state
        .values
        .keys()
        .filter(|key| !next_values.contains_key(*key))
        .cloned()
        .collect();
    for key in removed_keys {
        *state.generations.entry(key).or_insert(0) += 1;
    }
    state.values = next_values;
}

pub fn credential_generation(env_name: &str) -> u64 {
    managed_credential_state()
        .read()
        .generations
        .get(env_name)
        .copied()
        .unwrap_or(0)
}

pub fn key_present_for_env(env_name: &str) -> bool {
    read_env_value(env_name).is_some()
}

#[cfg(test)]
pub fn clear_managed_credentials_for_test() {
    let mut state = managed_credential_state().write();
    state.values.clear();
    state.generations.clear();
}

fn read_env_value(env_name: &str) -> Option<String> {
    if let Some(value) = managed_credential_state()
        .read()
        .values
        .get(env_name)
        .cloned()
    {
        return Some(value);
    }
    std::env::var(env_name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}
