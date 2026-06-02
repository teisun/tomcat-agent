use std::collections::BTreeMap;
use std::path::Path;

use dialoguer::{Password, Select};

use crate::core::llm::{env_name_for_provider, ModelCatalog, ModelEntry};
use crate::{AppConfig, AppError};

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
    let env_name = env_name_for_provider(&entry.provider);
    cfg.llm.default_model = entry.id.clone();
    cfg.llm.provider = entry.api.clone();
    cfg.llm.api_base = persisted_api_base(entry);
    cfg.llm.api_key_env = Some(env_name.clone());
    InitModelChoice {
        entry: entry.clone(),
        env_name,
    }
}

fn persisted_api_base(entry: &ModelEntry) -> Option<String> {
    match (entry.api.as_str(), entry.base_url.as_deref()) {
        ("openai", Some("https://api.openai.com"))
        | ("openai-responses", Some("https://api.openai.com")) => None,
        _ => entry.base_url.clone(),
    }
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
    Ok(KeyConfigStatus::Written)
}

fn read_env_entries(env_path: &Path) -> BTreeMap<String, String> {
    let mut vars = BTreeMap::new();
    if !env_path.exists() {
        return vars;
    }
    if let Ok(iter) = dotenvy::from_path_iter(env_path) {
        for (key, value) in iter.flatten() {
            if !key.trim().is_empty() {
                vars.insert(key, value);
            }
        }
    }
    vars
}

pub(crate) fn write_env_entries(
    env_path: &Path,
    vars: &BTreeMap<String, String>,
) -> Result<(), AppError> {
    if let Some(parent) = env_path.parent() {
        std::fs::create_dir_all(parent).map_err(AppError::Io)?;
    }

    let mut lines =
        vec!["# tomcat runtime credentials — 此文件由 tomcat init 生成，权限 0600".to_string()];
    for (key, value) in vars.iter().filter(|(key, _)| !is_proxy_key(key)) {
        lines.push(format!("{key}={value}"));
    }
    lines.push(String::new());
    lines.push("# 如需通过代理访问大模型，取消以下注释并填入代理地址：".to_string());
    for key in ["HTTPS_PROXY", "HTTP_PROXY", "ALL_PROXY"] {
        match vars.get(key) {
            Some(value) => lines.push(format!("{key}={value}")),
            None => lines.push(format!("# {}={}", key, proxy_placeholder(key))),
        }
    }
    std::fs::write(env_path, format!("{}\n", lines.join("\n"))).map_err(AppError::Io)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(env_path, perms).map_err(AppError::Io)?;
    }

    Ok(())
}

fn is_proxy_key(key: &str) -> bool {
    matches!(key, "HTTPS_PROXY" | "HTTP_PROXY" | "ALL_PROXY")
}

fn proxy_placeholder(key: &str) -> &'static str {
    match key {
        "ALL_PROXY" => "socks5://127.0.0.1:7890",
        _ => "http://127.0.0.1:7890",
    }
}
