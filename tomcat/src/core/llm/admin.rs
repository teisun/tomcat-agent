use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

use fs2::FileExt;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

use crate::core::llm::auth::{
    env_name_for_provider, key_present_for_env, refresh_managed_credentials,
};
use crate::core::session::{
    clear_model_overrides_in_store, precheck_model_override_store, ModelPrefsStore,
};
use crate::infra::config::{
    clear_model_references, get_work_dir, read_env_entries, write_default_model, write_env_entries,
    ThinkingConfig,
};
use crate::infra::platform::write_file_atomic;
use crate::{AppConfig, AppError};

use super::catalog::{
    load_user_models_file, render_user_models_file, validate_context_window_options,
    validate_supported_speeds, Capabilities, ModelCatalog, ModelEntry, PartialCapabilities,
    UserModelEntry, UserModelsFile,
};
use super::thinking_policy::{
    clamp_reasoning_level, default_thinking_format_for_api, normalize_supported_reasoning_levels,
    resolve_request_fields, safe_supported_reasoning_levels_for, ThinkingFormat,
};
use super::Speed;

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
    #[serde(default)]
    pub context_window_options: Vec<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_context_window: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_reasoning_level: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    #[serde(default)]
    pub supported_reasoning_levels: Vec<String>,
    #[serde(default)]
    pub supported_speeds: Vec<Speed>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_speed: Option<Speed>,
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
    pub context_window_options: Option<Vec<u32>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supported_reasoning_levels: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supported_speeds: Option<Vec<Speed>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpsertModelResult {
    pub model: ModelView,
    pub warnings: Vec<String>,
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
            context_window_options: entry.context_window_options.clone(),
            selected_context_window: None,
            selected_reasoning_level: None,
            description: entry.description.clone(),
            max_output_tokens: entry.max_output_tokens,
            supported_reasoning_levels: entry.supported_reasoning_levels.clone(),
            supported_speeds: entry.supported_speeds.clone(),
            selected_speed: entry.resolve_speed(Speed::Standard),
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
        let thinking_format = normalize_optional(self.thinking_format)
            .or_else(|| Some(default_thinking_format_for_api(api.as_str()).to_string()));
        let context_window_options = validate_context_window_options(
            &id,
            self.context_window,
            self.context_window_options.as_deref().unwrap_or_default(),
            self.max_output_tokens,
        )?;
        let description = normalize_optional(self.description);
        let supported_reasoning_levels = match self.supported_reasoning_levels {
            Some(levels) => {
                let normalized = normalize_supported_reasoning_levels(&levels);
                if normalized.is_empty() && !levels.is_empty() {
                    safe_supported_reasoning_levels_for(api.as_str(), thinking_format.as_deref())
                } else {
                    normalized
                }
            }
            None => safe_supported_reasoning_levels_for(api.as_str(), thinking_format.as_deref()),
        };
        let supported_speeds = validate_supported_speeds(
            &id,
            &api,
            self.supported_speeds.as_deref().unwrap_or_default(),
        )?;
        Ok(ModelEntry {
            id,
            model_name,
            api,
            provider,
            api_key_env,
            base_url,
            capabilities: self.capabilities,
            context_window: self.context_window,
            context_window_options,
            max_output_tokens: self.max_output_tokens,
            description,
            thinking_format,
            supported_reasoning_levels,
            supported_speeds,
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

pub fn list_model_views_with_prefs(
    catalog: &ModelCatalog,
    prefs: &ModelPrefsStore,
) -> Vec<ModelView> {
    catalog
        .entries_in_merge_order()
        .into_iter()
        .map(|entry| {
            let mut view = ModelView::from_entry(catalog, entry.clone());
            let explicit_prefs = prefs.explicit_prefs_for(&entry.id);
            view.selected_reasoning_level =
                (!entry.supported_reasoning_levels.is_empty()).then(|| {
                    let requested = explicit_prefs
                        .as_ref()
                        .map(|prefs| prefs.reasoning)
                        .unwrap_or_else(|| prefs.default_reasoning());
                    clamp_reasoning_level(requested, &entry.supported_reasoning_levels)
                        .as_str()
                        .to_string()
                });
            view.selected_speed = entry.resolve_speed(prefs.speed_for(&entry.id));
            view.selected_context_window = explicit_prefs
                .and_then(|prefs| prefs.context_window)
                .filter(|value| entry.context_window_options.contains(value))
                .or_else(|| {
                    entry.context_window.filter(|value| {
                        entry.context_window_options.is_empty()
                            || entry.context_window_options.contains(value)
                    })
                });
            view
        })
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

pub fn upsert_user_model(
    cfg: &AppConfig,
    input: ModelEntryInput,
) -> Result<UpsertModelResult, AppError> {
    let entry = input.into_model_entry()?;
    let warnings = collect_model_warnings(&entry);
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
        Ok(UpsertModelResult {
            model: view,
            warnings: warnings.clone(),
        })
    })
}

/// 在 models.toml 锁内从磁盘加载当前目录，并在同一临界区完成依赖该目录的写入。
///
/// 删除和选择模型都通过此入口，以免一个请求已基于旧缓存验证模型、另一个请求
/// 随后删除该模型后，前者又把已删除的 ID 写回会话或默认配置。
pub fn with_current_model_catalog<T>(
    cfg: &AppConfig,
    work: impl FnOnce(&ModelCatalog) -> Result<T, AppError>,
) -> Result<T, AppError> {
    let path = ModelCatalog::default_user_path(cfg)?;
    with_file_lock(&models_lock_path(&path), || {
        let catalog = ModelCatalog::load_from_path(cfg, path.clone())?;
        work(&catalog)
    })
}

pub fn remove_user_model(cfg: &AppConfig, model_id: &str) -> Result<(), AppError> {
    remove_user_model_with_config_path(cfg, None, model_id)
}

/// 删除用户模型，并在提供真实配置路径时一并忘掉所有持久化的同名选择。
///
/// CLI / serve 必须传配置路径；保留无路径 wrapper 仅供旧的内存隔离测试使用，不能用于
/// 用户实际删除操作。
pub fn remove_user_model_with_config_path(
    cfg: &AppConfig,
    config_path: Option<&Path>,
    model_id: &str,
) -> Result<(), AppError> {
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

        let keeps_builtin_entry = current.is_builtin_seed(trimmed);
        let session_stores = session_store_paths(cfg)?;
        if !keeps_builtin_entry {
            for store_path in &session_stores {
                precheck_model_override_store(store_path)?;
            }
        }

        let mut cleared = Vec::new();
        if !keeps_builtin_entry {
            if let Some(config_path) = config_path {
                let references = clear_model_references(config_path, trimmed)?;
                cleared.extend(references.into_iter().map(str::to_string));
            }
            for store_path in &session_stores {
                match clear_model_overrides_in_store(store_path, trimmed) {
                    Ok(0) => {}
                    Ok(count) => {
                        cleared.push(format!("{} 个会话选择 ({})", count, store_path.display()))
                    }
                    Err(error) => return Err(removal_partial_error(trimmed, &cleared, error)),
                }
            }
        }

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

fn session_store_paths(cfg: &AppConfig) -> Result<Vec<PathBuf>, AppError> {
    let agents_dir = get_work_dir(cfg)?.join("agents");
    let entries = match std::fs::read_dir(&agents_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(AppError::Io(error)),
    };
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(AppError::Io)?;
        if !entry.file_type().map_err(AppError::Io)?.is_dir() {
            continue;
        }
        let store_path = entry.path().join("sessions").join("sessions.json");
        match std::fs::metadata(&store_path) {
            Ok(_) => paths.push(store_path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(AppError::Io(error)),
        }
    }
    paths.sort();
    Ok(paths)
}

fn removal_partial_error(model_id: &str, cleared: &[String], error: AppError) -> AppError {
    if cleared.is_empty() {
        return error;
    }
    AppError::Config(format!(
        "模型 `{model_id}` 尚未删除；已清理 {}；原因: {error}",
        cleared.join("、")
    ))
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
    with_current_model_catalog(cfg, |catalog| {
        catalog.lookup_explicit(trimmed)?;
        write_default_model(config_path, trimmed)
    })
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
        context_window_options: (!entry.context_window_options.is_empty())
            .then(|| entry.context_window_options.clone()),
        max_output_tokens: entry.max_output_tokens,
        description: entry.description.clone(),
        thinking_format: entry.thinking_format.clone(),
        supported_reasoning_levels: Some(entry.supported_reasoning_levels.clone()),
        supported_speeds: Some(entry.supported_speeds.clone()),
        extra: toml::Table::new(),
    }
}

fn collect_model_warnings(entry: &ModelEntry) -> Vec<String> {
    if !entry.capabilities.reasoning {
        return Vec::new();
    }
    let mut warnings = Vec::new();
    if matches!(entry.api.as_str(), "openai" | "openai-responses") {
        let fmt = ThinkingFormat::parse_or_auto(entry.thinking_format.as_deref())
            .resolve_for_api(entry.api.as_str());
        let probe = resolve_request_fields(&ThinkingConfig::default(), fmt);
        if probe.reasoning_effort.is_none() {
            warnings.push(format!(
                "API `{}` expects reasoning effort, but thinking_format=`{}` will not send it. Tomcat will omit reasoning depth for this model.",
                entry.api,
                fmt.as_str(),
            ));
        }
    }
    warnings
}

fn upsert_user_model_entry(file: &mut UserModelsFile, mut next: UserModelEntry) {
    if let Some(existing) = file.models.iter_mut().find(|entry| entry.id == next.id) {
        next.extra = std::mem::take(&mut existing.extra);
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
    if let Err(error) = file.unlock() {
        tracing::warn!(path = %lock_path.display(), error = %error, "model file lock release failed");
    }
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
