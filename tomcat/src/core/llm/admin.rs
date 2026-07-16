use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

use fs2::FileExt;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

use crate::core::llm::auth::{
    env_name_for_provider, key_present_for_env, refresh_managed_credentials,
};
use crate::infra::config::{
    get_work_dir, read_env_entries, write_default_model, write_env_entries,
};
use crate::infra::platform::write_file_atomic;
use crate::{AppConfig, AppError};

use super::catalog::{
    load_user_models_file, render_user_models_file, Capabilities, ModelCatalog, ModelEntry,
    PartialCapabilities, UserModelEntry, UserModelsFile,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModelSource {
    Builtin,
    User,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ModelView {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_name: Option<String>,
    pub api: String,
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    pub capabilities: Capabilities,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
    pub source: ModelSource,
    pub api_key_env: String,
    pub key_present: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ModelEntryInput {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_name: Option<String>,
    pub api: String,
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default)]
    pub capabilities: Capabilities,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_format: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderKeyInput {
    pub env_name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ModelKeyStatus {
    pub env_name: String,
    pub key_present: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderKeyView {
    pub provider: String,
    pub env_name: String,
    pub key_present: bool,
    pub model_ids: Vec<String>,
}

impl ModelView {
    fn from_entry(catalog: &ModelCatalog, entry: ModelEntry) -> Self {
        let api_key_env = inferred_api_key_env(&entry);
        Self {
            id: entry.id.clone(),
            model_name: entry.model_name.clone(),
            api: entry.api.clone(),
            provider: entry.provider.clone(),
            base_url: entry.base_url.clone(),
            capabilities: entry.capabilities.clone(),
            thinking_format: entry.thinking_format.clone(),
            context_window: entry.context_window,
            source: if catalog.is_builtin_seed(&entry.id) {
                ModelSource::Builtin
            } else {
                ModelSource::User
            },
            key_present: key_present_for_env(&api_key_env),
            api_key_env,
        }
    }
}

impl ModelEntryInput {
    pub fn into_model_entry(self) -> Result<ModelEntry, AppError> {
        let id = self.id.trim().to_string();
        if id.is_empty() {
            return Err(AppError::Config("模型 id 不能为空。".to_string()));
        }
        let api = self.api.trim().to_string();
        if api.is_empty() {
            return Err(AppError::Config(format!("模型 `{id}` 的 api 不能为空。")));
        }
        let provider = self.provider.trim().to_string();
        if provider.is_empty() {
            return Err(AppError::Config(format!(
                "模型 `{id}` 的 provider 不能为空。"
            )));
        }
        let model_name = normalize_optional(self.model_name);
        let api_key_env = normalize_optional(self.api_key_env);
        if let Some(env_name) = api_key_env.as_deref() {
            validate_api_key_env_name(env_name)?;
        }
        let base_url = normalize_optional(self.base_url);
        let thinking_format = normalize_optional(self.thinking_format);
        Ok(ModelEntry {
            id,
            model_name,
            api,
            provider,
            api_key_env,
            base_url,
            capabilities: self.capabilities,
            context_window: self.context_window,
            thinking_format,
        })
    }
}

pub fn list_model_views(catalog: &ModelCatalog) -> Vec<ModelView> {
    catalog
        .entries_in_merge_order()
        .into_iter()
        .map(|entry| ModelView::from_entry(catalog, entry))
        .collect()
}

pub fn list_provider_keys(cfg: &AppConfig) -> Result<Vec<ProviderKeyView>, AppError> {
    let env_path = runtime_env_path(cfg)?;
    refresh_managed_credentials(&env_path)?;
    let vars = read_env_entries(&env_path)?;
    Ok(vars
        .into_iter()
        .filter(|(env_name, value)| {
            is_valid_api_key_env_name(env_name)
                && env_name.ends_with("_API_KEY")
                && !value.trim().is_empty()
        })
        .map(|(env_name, _)| ProviderKeyView {
            provider: String::new(),
            env_name,
            key_present: true,
            model_ids: Vec::new(),
        })
        .collect())
}

pub fn resolve_provider_key_env_name(catalog: &ModelCatalog, raw: &str) -> String {
    let candidate = raw.trim();
    if candidate.is_empty() {
        return String::new();
    }
    if is_valid_api_key_env_name(candidate) {
        return candidate.to_string();
    }
    if let Some(entry) = catalog
        .entries_in_merge_order()
        .into_iter()
        .find(|entry| entry.provider == candidate)
    {
        return inferred_api_key_env(&entry);
    }
    env_name_for_provider(candidate)
}

pub fn upsert_user_model(cfg: &AppConfig, input: ModelEntryInput) -> Result<ModelView, AppError> {
    let entry = input.into_model_entry()?;
    validate_mutable_model_entry(&entry)?;
    let path = ModelCatalog::default_user_path(cfg)?;
    with_file_lock(&models_lock_path(&path), || {
        let mut file = load_user_models_file(&path)?;
        upsert_user_model_entry(&mut file, model_entry_to_user_model(&entry));
        let rendered = render_user_models_file(&file)?;
        validate_and_write_models(cfg, &path, rendered.as_bytes())?;
        let reloaded = ModelCatalog::load_from_path(cfg, path.clone())?;
        let view = reloaded
            .lookup(&entry.id)
            .cloned()
            .map(|resolved| ModelView::from_entry(&reloaded, resolved))
            .ok_or_else(|| AppError::Config(format!("模型 `{}` 写入后未能重新加载。", entry.id)))?;
        Ok(view)
    })
}

pub fn remove_user_model(cfg: &AppConfig, model_id: &str) -> Result<(), AppError> {
    let trimmed = model_id.trim();
    if trimmed.is_empty() {
        return Err(AppError::Config("模型 id 不能为空。".to_string()));
    }
    let path = ModelCatalog::default_user_path(cfg)?;
    with_file_lock(&models_lock_path(&path), || {
        let current = ModelCatalog::load_from_path(cfg, path.clone())?;
        if !current.is_user_model(trimmed) {
            if current.lookup(trimmed).is_some() {
                return Err(AppError::Config(format!(
                    "模型 `{trimmed}` 是内置模型，不能删除；如需自定义请覆盖或仅配置 API Key。"
                )));
            }
            return Err(AppError::Config(format!("模型 `{trimmed}` 不存在。")));
        }
        ensure_model_not_in_use(cfg, trimmed)?;
        let mut file = load_user_models_file(&path)?;
        let before = file.models.len();
        file.models.retain(|entry| entry.id.trim() != trimmed);
        if file.models.len() == before {
            return Err(AppError::Config(format!(
                "模型 `{trimmed}` 不在用户 models.toml 中。"
            )));
        }
        let rendered = render_user_models_file(&file)?;
        validate_and_write_models(cfg, &path, rendered.as_bytes())?;
        Ok(())
    })
}

pub fn set_default_model(
    cfg: &AppConfig,
    config_path: &Path,
    model_id: &str,
) -> Result<(), AppError> {
    let trimmed = model_id.trim();
    if trimmed.is_empty() {
        return Err(AppError::Config("默认模型不能为空。".to_string()));
    }
    let catalog = ModelCatalog::load(cfg)?;
    catalog.lookup_explicit(trimmed)?;
    write_default_model(config_path, trimmed)
}

pub fn set_provider_key(
    cfg: &AppConfig,
    input: ProviderKeyInput,
) -> Result<ModelKeyStatus, AppError> {
    let env_name = input.env_name.trim().to_string();
    let value = input.value.trim().to_string();
    if !is_valid_api_key_env_name(&env_name) {
        return Err(AppError::Config(format!(
            "envName `{env_name}` 必须匹配大写环境变量格式 `^[A-Z_][A-Z0-9_]*$`。"
        )));
    }
    if value.is_empty() {
        return Err(AppError::Config(format!("`{env_name}` 不能为空。")));
    }
    let env_path = runtime_env_path(cfg)?;
    with_file_lock(&env_lock_path(&env_path), || {
        let mut vars = read_env_entries(&env_path)?;
        vars.insert(env_name.clone(), value);
        write_env_entries(&env_path, &vars)?;
        refresh_managed_credentials(&env_path)?;
        Ok(ModelKeyStatus {
            key_present: key_present_for_env(&env_name),
            env_name,
        })
    })
}

fn normalize_optional(value: Option<String>) -> Option<String> {
    value
        .map(|raw| raw.trim().to_string())
        .filter(|raw| !raw.is_empty())
}

fn validate_mutable_model_entry(entry: &ModelEntry) -> Result<(), AppError> {
    let registered = super::registered_provider_ids();
    if !registered.iter().any(|api| *api == entry.api) {
        return Err(AppError::Config(format!(
            "模型 `{}` 的 api=`{}` 未注册；可选值：{}。",
            entry.id,
            entry.api,
            registered.join(", ")
        )));
    }
    if entry.api == "anthropic-messages" && entry.capabilities.files {
        return Err(AppError::Config(format!(
            "模型 `{}` 的 api=`anthropic-messages` 当前不支持 files 附件能力，请关闭 files 或改用支持文件附件的 api。",
            entry.id
        )));
    }
    Ok(())
}

fn inferred_api_key_env(entry: &ModelEntry) -> String {
    entry
        .api_key_env
        .clone()
        .unwrap_or_else(|| env_name_for_provider(&entry.provider))
}

fn ensure_model_not_in_use(cfg: &AppConfig, model_id: &str) -> Result<(), AppError> {
    let mut refs = Vec::new();
    if cfg.llm.default_model.trim() == model_id {
        refs.push("llm.default_model".to_string());
    }
    if cfg.context.compaction_model.trim() == model_id {
        refs.push("context.compaction_model".to_string());
    }
    if cfg.llm.vision_model.as_deref().map(str::trim) == Some(model_id) {
        refs.push("llm.vision_model".to_string());
    }
    if cfg.llm.title_model.as_deref().map(str::trim) == Some(model_id) {
        refs.push("llm.title_model".to_string());
    }

    let sessions_path = crate::resolve_sessions_dir(cfg)?.join("sessions.json");
    let store = crate::load_store(&sessions_path)?;
    for entry in store.sessions.values().filter(|entry| {
        entry
            .model_override
            .as_deref()
            .map(str::trim)
            .is_some_and(|current| current == model_id)
    }) {
        let label = entry
            .title
            .as_deref()
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .map(|title| format!("session `{}` ({title})", entry.session_id))
            .unwrap_or_else(|| format!("session `{}`", entry.session_id));
        refs.push(label);
    }

    if refs.is_empty() {
        return Ok(());
    }
    Err(AppError::Config(format!(
        "模型 `{model_id}` 仍被以下位置引用：{}。请先切换这些位置的模型，再删除。",
        refs.join("、")
    )))
}

fn model_entry_to_user_model(entry: &ModelEntry) -> UserModelEntry {
    UserModelEntry {
        id: entry.id.clone(),
        model_name: entry.model_name.clone(),
        api: Some(entry.api.clone()),
        provider: Some(entry.provider.clone()),
        api_key_env: entry.api_key_env.clone(),
        base_url: entry.base_url.clone(),
        capabilities: Some(PartialCapabilities {
            vision: Some(entry.capabilities.vision),
            files: Some(entry.capabilities.files),
            tools: Some(entry.capabilities.tools),
            reasoning: Some(entry.capabilities.reasoning),
            web_search: Some(entry.capabilities.web_search),
        }),
        context_window: entry.context_window,
        thinking_format: entry.thinking_format.clone(),
    }
}

fn upsert_user_model_entry(file: &mut UserModelsFile, next: UserModelEntry) {
    if let Some(existing) = file.models.iter_mut().find(|entry| entry.id == next.id) {
        *existing = next;
    } else {
        file.models.push(next);
    }
}

fn validate_and_write_models(cfg: &AppConfig, path: &Path, content: &[u8]) -> Result<(), AppError> {
    let staged = staged_models_path(path);
    write_file_atomic(&staged, content)?;
    let validated = ModelCatalog::load_from_path(cfg, staged.clone());
    if let Err(error) = std::fs::remove_file(&staged) {
        if error.kind() != std::io::ErrorKind::NotFound {
            return Err(AppError::Io(error));
        }
    }
    validated?;
    write_file_atomic(path, content)
}

fn staged_models_path(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    parent.join(".models.toml.validate")
}

fn runtime_env_path(cfg: &AppConfig) -> Result<PathBuf, AppError> {
    Ok(get_work_dir(cfg)?.join("assets").join(".env"))
}

fn models_lock_path(path: &Path) -> PathBuf {
    sibling_lock_path(path, "models.toml.lock")
}

fn env_lock_path(path: &Path) -> PathBuf {
    sibling_lock_path(path, ".env.lock")
}

fn sibling_lock_path(path: &Path, file_name: &str) -> PathBuf {
    path.parent()
        .unwrap_or_else(|| Path::new("."))
        .join(file_name)
}

fn with_file_lock<T>(
    lock_path: &Path,
    work: impl FnOnce() -> Result<T, AppError>,
) -> Result<T, AppError> {
    const LOCK_TIMEOUT: Duration = Duration::from_secs(10);
    const LOCK_RETRY: Duration = Duration::from_millis(50);

    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent).map_err(AppError::Io)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(lock_path)
        .map_err(AppError::Io)?;
    let deadline = Instant::now() + LOCK_TIMEOUT;
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => break,
            Err(error) if Instant::now() < deadline => {
                if error.kind() != std::io::ErrorKind::WouldBlock {
                    return Err(AppError::Io(error));
                }
                std::thread::sleep(LOCK_RETRY);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                return Err(AppError::Config(format!(
                    "等待文件锁超时（{}ms）：{}",
                    LOCK_TIMEOUT.as_millis(),
                    lock_path.display()
                )));
            }
            Err(error) => return Err(AppError::Io(error)),
        }
    }
    let result = work();
    file.unlock().map_err(AppError::Io)?;
    result
}

fn validate_api_key_env_name(env_name: &str) -> Result<(), AppError> {
    if !is_valid_api_key_env_name(env_name) {
        return Err(AppError::Config(format!(
            "api_key_env `{env_name}` 必须匹配大写环境变量格式 `^[A-Z_][A-Z0-9_]*$`。"
        )));
    }
    Ok(())
}

fn is_valid_api_key_env_name(candidate: &str) -> bool {
    let mut chars = candidate.chars();
    matches!(chars.next(), Some(ch) if ch.is_ascii_uppercase() || ch == '_')
        && chars.all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '_')
}
