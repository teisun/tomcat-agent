use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::RwLock;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::infra::config::{get_work_dir, AppConfig, ContextConfig};
use crate::infra::error::AppError;

const BUILTIN_MODELS_TOML: &str = include_str!("builtin_models.toml");

fn default_tools_enabled() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Capabilities {
    #[serde(default)]
    pub vision: bool,
    #[serde(default)]
    pub files: bool,
    #[serde(default = "default_tools_enabled")]
    pub tools: bool,
    #[serde(default)]
    pub reasoning: bool,
    #[serde(default)]
    pub web_search: bool,
}

impl Default for Capabilities {
    fn default() -> Self {
        Self {
            vision: false,
            files: false,
            tools: true,
            reasoning: false,
            web_search: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ModelEntry {
    pub id: String,
    #[serde(default)]
    pub model_name: Option<String>,
    pub api: String,
    pub provider: String,
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub capabilities: Capabilities,
    #[serde(default)]
    pub context_window: Option<u32>,
    #[serde(default)]
    pub thinking_format: Option<String>,
}

impl ModelEntry {
    pub fn request_model_name(&self) -> &str {
        self.model_name.as_deref().unwrap_or(self.id.as_str())
    }
}

#[derive(Debug, Clone)]
pub struct ModelCatalog {
    by_id: HashMap<String, ModelEntry>,
    user_path: PathBuf,
    ordered_ids: Vec<String>,
    user_ids: HashSet<String>,
}

#[derive(Debug, Clone)]
pub struct SharedModelCatalog {
    inner: Arc<RwLock<Arc<ModelCatalog>>>,
}

impl ModelCatalog {
    pub fn load(config: &AppConfig) -> Result<Self, AppError> {
        let user_path = Self::default_user_path(config)?;
        Self::load_from_path(config, user_path)
    }

    pub fn load_from_path(config: &AppConfig, user_path: PathBuf) -> Result<Self, AppError> {
        let mut by_id = HashMap::new();
        let mut ordered_ids = Vec::new();
        let mut user_ids = HashSet::new();
        for entry in builtin_seed_entries_result(&config.context)? {
            ordered_ids.push(entry.id.clone());
            by_id.insert(entry.id.clone(), entry);
        }
        if user_path.exists() {
            let file = load_user_models_file(&user_path)?;
            for raw in file.models {
                let model_id = raw.id.clone();
                user_ids.insert(model_id.clone());
                let merged = merge_user_model(raw, by_id.remove(&model_id), &config.context)?;
                if !ordered_ids.iter().any(|existing| existing == &merged.id) {
                    ordered_ids.push(merged.id.clone());
                }
                by_id.insert(merged.id.clone(), merged);
            }
        }
        Ok(Self {
            by_id,
            user_path,
            ordered_ids,
            user_ids,
        })
    }

    pub fn default_user_path(config: &AppConfig) -> Result<PathBuf, AppError> {
        Ok(get_work_dir(config)?.join("models.toml"))
    }

    pub fn user_path(&self) -> &Path {
        &self.user_path
    }

    pub fn lookup(&self, model_id: &str) -> Option<&ModelEntry> {
        self.by_id.get(model_id.trim())
    }

    pub fn lookup_explicit(&self, model_id: &str) -> Result<ModelEntry, AppError> {
        self.lookup(model_id)
            .cloned()
            .ok_or_else(|| missing_model_error(model_id, &self.user_path))
    }

    pub fn entries(&self) -> Vec<ModelEntry> {
        let mut entries: Vec<_> = self.by_id.values().cloned().collect();
        entries.sort_by(|a, b| a.id.cmp(&b.id));
        entries
    }

    pub fn entries_in_merge_order(&self) -> Vec<ModelEntry> {
        self.ordered_ids
            .iter()
            .filter_map(|id| self.by_id.get(id).cloned())
            .collect()
    }

    pub fn is_user_model(&self, model_id: &str) -> bool {
        self.user_ids.contains(model_id.trim())
    }
}

impl SharedModelCatalog {
    pub fn load(config: &AppConfig) -> Result<Self, AppError> {
        Ok(Self::from(ModelCatalog::load(config)?))
    }

    pub fn snapshot(&self) -> Arc<ModelCatalog> {
        self.inner.read().clone()
    }

    pub fn replace(&self, catalog: ModelCatalog) -> Arc<ModelCatalog> {
        let next = Arc::new(catalog);
        *self.inner.write() = next.clone();
        next
    }

    pub fn reload(&self, config: &AppConfig) -> Result<Arc<ModelCatalog>, AppError> {
        let user_path = self.snapshot().user_path().to_path_buf();
        let next = ModelCatalog::load_from_path(config, user_path)?;
        Ok(self.replace(next))
    }

    pub fn lookup(&self, model_id: &str) -> Option<ModelEntry> {
        self.snapshot().lookup(model_id).cloned()
    }

    pub fn lookup_explicit(&self, model_id: &str) -> Result<ModelEntry, AppError> {
        self.snapshot().lookup_explicit(model_id)
    }

    pub fn entries(&self) -> Vec<ModelEntry> {
        self.snapshot().entries()
    }

    pub fn entries_in_merge_order(&self) -> Vec<ModelEntry> {
        self.snapshot().entries_in_merge_order()
    }

    pub fn is_user_model(&self, model_id: &str) -> bool {
        self.snapshot().is_user_model(model_id)
    }

    pub fn user_path(&self) -> PathBuf {
        self.snapshot().user_path().to_path_buf()
    }

    pub fn with_catalog<R>(&self, f: impl FnOnce(&ModelCatalog) -> R) -> R {
        let snapshot = self.snapshot();
        f(snapshot.as_ref())
    }
}

impl From<ModelCatalog> for SharedModelCatalog {
    fn from(value: ModelCatalog) -> Self {
        Self {
            inner: Arc::new(RwLock::new(Arc::new(value))),
        }
    }
}

impl From<Arc<ModelCatalog>> for SharedModelCatalog {
    fn from(value: Arc<ModelCatalog>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(value)),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct UserModelsFile {
    #[serde(default)]
    pub(crate) models: Vec<UserModelEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct UserModelEntry {
    pub(crate) id: String,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) model_name: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) api: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) provider: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) api_key_env: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) base_url: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) capabilities: Option<PartialCapabilities>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) context_window: Option<u32>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) thinking_format: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct PartialCapabilities {
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) vision: Option<bool>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) files: Option<bool>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tools: Option<bool>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reasoning: Option<bool>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) web_search: Option<bool>,
}

pub(crate) fn load_user_models_file(path: &Path) -> Result<UserModelsFile, AppError> {
    if !path.exists() {
        return Ok(UserModelsFile::default());
    }
    let content = std::fs::read_to_string(path).map_err(AppError::Io)?;
    toml::from_str(&content).map_err(|e| {
        AppError::Config(format!(
            "解析 models.toml 失败（{}）：{}",
            path.display(),
            e
        ))
    })
}

pub(crate) fn render_user_models_file(file: &UserModelsFile) -> Result<String, AppError> {
    toml::to_string_pretty(file)
        .map(|text| format!("{text}\n"))
        .map_err(|e| AppError::Config(format!("序列化 models.toml 失败: {e}")))
}

pub(crate) fn builtin_seed_toml_text() -> &'static str {
    BUILTIN_MODELS_TOML
}

#[cfg(test)]
pub(crate) fn builtin_seed_entries(context: &ContextConfig) -> Vec<ModelEntry> {
    builtin_seed_entries_result(context)
        .unwrap_or_else(|err| panic!("解析内嵌 builtin_models.toml 失败: {err}"))
}

pub(crate) fn builtin_seed_entries_result(
    context: &ContextConfig,
) -> Result<Vec<ModelEntry>, AppError> {
    let file = toml::from_str::<UserModelsFile>(BUILTIN_MODELS_TOML)
        .map_err(|e| AppError::Config(format!("解析内嵌 builtin_models.toml 失败: {e}")))?;
    file.models
        .into_iter()
        .map(|raw| merge_user_model(raw, None, context))
        .collect()
}

fn merge_user_model(
    raw: UserModelEntry,
    existing: Option<ModelEntry>,
    context: &ContextConfig,
) -> Result<ModelEntry, AppError> {
    let mut merged = existing.unwrap_or_else(|| ModelEntry {
        id: raw.id.clone(),
        model_name: None,
        api: String::new(),
        provider: String::new(),
        api_key_env: None,
        base_url: None,
        capabilities: Capabilities::default(),
        context_window: Some(context.context_window as u32),
        thinking_format: None,
    });
    merged.id = raw.id.clone();
    if let Some(model_name) = raw.model_name {
        merged.model_name = Some(model_name);
    }
    if let Some(api) = raw.api {
        merged.api = api;
    } else if merged.api.trim().is_empty() {
        return Err(AppError::Config(format!(
            "models.toml 中模型 `{}` 必须显式填写 `api`。",
            raw.id.trim()
        )));
    }
    if let Some(provider) = raw.provider {
        merged.provider = provider;
    } else if merged.provider.trim().is_empty() {
        return Err(AppError::Config(format!(
            "models.toml 中模型 `{}` 必须显式填写 `provider`。",
            raw.id.trim()
        )));
    }
    if let Some(api_key_env) = raw.api_key_env {
        merged.api_key_env = Some(api_key_env);
    }
    if let Some(base_url) = raw.base_url {
        merged.base_url = Some(base_url);
    } else if merged.base_url.is_none() {
        merged.base_url = infer_default_base_url(Some(merged.provider.as_str()));
    }
    if let Some(capabilities) = raw.capabilities {
        apply_partial_capabilities(&mut merged.capabilities, capabilities);
    }
    if let Some(context_window) = raw.context_window {
        merged.context_window = Some(context_window);
    }
    if let Some(thinking_format) = raw.thinking_format {
        merged.thinking_format = Some(thinking_format);
    }
    Ok(merged)
}

fn apply_partial_capabilities(target: &mut Capabilities, partial: PartialCapabilities) {
    if let Some(vision) = partial.vision {
        target.vision = vision;
    }
    if let Some(files) = partial.files {
        target.files = files;
    }
    if let Some(tools) = partial.tools {
        target.tools = tools;
    }
    if let Some(reasoning) = partial.reasoning {
        target.reasoning = reasoning;
    }
    if let Some(web_search) = partial.web_search {
        target.web_search = web_search;
    }
}

fn missing_model_error(model_id: &str, user_path: &Path) -> AppError {
    AppError::Config(format!(
        "模型 `{}` 未收录，请补 {} 或切回已收录模型。",
        model_id.trim(),
        user_path.display()
    ))
}

pub(crate) fn infer_default_base_url(provider: Option<&str>) -> Option<String> {
    match provider.unwrap_or_default() {
        "openai" | "openai-responses" => Some("https://api.openai.com".to_string()),
        "deepseek" => Some("https://api.deepseek.com".to_string()),
        "mimo" => Some("https://token-plan-cn.xiaomimimo.com".to_string()),
        "zhipu" => Some("https://open.bigmodel.cn/api/paas/v4".to_string()),
        "moonshot" => Some("https://api.moonshot.cn".to_string()),
        "anthropic" | "anthropic-messages" => Some("https://api.anthropic.com".to_string()),
        _ => None,
    }
}
