use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use tracing::warn;

use crate::infra::config::{AppConfig, LlmRuntimeConfig};
use crate::infra::error::AppError;

use super::auth::{AuthStore, Credential};
use super::catalog::{infer_default_base_url, Capabilities, ModelCatalog, ModelEntry};
use super::provider::LlmProvider;
use super::registry::build_provider;
use super::thinking_policy::{thinking_format_for_model, ThinkingFormat};
use super::{ChatMessage, ChatMessageContent, ChatMessageContentPart};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmScene {
    Main,
    Compaction,
    Vision,
    Title,
}

pub struct ResolvedCall {
    pub provider_impl: Arc<dyn LlmProvider>,
    pub model: String,
    pub api: String,
    pub provider: String,
    pub base_url: Option<String>,
    pub key_source: String,
    pub thinking_format: ThinkingFormat,
    pub capabilities: Capabilities,
}

impl std::fmt::Debug for ResolvedCall {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedCall")
            .field("model", &self.model)
            .field("api", &self.api)
            .field("provider", &self.provider)
            .field("base_url", &self.base_url)
            .field("key_source", &self.key_source)
            .field("thinking_format", &self.thinking_format)
            .field("capabilities", &self.capabilities)
            .finish()
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CapabilityRequirements {
    pub vision: bool,
    pub files: bool,
}

impl CapabilityRequirements {
    fn for_scene(scene: LlmScene) -> Self {
        match scene {
            LlmScene::Vision => Self {
                vision: true,
                files: false,
            },
            _ => Self::default(),
        }
    }

    fn merge(self, other: Self) -> Self {
        Self {
            vision: self.vision || other.vision,
            files: self.files || other.files,
        }
    }

    fn satisfied_by(self, capabilities: &Capabilities) -> bool {
        (!self.vision || capabilities.vision) && (!self.files || capabilities.files)
    }

    fn missing_labels(self, capabilities: &Capabilities) -> Vec<&'static str> {
        let mut labels = Vec::new();
        if self.vision && !capabilities.vision {
            labels.push("vision");
        }
        if self.files && !capabilities.files {
            labels.push("files");
        }
        labels
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ProviderCacheKey {
    api: String,
    base_url: Option<String>,
    key_source: String,
}

pub fn capability_requirements_for_messages(messages: &[ChatMessage]) -> CapabilityRequirements {
    let mut requirements = CapabilityRequirements::default();
    for message in messages {
        if let Some(ChatMessageContent::Parts(parts)) = &message.content {
            for part in parts {
                match part {
                    ChatMessageContentPart::InputImage { .. } => {
                        requirements.vision = true;
                    }
                    ChatMessageContentPart::InputFile { .. } => {
                        requirements.files = true;
                    }
                    ChatMessageContentPart::InputText { .. } => {}
                }
            }
        }
    }
    requirements
}

pub fn validate_capabilities(
    catalog: &ModelCatalog,
    default_model: &str,
    scene: LlmScene,
    model_id: &str,
    capabilities: &Capabilities,
    messages: &[ChatMessage],
) -> Result<(), AppError> {
    let requirements = CapabilityRequirements::for_scene(scene)
        .merge(capability_requirements_for_messages(messages));
    if requirements.satisfied_by(capabilities) {
        return Ok(());
    }

    let suggested = catalog
        .entries()
        .into_iter()
        .find(|candidate| {
            candidate.id != model_id && requirements.satisfied_by(&candidate.capabilities)
        })
        .map(|candidate| candidate.id)
        .unwrap_or_else(|| default_model.to_string());
    let missing = requirements.missing_labels(capabilities).join("/");
    Err(AppError::Llm(format!(
        "provider/model 不支持 {}，建议改用 `{}`。",
        missing, suggested
    )))
}

pub trait LlmResolver: Send + Sync {
    fn resolve(
        &self,
        scene: LlmScene,
        session_override: Option<&str>,
    ) -> Result<ResolvedCall, AppError>;
}

pub struct DefaultLlmResolver {
    config: AppConfig,
    catalog: Arc<ModelCatalog>,
    auth: AuthStore,
    provider_cache: Mutex<HashMap<ProviderCacheKey, Arc<dyn LlmProvider>>>,
}

impl DefaultLlmResolver {
    pub fn new(config: AppConfig, catalog: Arc<ModelCatalog>) -> Self {
        Self {
            config,
            catalog,
            auth: AuthStore,
            provider_cache: Mutex::new(HashMap::new()),
        }
    }

    fn select_model_id(&self, scene: LlmScene, session_override: Option<&str>) -> String {
        match scene {
            LlmScene::Main => session_override
                .map(str::trim)
                .filter(|model| !model.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| self.config.llm.default_model.clone()),
            LlmScene::Compaction => {
                let model = self.config.context.compaction_model.trim();
                if model.is_empty() {
                    self.config.llm.default_model.clone()
                } else {
                    model.to_string()
                }
            }
            LlmScene::Vision => self
                .config
                .llm
                .vision_model
                .as_deref()
                .map(str::trim)
                .filter(|model| !model.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| {
                    session_override
                        .map(str::trim)
                        .filter(|model| !model.is_empty())
                        .unwrap_or(&self.config.llm.default_model)
                        .to_string()
                }),
            LlmScene::Title => self
                .config
                .llm
                .title_model
                .as_deref()
                .map(str::trim)
                .filter(|model| !model.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| {
                    let fallback = self.config.context.compaction_model.trim();
                    if fallback.is_empty() {
                        self.config.llm.default_model.clone()
                    } else {
                        fallback.to_string()
                    }
                }),
        }
    }

    fn lookup_entry(&self, model_id: &str) -> Result<ModelEntry, AppError> {
        self.catalog.lookup_explicit(model_id)
    }

    fn guard_scene(&self, scene: LlmScene, entry: &ModelEntry) -> Result<(), AppError> {
        validate_capabilities(
            &self.catalog,
            &self.config.llm.default_model,
            scene,
            &entry.id,
            &entry.capabilities,
            &[],
        )
    }

    fn credential_for(
        &self,
        entry: &ModelEntry,
        compatible_fallback_env: Option<&str>,
    ) -> Result<Credential, AppError> {
        self.auth.get(entry, compatible_fallback_env)
    }

    fn compatible_fallback_env<'a>(
        &'a self,
        scene: LlmScene,
        entry: &ModelEntry,
    ) -> Option<&'a str> {
        match scene {
            LlmScene::Compaction => self.compaction_fallback_env(entry),
            _ => self.test_fallback_env(),
        }
    }

    fn compaction_fallback_env<'a>(&'a self, entry: &ModelEntry) -> Option<&'a str> {
        let default_model = self.config.llm.default_model.trim();
        if default_model.is_empty() || entry.id == default_model {
            return None;
        }
        let Some(default_entry) = self.catalog.lookup(default_model) else {
            return None;
        };
        if default_entry.provider == entry.provider {
            default_entry.api_key_env.as_deref()
        } else {
            None
        }
    }

    fn effective_base_url(&self, entry: &ModelEntry) -> Option<String> {
        #[cfg(test)]
        if let Some(base_url) = self.config.llm.api_base.clone() {
            return Some(base_url);
        }
        entry
            .base_url
            .clone()
            .or_else(|| infer_default_base_url(Some(entry.provider.as_str())))
            .or_else(|| infer_default_base_url(Some(entry.api.as_str())))
    }

    #[cfg(test)]
    fn test_fallback_env(&self) -> Option<&str> {
        self.config.llm.api_key_env.as_deref()
    }

    #[cfg(not(test))]
    fn test_fallback_env(&self) -> Option<&str> {
        None
    }

    fn resolved_thinking_format(&self, entry: &ModelEntry) -> ThinkingFormat {
        let model_name = entry.request_model_name();
        match entry.thinking_format.as_deref() {
            Some(format) => {
                ThinkingFormat::parse_or_auto(Some(format)).resolve_for_model(model_name)
            }
            None => match self.config.llm.thinking.format.as_deref() {
                Some(format) => {
                    ThinkingFormat::parse_or_auto(Some(format)).resolve_for_model(model_name)
                }
                None => thinking_format_for_model(model_name),
            },
        }
    }

    fn runtime(&self) -> LlmRuntimeConfig {
        self.config.llm.runtime()
    }

    fn provider_cache_key(&self, entry: &ModelEntry, credential: &Credential) -> ProviderCacheKey {
        ProviderCacheKey {
            api: entry.api.clone(),
            base_url: self.effective_base_url(entry),
            key_source: credential.env_name.clone(),
        }
    }

    fn resolve_cached_provider(
        &self,
        entry: &ModelEntry,
        credential: &Credential,
    ) -> Result<Arc<dyn LlmProvider>, AppError> {
        let cache_key = self.provider_cache_key(entry, credential);
        if let Some(existing) = self.provider_cache.lock().get(&cache_key).cloned() {
            return Ok(existing);
        }

        let runtime = self.runtime();
        let provider = build_provider(entry, &runtime, credential)?;
        let mut cache = self.provider_cache.lock();
        Ok(cache
            .entry(cache_key)
            .or_insert_with(|| provider.clone())
            .clone())
    }

    fn resolve_model_call(
        &self,
        scene: LlmScene,
        model_id: &str,
    ) -> Result<ResolvedCall, AppError> {
        let entry = self.lookup_entry(model_id)?;
        self.guard_scene(scene, &entry)?;
        let compatible_fallback_env = self.compatible_fallback_env(scene, &entry);
        let credential = self.credential_for(&entry, compatible_fallback_env)?;
        let provider_impl = self.resolve_cached_provider(&entry, &credential)?;
        let base_url = self.effective_base_url(&entry);
        Ok(ResolvedCall {
            provider_impl,
            model: entry.request_model_name().to_string(),
            api: entry.api.clone(),
            provider: entry.provider.clone(),
            base_url,
            key_source: credential.env_name,
            thinking_format: self.resolved_thinking_format(&entry),
            capabilities: entry.capabilities.clone(),
        })
    }

    fn resolve_compaction_call(&self, model_id: &str) -> Result<ResolvedCall, AppError> {
        let selected_model = model_id.trim();
        let default_model = self.config.llm.default_model.trim();
        match self.resolve_model_call(LlmScene::Compaction, selected_model) {
            Ok(resolved) => Ok(resolved),
            Err(original_err) if !default_model.is_empty() && selected_model != default_model => {
                warn!(
                    compaction_model = selected_model,
                    fallback_model = default_model,
                    error = %original_err,
                    "compaction model unavailable, falling back to default model"
                );
                match self.resolve_model_call(LlmScene::Compaction, default_model) {
                    Ok(resolved) => Ok(resolved),
                    Err(fallback_err) => Err(AppError::Config(format!(
                        "压缩模型 `{}` 不可用，回退默认模型 `{}` 也失败。原始错误：{}；回退错误：{}",
                        selected_model, default_model, original_err, fallback_err
                    ))),
                }
            }
            Err(original_err) => Err(original_err),
        }
    }
}

impl LlmResolver for DefaultLlmResolver {
    fn resolve(
        &self,
        scene: LlmScene,
        session_override: Option<&str>,
    ) -> Result<ResolvedCall, AppError> {
        let model_id = self.select_model_id(scene, session_override);
        match scene {
            LlmScene::Compaction => self.resolve_compaction_call(&model_id),
            _ => self.resolve_model_call(scene, &model_id),
        }
    }
}
