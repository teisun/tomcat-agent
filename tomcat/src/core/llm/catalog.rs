use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::infra::config::{get_work_dir, AppConfig, ContextConfig, LlmConfig};
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
}

impl Default for Capabilities {
    fn default() -> Self {
        Self {
            vision: false,
            files: false,
            tools: true,
            reasoning: false,
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
    pub api: String,
    pub provider: String,
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

#[derive(Debug, Clone)]
pub struct ModelCatalog {
    by_id: HashMap<String, ModelEntry>,
    user_path: PathBuf,
}

impl ModelCatalog {
    pub fn load(config: &AppConfig) -> Result<Self, AppError> {
        let user_path = Self::default_user_path(config)?;
        Self::load_from_path(config, user_path)
    }

    pub fn load_from_path(config: &AppConfig, user_path: PathBuf) -> Result<Self, AppError> {
        let mut by_id = builtin_models(&config.context);
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
                let merged =
                    merge_user_model(raw, by_id.remove(&model_id), &config.llm, &config.context);
                by_id.insert(merged.id.clone(), merged);
            }
        }
        Ok(Self { by_id, user_path })
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
    api: Option<String>,
    #[serde(default)]
    provider: Option<String>,
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
}

fn builtin_models(context: &ContextConfig) -> HashMap<String, ModelEntry> {
    let mut by_id = HashMap::new();
    let builtins = vec![
        ModelEntry {
            id: "gpt-5.4".to_string(),
            api: "openai-responses".to_string(),
            provider: "openai".to_string(),
            base_url: Some("https://api.openai.com".to_string()),
            capabilities: Capabilities {
                vision: true,
                files: true,
                tools: true,
                reasoning: true,
            },
            context_window: Some(context.context_window as u32),
            cost: None,
            thinking_format: None,
        },
        ModelEntry {
            id: "gpt-5.2".to_string(),
            api: "openai-responses".to_string(),
            provider: "openai".to_string(),
            base_url: Some("https://api.openai.com".to_string()),
            capabilities: Capabilities {
                vision: true,
                files: true,
                tools: true,
                reasoning: true,
            },
            context_window: Some(context.context_window as u32),
            cost: None,
            thinking_format: None,
        },
        ModelEntry {
            id: "deepseek-v4-pro".to_string(),
            api: "openai".to_string(),
            provider: "deepseek".to_string(),
            base_url: Some("https://api.deepseek.com".to_string()),
            capabilities: Capabilities {
                vision: false,
                files: false,
                tools: true,
                reasoning: true,
            },
            context_window: Some(context.context_window as u32),
            cost: None,
            thinking_format: Some("deepseek".to_string()),
        },
        ModelEntry {
            id: "deepseek-v4-flash".to_string(),
            api: "openai".to_string(),
            provider: "deepseek".to_string(),
            base_url: Some("https://api.deepseek.com".to_string()),
            capabilities: Capabilities {
                vision: false,
                files: false,
                tools: true,
                reasoning: true,
            },
            context_window: Some(context.context_window as u32),
            cost: None,
            thinking_format: Some("deepseek".to_string()),
        },
    ];
    for entry in builtins {
        by_id.insert(entry.id.clone(), entry);
    }
    by_id
}

fn merge_user_model(
    raw: UserModelEntry,
    existing: Option<ModelEntry>,
    llm: &LlmConfig,
    context: &ContextConfig,
) -> ModelEntry {
    let default_provider = infer_provider_from_model_id(&raw.id)
        .or_else(|| infer_provider_from_env(llm.api_key_env.as_deref()))
        .unwrap_or_else(|| llm.provider.clone());
    let default_api = infer_api_from_model_id(&raw.id).unwrap_or_else(|| llm.provider.clone());
    let mut merged = existing.unwrap_or_else(|| ModelEntry {
        id: raw.id.clone(),
        api: default_api,
        provider: default_provider,
        base_url: llm
            .api_base
            .clone()
            .or_else(|| infer_default_base_url(infer_provider_from_model_id(&raw.id).as_deref())),
        capabilities: infer_capabilities_from_model_id(&raw.id),
        context_window: Some(context.context_window as u32),
        cost: None,
        thinking_format: None,
    });
    merged.id = raw.id.clone();
    if let Some(api) = raw.api {
        merged.api = api;
    }
    if let Some(provider) = raw.provider {
        merged.provider = provider;
    }
    if let Some(base_url) = raw.base_url {
        merged.base_url = Some(base_url);
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
    merged
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
}

fn missing_model_error(model_id: &str, user_path: &Path) -> AppError {
    AppError::Config(format!(
        "模型 `{}` 未收录，请补 {} 或切回已收录模型。",
        model_id.trim(),
        user_path.display()
    ))
}

pub(crate) fn infer_provider_from_env(env_name: Option<&str>) -> Option<String> {
    let env = env_name?.trim();
    env.strip_suffix("_API_KEY")
        .filter(|prefix| !prefix.is_empty())
        .map(|prefix| prefix.to_ascii_lowercase())
}

pub(crate) fn infer_provider_from_model_id(model_id: &str) -> Option<String> {
    let lower = model_id.trim().to_ascii_lowercase();
    if lower.starts_with("deepseek-") {
        Some("deepseek".to_string())
    } else if lower.starts_with("gpt-") || lower.starts_with("o1") || lower.starts_with("o3") {
        Some("openai".to_string())
    } else if lower.starts_with("claude-") {
        Some("anthropic".to_string())
    } else {
        None
    }
}

pub(crate) fn infer_api_from_model_id(model_id: &str) -> Option<String> {
    let lower = model_id.trim().to_ascii_lowercase();
    if lower.starts_with("deepseek-") {
        Some("openai".to_string())
    } else if lower.starts_with("gpt-") || lower.starts_with("o1") || lower.starts_with("o3") {
        Some("openai-responses".to_string())
    } else {
        None
    }
}

pub(crate) fn infer_default_base_url(provider: Option<&str>) -> Option<String> {
    match provider.unwrap_or_default() {
        "openai" | "openai-responses" => Some("https://api.openai.com".to_string()),
        "deepseek" => Some("https://api.deepseek.com".to_string()),
        _ => None,
    }
}

pub(crate) fn infer_capabilities_from_model_id(model_id: &str) -> Capabilities {
    let lower = model_id.trim().to_ascii_lowercase();
    if lower.starts_with("deepseek-v4-") || lower.starts_with("gpt-5.") {
        Capabilities {
            vision: lower.starts_with("gpt-"),
            files: lower.starts_with("gpt-"),
            tools: true,
            reasoning: true,
        }
    } else {
        Capabilities::default()
    }
}
