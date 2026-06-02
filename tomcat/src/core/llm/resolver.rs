use std::sync::Arc;

use crate::infra::config::{AppConfig, LlmConfig};
use crate::infra::error::AppError;

use super::auth::{AuthStore, Credential};
use super::catalog::{
    infer_default_base_url, legacy_entry_for, Capabilities, ModelCatalog, ModelEntry,
};
use super::provider::LlmProvider;
use super::registry::resolve_llm;
use super::thinking_policy::{thinking_format_for_model, ThinkingFormat};

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
}

impl DefaultLlmResolver {
    pub fn new(config: AppConfig, catalog: Arc<ModelCatalog>) -> Self {
        Self {
            config,
            catalog,
            auth: AuthStore,
        }
    }

    fn select_model_id(&self, scene: LlmScene, session_override: Option<&str>) -> (String, bool) {
        match scene {
            LlmScene::Main => session_override
                .map(str::trim)
                .filter(|model| !model.is_empty())
                .map(|model| (model.to_string(), true))
                .unwrap_or_else(|| (self.config.llm.default_model.clone(), false)),
            LlmScene::Compaction => {
                let model = self.config.context.compaction_model.trim();
                if model.is_empty() {
                    (self.config.llm.default_model.clone(), false)
                } else {
                    (model.to_string(), false)
                }
            }
            LlmScene::Vision => self
                .config
                .llm
                .vision_model
                .as_deref()
                .map(str::trim)
                .filter(|model| !model.is_empty())
                .map(|model| (model.to_string(), true))
                .unwrap_or_else(|| {
                    let main_model = session_override
                        .map(str::trim)
                        .filter(|model| !model.is_empty())
                        .unwrap_or(&self.config.llm.default_model);
                    (main_model.to_string(), false)
                }),
            LlmScene::Title => self
                .config
                .llm
                .title_model
                .as_deref()
                .map(str::trim)
                .filter(|model| !model.is_empty())
                .map(|model| (model.to_string(), true))
                .unwrap_or_else(|| {
                    let fallback = self.config.context.compaction_model.trim();
                    if fallback.is_empty() {
                        (self.config.llm.default_model.clone(), false)
                    } else {
                        (fallback.to_string(), false)
                    }
                }),
        }
    }

    fn lookup_entry(&self, model_id: &str, explicit: bool) -> Result<ModelEntry, AppError> {
        if explicit {
            self.catalog.lookup_explicit(model_id)
        } else {
            if self.prefers_legacy_single_provider_mode() {
                return Ok(legacy_entry_for(
                    model_id,
                    &self.config.llm,
                    &self.config.context,
                ));
            }
            self.catalog
                .lookup_or_legacy(model_id, &self.config.llm, &self.config.context)
        }
    }

    fn prefers_legacy_single_provider_mode(&self) -> bool {
        if self.config.llm.provider != "openai-responses" {
            return true;
        }

        match self.config.llm.api_base.as_deref() {
            Some(base) => {
                let default_base = infer_default_base_url(Some(self.config.llm.provider.as_str()));
                default_base.as_deref() != Some(base)
            }
            None => false,
        }
    }

    fn guard_scene(&self, scene: LlmScene, entry: &ModelEntry) -> Result<(), AppError> {
        if matches!(scene, LlmScene::Vision) && !entry.capabilities.vision {
            let suggested = self
                .catalog
                .entries()
                .into_iter()
                .find(|candidate| candidate.capabilities.vision)
                .map(|candidate| candidate.id)
                .unwrap_or_else(|| self.config.llm.default_model.clone());
            return Err(AppError::Llm(format!(
                "provider/model 不支持 vision，建议改用 `{}`。",
                suggested
            )));
        }
        Ok(())
    }

    fn credential_for(&self, entry: &ModelEntry) -> Result<Credential, AppError> {
        self.auth
            .get(&entry.provider, self.config.llm.api_key_env.as_deref())
    }

    fn effective_base_url(&self, entry: &ModelEntry) -> Option<String> {
        entry
            .base_url
            .clone()
            .or_else(|| self.config.llm.api_base.clone())
            .or_else(|| infer_default_base_url(Some(entry.provider.as_str())))
            .or_else(|| infer_default_base_url(Some(entry.api.as_str())))
    }

    fn build_provider_config(&self, entry: &ModelEntry, credential: &Credential) -> LlmConfig {
        let mut cfg = self.config.llm.clone();
        cfg.provider = entry.api.clone();
        cfg.api_base = self.effective_base_url(entry);
        cfg.api_key_env = Some(credential.env_name.clone());
        cfg.default_model = entry.id.clone();
        if let Some(format) = entry.thinking_format.clone() {
            cfg.thinking.format = Some(format);
        }
        cfg
    }

    fn resolved_thinking_format(&self, entry: &ModelEntry) -> ThinkingFormat {
        match entry.thinking_format.as_deref() {
            Some(format) => {
                ThinkingFormat::parse_or_auto(Some(format)).resolve_for_model(&entry.id)
            }
            None => match self.config.llm.thinking.format.as_deref() {
                Some(format) => {
                    ThinkingFormat::parse_or_auto(Some(format)).resolve_for_model(&entry.id)
                }
                None => thinking_format_for_model(&entry.id),
            },
        }
    }
}

impl LlmResolver for DefaultLlmResolver {
    fn resolve(
        &self,
        scene: LlmScene,
        session_override: Option<&str>,
    ) -> Result<ResolvedCall, AppError> {
        let (model_id, explicit) = self.select_model_id(scene, session_override);
        let entry = self.lookup_entry(&model_id, explicit)?;
        self.guard_scene(scene, &entry)?;
        let credential = self.credential_for(&entry)?;
        let provider_cfg = self.build_provider_config(&entry, &credential);
        let provider_impl = resolve_llm(&provider_cfg)?;
        Ok(ResolvedCall {
            provider_impl,
            model: entry.id.clone(),
            api: entry.api.clone(),
            provider: entry.provider.clone(),
            base_url: provider_cfg.api_base.clone(),
            key_source: credential.env_name,
            thinking_format: self.resolved_thinking_format(&entry),
            capabilities: entry.capabilities.clone(),
        })
    }
}
