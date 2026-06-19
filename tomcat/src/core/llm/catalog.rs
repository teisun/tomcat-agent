use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::infra::config::{get_work_dir, AppConfig, ContextConfig};
use crate::infra::error::AppError;

fn default_tools_enabled() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cost {
    #[serde(default)]
    pub input_per_mtok: Option<f64>,
    #[serde(default)]
    pub output_per_mtok: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    pub cost: Option<Cost>,
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
}

impl ModelCatalog {
    pub fn load(config: &AppConfig) -> Result<Self, AppError> {
        let user_path = Self::default_user_path(config)?;
        Self::load_from_path(config, user_path)
    }

    pub fn load_from_path(config: &AppConfig, user_path: PathBuf) -> Result<Self, AppError> {
        let mut by_id = HashMap::new();
        let mut ordered_ids = Vec::new();
        for entry in builtin_models(&config.context) {
            ordered_ids.push(entry.id.clone());
            by_id.insert(entry.id.clone(), entry);
        }
        if user_path.exists() {
            let content = std::fs::read_to_string(&user_path).map_err(AppError::Io)?;
            let file: UserModelsFile = toml::from_str(&content).map_err(|e| {
                AppError::Config(format!(
                    "解析 models.toml 失败（{}）：{}",
                    user_path.display(),
                    e
                ))
            })?;
            for raw in file.models {
                let model_id = raw.id.clone();
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
}

#[derive(Debug, Deserialize)]
struct UserModelsFile {
    #[serde(default)]
    models: Vec<UserModelEntry>,
}

#[derive(Debug, Clone, Deserialize)]
struct UserModelEntry {
    id: String,
    #[serde(default)]
    model_name: Option<String>,
    #[serde(default)]
    api: Option<String>,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    api_key_env: Option<String>,
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    capabilities: Option<PartialCapabilities>,
    #[serde(default)]
    context_window: Option<u32>,
    #[serde(default)]
    cost: Option<Cost>,
    #[serde(default)]
    thinking_format: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct PartialCapabilities {
    #[serde(default)]
    vision: Option<bool>,
    #[serde(default)]
    files: Option<bool>,
    #[serde(default)]
    tools: Option<bool>,
    #[serde(default)]
    reasoning: Option<bool>,
    #[serde(default)]
    web_search: Option<bool>,
}

fn builtin_models(context: &ContextConfig) -> Vec<ModelEntry> {
    vec![
        ModelEntry {
            id: "gpt-5.4".to_string(),
            model_name: None,
            api: "openai-responses".to_string(),
            provider: "openai".to_string(),
            api_key_env: None,
            base_url: Some("https://api.openai.com".to_string()),
            capabilities: Capabilities {
                vision: true,
                files: true,
                tools: true,
                reasoning: true,
                web_search: false,
            },
            context_window: Some(context.context_window as u32),
            cost: None,
            thinking_format: None,
        },
        ModelEntry {
            id: "deepseek-v4-pro".to_string(),
            model_name: None,
            api: "openai".to_string(),
            provider: "deepseek".to_string(),
            api_key_env: None,
            base_url: Some("https://api.deepseek.com".to_string()),
            capabilities: Capabilities {
                vision: false,
                files: false,
                tools: true,
                reasoning: true,
                web_search: false,
            },
            context_window: Some(context.context_window as u32),
            cost: None,
            thinking_format: Some("deepseek".to_string()),
        },
    ]
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
        cost: None,
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
    if let Some(cost) = raw.cost {
        merged.cost = Some(cost);
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
        _ => None,
    }
}
