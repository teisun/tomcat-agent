use parking_lot::{Mutex, RwLock};
use std::collections::HashMap;
use tracing::warn;

use crate::infra::config::{AppConfig, ContextConfig, LlmRuntimeConfig, ThinkingConfig};
use crate::infra::error::AppError;

use super::auth::{credential_generation, AuthStore, Credential};
use std::sync::Arc;

use super::catalog::{
    infer_default_base_url, validate_model_limit_values, Capabilities, ModelCatalog, ModelEntry,
    SharedModelCatalog,
};
use super::provider::LlmProvider;
use super::registry::{build_provider_with_route, ProviderRoute};
use super::thinking_policy::UNKNOWN_ANTHROPIC_MAX_OUTPUT_TOKENS;
use super::thinking_policy::{resolve_anthropic_request, ThinkingFormat, ThinkingLevel};
use super::{ChatMessage, ChatMessageContent, ChatMessageContentPart, ChatRequest};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmScene {
    Main,
    Compaction,
    Vision,
    Title,
}

#[derive(Clone)]
pub struct ResolvedCall {
    pub provider_impl: Arc<dyn LlmProvider>,
    pub model: String,
    pub catalog_id: String,
    pub api: String,
    pub provider: String,
    pub base_url: Option<String>,
    pub key_source: String,
    pub thinking_format: ThinkingFormat,
    pub thinking_config: ThinkingConfig,
    pub capabilities: Capabilities,
    pub limits: EffectiveModelLimits,
    #[allow(dead_code)]
    sealed: Sealed,
}

impl std::fmt::Debug for ResolvedCall {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedCall")
            .field("model", &self.model)
            .field("catalog_id", &self.catalog_id)
            .field("api", &self.api)
            .field("provider", &self.provider)
            .field("base_url", &self.base_url)
            .field("key_source", &self.key_source)
            .field("thinking_format", &self.thinking_format)
            .field("capabilities", &self.capabilities)
            .field("limits", &self.limits)
            .finish()
    }
}

#[derive(Debug, Clone, Copy)]
struct Sealed;

/// Where a resolved model limit came from. Values are emitted in diagnostics so
/// a fallback can never masquerade as a model declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitSource {
    ModelCatalog,
    LegacyFallback,
    UnknownAnthropicFallback,
    UnknownOpenAiLocalReserve,
    ExplicitRequest,
}

impl LimitSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ModelCatalog => "model_catalog",
            Self::LegacyFallback => "legacy_fallback",
            Self::UnknownAnthropicFallback => "unknown_anthropic_fallback",
            Self::UnknownOpenAiLocalReserve => "unknown_openai_local_reserve",
            Self::ExplicitRequest => "explicit_request",
        }
    }
}

/// The single runtime interpretation of a model's context and output limits.
///
/// Model capability stays in `ModelEntry`; this structure combines it with
/// local fallback/reserve policy and is what context management consumes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveModelLimits {
    pub context_window: usize,
    pub model_max_output_tokens: Option<usize>,
    pub output_reserve_tokens: usize,
    pub input_budget_tokens: usize,
    pub context_source: LimitSource,
    pub output_source: LimitSource,
}

impl EffectiveModelLimits {
    pub fn resolve(entry: &ModelEntry, config: &ContextConfig) -> Result<Self, AppError> {
        Self::resolve_with_context_window(entry, config, None)
    }

    pub fn resolve_with_context_window(
        entry: &ModelEntry,
        config: &ContextConfig,
        selected_context_window: Option<u32>,
    ) -> Result<Self, AppError> {
        let selected_context_window = selected_context_window.filter(|value| {
            !entry.context_window_options.is_empty() && entry.context_window_options.contains(value)
        });
        let (context_window, context_source) = match selected_context_window {
            Some(value) => (value as usize, LimitSource::ModelCatalog),
            None => match entry.context_window {
                Some(value) => (value as usize, LimitSource::ModelCatalog),
                None => (config.context_window_fallback, LimitSource::LegacyFallback),
            },
        };
        let (model_max_output_tokens, output_source) = match entry.max_output_tokens {
            Some(value) => (Some(value as usize), LimitSource::ModelCatalog),
            None if entry.api == "anthropic-messages" => (
                Some(UNKNOWN_ANTHROPIC_MAX_OUTPUT_TOKENS as usize),
                LimitSource::UnknownAnthropicFallback,
            ),
            None => (None, LimitSource::UnknownOpenAiLocalReserve),
        };
        validate_model_limit_values(&entry.id, context_window, model_max_output_tokens)?;

        let local_unknown_reserve = (context_window / 4).min(128_000);
        let model_or_local_reserve = model_max_output_tokens.unwrap_or(local_unknown_reserve);
        let output_reserve_tokens = config
            .output_reserve_tokens
            .unwrap_or(0)
            .max(model_or_local_reserve);
        if output_reserve_tokens >= context_window {
            return Err(AppError::Config(tr(
                "model.reserveInvalid",
                &[
                    ("id", &entry.id),
                    ("reserve", &output_reserve_tokens.to_string()),
                    ("window", &context_window.to_string()),
                ],
            )));
        }

        Ok(Self {
            context_window,
            model_max_output_tokens,
            output_reserve_tokens,
            input_budget_tokens: context_window - output_reserve_tokens,
            context_source,
            output_source,
        })
    }

    pub fn wire_output_limit_for_request(
        &self,
        api: &str,
        request_max_tokens: Option<u32>,
    ) -> (Option<u32>, LimitSource) {
        let known_capacity = self.model_max_output_tokens.map(|value| value as u32);
        match api {
            "anthropic-messages" => (
                Some(
                    request_max_tokens
                        .unwrap_or_else(|| {
                            known_capacity.unwrap_or(UNKNOWN_ANTHROPIC_MAX_OUTPUT_TOKENS)
                        })
                        .min(known_capacity.unwrap_or(UNKNOWN_ANTHROPIC_MAX_OUTPUT_TOKENS)),
                ),
                if request_max_tokens.is_some() {
                    LimitSource::ExplicitRequest
                } else {
                    self.output_source
                },
            ),
            _ => match request_max_tokens {
                Some(requested) => (
                    Some(
                        known_capacity
                            .map(|capacity| requested.min(capacity))
                            .unwrap_or(requested),
                    ),
                    LimitSource::ExplicitRequest,
                ),
                None => (None, self.output_source),
            },
        }
    }
}

impl ResolvedCall {
    pub fn wire_model(&self) -> &str {
        &self.model
    }

    pub fn catalog_id(&self) -> &str {
        &self.catalog_id
    }

    /// Resolve the provider-wire output cap for one request and keep it
    /// separate from `ChatRequest::max_tokens`, which is the caller's optional
    /// product-level request. The result must travel with the request instead
    /// of being copied into a connection-scoped provider instance.
    pub fn apply_resolved_output_limit(&self, request: &mut ChatRequest) -> LimitSource {
        let (limit, source) = self.output_limit_for_request(request.max_tokens);
        request.resolved_output_limit = limit;
        source
    }

    pub fn output_limit_for_request(
        &self,
        request_max_tokens: Option<u32>,
    ) -> (Option<u32>, LimitSource) {
        self.limits
            .wire_output_limit_for_request(&self.api, request_max_tokens)
    }

    /// The classic Anthropic thinking budget that the configured policy will
    /// send for this request. Adaptive thinking intentionally returns `None`:
    /// its `effort` is not a numeric provider budget.
    pub fn thinking_budget_for_request(
        &self,
        thinking_level: Option<ThinkingLevel>,
        resolved_output_limit: Option<u32>,
    ) -> Option<u32> {
        if self.api != "anthropic-messages" {
            return None;
        }
        let mut config = self.thinking_config.clone();
        if let Some(level) = thinking_level {
            config.level = level.as_str().to_string();
        }
        let request = resolve_anthropic_request(
            &config,
            self.thinking_format,
            resolved_output_limit,
            resolved_output_limit,
        );
        request
            .thinking
            .as_ref()
            .and_then(|thinking| thinking.get("budget_tokens"))
            .and_then(serde_json::Value::as_u64)
            .map(|budget| budget as u32)
    }

    /// 仅测试与 provider 适配层可用；生产路径必须经 [`LlmResolver`]。
    ///
    /// 该工厂绕过密封构造，允许在单测里装配 `ResolvedCall`，**不得**在生产派发路径使用。
    /// 它默认关闭 vision/files 等能力；需要模拟 catalog 模型的测试应使用
    /// [`Self::from_provider_and_entry_unchecked`]，或显式构造所需能力。
    #[doc(hidden)]
    pub fn from_parts_unchecked(
        provider_impl: Arc<dyn LlmProvider>,
        catalog_id: impl Into<String>,
        wire_model: impl Into<String>,
    ) -> Self {
        Self {
            provider_impl,
            model: wire_model.into(),
            catalog_id: catalog_id.into(),
            api: String::new(),
            provider: String::new(),
            base_url: None,
            key_source: String::new(),
            thinking_format: ThinkingFormat::default(),
            thinking_config: ThinkingConfig::default(),
            capabilities: Capabilities::default(),
            limits: EffectiveModelLimits {
                context_window: 400_000,
                model_max_output_tokens: None,
                output_reserve_tokens: 100_000,
                input_budget_tokens: 300_000,
                context_source: LimitSource::LegacyFallback,
                output_source: LimitSource::UnknownOpenAiLocalReserve,
            },
            sealed: Sealed,
        }
    }

    /// 从 catalog entry 组装测试用调用绑定，保留模型声明的能力与 wire model。
    ///
    /// 仅测试与 provider 适配层可用；生产路径必须经 [`LlmResolver`]。
    #[doc(hidden)]
    pub fn from_provider_and_entry_unchecked(
        provider_impl: Arc<dyn LlmProvider>,
        entry: &ModelEntry,
    ) -> Self {
        Self {
            provider_impl,
            model: entry.request_model_name().to_string(),
            catalog_id: entry.id.clone(),
            api: entry.api.clone(),
            provider: entry.provider.clone(),
            base_url: entry.base_url.clone(),
            key_source: entry.api_key_env.clone().unwrap_or_default(),
            thinking_format: ThinkingFormat::parse_or_auto(entry.thinking_format.as_deref())
                .resolve_for_api(entry.api.as_str()),
            thinking_config: ThinkingConfig::default(),
            capabilities: entry.capabilities.clone(),
            limits: EffectiveModelLimits::resolve(entry, &ContextConfig::default())
                .expect("test ModelEntry must contain valid limits"),
            sealed: Sealed,
        }
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
struct RouteKey {
    api: String,
    base_url: Option<String>,
    key_source: String,
    key_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ModelKey {
    route: RouteKey,
    catalog_id: String,
    catalog_generation: u64,
}

pub fn capability_requirements_for_messages(messages: &[ChatMessage]) -> CapabilityRequirements {
    let mut requirements = CapabilityRequirements::default();
    for message in messages {
        if let Some(ChatMessageContent::Parts(parts)) = &message.content {
            for part in parts {
                match part {
                    ChatMessageContentPart::InputReference { .. } => {}
                    ChatMessageContentPart::InputImage { .. }
                    | ChatMessageContentPart::InputImageRef { .. } => {
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

use crate::infra::i18n::tr;

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
    Err(AppError::Llm(tr(
        "model.capabilitiesMissing",
        &[("missing", &missing), ("suggested", &suggested)],
    )))
}

/// Metadata is sufficient for history and context budgets; it never contains credentials.
#[derive(Debug, Clone)]
pub struct ResolvedModelMetadata {
    pub catalog_id: String,
    pub capabilities: Capabilities,
    pub limits: EffectiveModelLimits,
}

pub trait LlmResolver: Send + Sync {
    fn resolve(
        &self,
        scene: LlmScene,
        session_override: Option<&str>,
    ) -> Result<ResolvedCall, AppError>;

    /// Test/custom resolvers may reuse their supplied call; the production resolver overrides
    /// this with catalog-only resolution, without constructing a provider or reading credentials.
    fn resolve_metadata(
        &self,
        scene: LlmScene,
        session_override: Option<&str>,
    ) -> Result<ResolvedModelMetadata, AppError> {
        self.resolve(scene, session_override)
            .map(|call| ResolvedModelMetadata {
                catalog_id: call.catalog_id,
                capabilities: call.capabilities,
                limits: call.limits,
            })
    }

    fn selected_model_id(&self, session_override: Option<&str>) -> String {
        self.resolve_metadata(LlmScene::Main, session_override)
            .map(|model| model.catalog_id)
            .unwrap_or_else(|_| session_override.unwrap_or_default().to_string())
    }

    /// Whether the main model can be resolved locally. Compaction falls back to the main model,
    /// so this is the only precondition. Stateless/custom resolvers supply their own runtime.
    fn model_ready(&self, _session_override: Option<&str>) -> bool {
        true
    }

    /// Model deletion updates only the selection-related configuration in a live process.
    /// Stateless/test resolvers intentionally retain their existing behavior.
    fn reload_config(&self, _config: AppConfig) {}
}

pub struct DefaultLlmResolver {
    config: RwLock<AppConfig>,
    catalog: SharedModelCatalog,
    auth: AuthStore,
    routes: Mutex<HashMap<RouteKey, Arc<ProviderRoute>>>,
    providers: Mutex<HashMap<ModelKey, Arc<dyn LlmProvider>>>,
    model_prefs: Arc<crate::core::session::ModelPrefsStore>,
}

impl DefaultLlmResolver {
    pub fn new(
        config: AppConfig,
        catalog: impl Into<SharedModelCatalog>,
        model_prefs: Arc<crate::core::session::ModelPrefsStore>,
    ) -> Self {
        Self {
            config: RwLock::new(config),
            catalog: catalog.into(),
            auth: AuthStore,
            routes: Mutex::new(HashMap::new()),
            providers: Mutex::new(HashMap::new()),
            model_prefs,
        }
    }

    fn config_snapshot(&self) -> AppConfig {
        self.config.read().clone()
    }

    fn select_model_id(&self, scene: LlmScene, session_override: Option<&str>) -> String {
        let config = self.config_snapshot();
        let session_model = session_override
            .map(str::trim)
            .filter(|model| !model.is_empty());
        let default_model = config.llm.default_model.trim();
        match scene {
            LlmScene::Main => session_model
                .map(str::to_string)
                .unwrap_or_else(|| default_model.to_string()),
            LlmScene::Compaction => self
                .compaction_candidates(session_override)
                .into_iter()
                .next()
                .unwrap_or_default(),
            LlmScene::Vision => config
                .llm
                .vision_model
                .as_deref()
                .map(str::trim)
                .filter(|model| !model.is_empty())
                .map(str::to_string)
                .or_else(|| session_model.map(str::to_string))
                .unwrap_or_else(|| default_model.to_string()),
            LlmScene::Title => config
                .llm
                .title_model
                .as_deref()
                .map(str::trim)
                .filter(|model| !model.is_empty())
                .map(str::to_string)
                .or_else(|| {
                    let compaction_model = config.context.compaction_model.trim();
                    (!compaction_model.is_empty()).then(|| compaction_model.to_string())
                })
                .or_else(|| (!default_model.is_empty()).then(|| default_model.to_string()))
                .or_else(|| session_model.map(str::to_string))
                .unwrap_or_default(),
        }
    }

    fn lookup_entry(&self, model_id: &str) -> Result<ModelEntry, AppError> {
        self.catalog
            .lookup_explicit(model_id)
            .map_err(|error| AppError::ModelUnavailable {
                model_id: model_id.to_string(),
                message: error.to_string(),
            })
    }

    fn guard_scene(&self, scene: LlmScene, entry: &ModelEntry) -> Result<(), AppError> {
        let config = self.config_snapshot();
        self.catalog.with_catalog(|catalog| {
            validate_capabilities(
                catalog,
                &config.llm.default_model,
                scene,
                &entry.id,
                &entry.capabilities,
                &[],
            )
        })
    }

    fn credential_for(
        &self,
        entry: &ModelEntry,
        compatible_fallback_env: Option<&str>,
    ) -> Result<Credential, AppError> {
        self.auth.get(entry, compatible_fallback_env)
    }

    fn compatible_fallback_env(&self, scene: LlmScene, entry: &ModelEntry) -> Option<String> {
        match scene {
            LlmScene::Compaction => self.compaction_fallback_env(entry),
            _ => self.test_fallback_env(),
        }
    }

    fn compaction_fallback_env(&self, entry: &ModelEntry) -> Option<String> {
        let config = self.config_snapshot();
        let default_model = config.llm.default_model.trim();
        if default_model.is_empty() || entry.id == default_model {
            return None;
        }
        let default_entry = self.catalog.lookup(default_model)?;
        if default_entry.provider == entry.provider {
            default_entry.api_key_env
        } else {
            None
        }
    }

    fn effective_base_url(&self, entry: &ModelEntry) -> Option<String> {
        #[cfg(test)]
        let config = self.config_snapshot();
        #[cfg(test)]
        if let Some(base_url) = config.llm.api_base.clone() {
            return Some(base_url);
        }
        entry
            .base_url
            .clone()
            .or_else(|| infer_default_base_url(Some(entry.provider.as_str())))
            .or_else(|| infer_default_base_url(Some(entry.api.as_str())))
    }

    #[cfg(test)]
    fn test_fallback_env(&self) -> Option<String> {
        self.config_snapshot().llm.api_key_env
    }

    #[cfg(not(test))]
    fn test_fallback_env(&self) -> Option<String> {
        None
    }

    fn resolved_thinking_format(&self, entry: &ModelEntry) -> ThinkingFormat {
        let config = self.config_snapshot();
        ThinkingFormat::parse_or_auto(
            entry
                .thinking_format
                .as_deref()
                .or(config.llm.thinking.format.as_deref()),
        )
        .resolve_for_api(entry.api.as_str())
    }

    fn runtime(&self) -> LlmRuntimeConfig {
        self.config_snapshot().llm.runtime()
    }

    fn route_key(&self, entry: &ModelEntry, credential: &Credential) -> RouteKey {
        RouteKey {
            api: entry.api.clone(),
            base_url: self.effective_base_url(entry),
            key_source: credential.env_name.clone(),
            key_generation: credential_generation(&credential.env_name),
        }
    }

    fn resolve_route(
        &self,
        key: &RouteKey,
        runtime: &LlmRuntimeConfig,
    ) -> Result<Arc<ProviderRoute>, AppError> {
        let mut routes = self.routes.lock();
        if let Some(route) = routes.get(key) {
            return Ok(route.clone());
        }
        let route = Arc::new(ProviderRoute::new(runtime)?);
        routes.insert(key.clone(), route.clone());
        Ok(route)
    }

    #[cfg(test)]
    pub(crate) fn route_for_model(&self, model: &str) -> Arc<ProviderRoute> {
        let entry = self.lookup_entry(model).unwrap();
        let credential = self
            .credential_for(&entry, self.test_fallback_env().as_deref())
            .unwrap();
        self.routes
            .lock()
            .get(&self.route_key(&entry, &credential))
            .unwrap()
            .clone()
    }

    #[cfg(test)]
    pub(crate) fn cached_provider_count(&self) -> usize {
        self.providers.lock().len()
    }

    fn resolve_cached_provider(
        &self,
        entry: &ModelEntry,
        credential: &Credential,
    ) -> Result<Arc<dyn LlmProvider>, AppError> {
        let route_key = self.route_key(entry, credential);
        let cache_key = ModelKey {
            route: route_key.clone(),
            catalog_id: entry.id.clone(),
            catalog_generation: self.catalog.generation(),
        };
        let mut cache = self.providers.lock();
        if let Some(existing) = cache.get(&cache_key) {
            return Ok(existing.clone());
        }
        let runtime = self.runtime();
        let route = self.resolve_route(&route_key, &runtime)?;
        let provider = build_provider_with_route(entry, &runtime, credential, &route)?;
        cache.insert(cache_key, provider.clone());
        Ok(provider)
    }

    fn entry_metadata(
        &self,
        scene: LlmScene,
        entry: &ModelEntry,
    ) -> Result<ResolvedModelMetadata, AppError> {
        self.guard_scene(scene, entry)?;
        let selected_context_window = if entry.context_window_options.is_empty() {
            None
        } else {
            self.model_prefs
                .context_window_for(&entry.id)
                .filter(|value| entry.context_window_options.contains(value))
        };
        let limits = EffectiveModelLimits::resolve_with_context_window(
            entry,
            &self.config_snapshot().context,
            selected_context_window,
        )?;
        Ok(ResolvedModelMetadata {
            catalog_id: entry.id.clone(),
            capabilities: entry.capabilities.clone(),
            limits,
        })
    }

    fn resolve_model_call(
        &self,
        scene: LlmScene,
        model_id: &str,
    ) -> Result<ResolvedCall, AppError> {
        let entry = self.lookup_entry(model_id)?;
        let metadata = self.entry_metadata(scene, &entry)?;
        let compatible_fallback_env = self.compatible_fallback_env(scene, &entry);
        let credential = self.credential_for(&entry, compatible_fallback_env.as_deref())?;
        let provider_impl = self.resolve_cached_provider(&entry, &credential)?;
        let base_url = self.effective_base_url(&entry);
        let config = self.config_snapshot();
        Ok(ResolvedCall {
            provider_impl,
            model: entry.request_model_name().to_string(),
            catalog_id: entry.id.clone(),
            api: entry.api.clone(),
            provider: entry.provider.clone(),
            base_url,
            key_source: credential.env_name,
            thinking_format: self.resolved_thinking_format(&entry),
            thinking_config: config.llm.thinking.clone(),
            capabilities: entry.capabilities.clone(),
            limits: metadata.limits,
            sealed: Sealed,
        })
    }

    /// Compaction prefers its configured model, then the default, and finally the session's main
    /// model, so a usable main model always keeps compaction usable.
    fn compaction_candidates(&self, session_override: Option<&str>) -> Vec<String> {
        let config = self.config_snapshot();
        let main_model = self.select_model_id(LlmScene::Main, session_override);
        let mut candidates: Vec<String> = Vec::new();
        for id in [
            config.context.compaction_model.trim(),
            config.llm.default_model.trim(),
            main_model.as_str(),
        ] {
            if !id.is_empty() && !candidates.iter().any(|candidate| candidate == id) {
                candidates.push(id.to_string());
            }
        }
        candidates
    }

    fn resolve_compaction_call(
        &self,
        session_override: Option<&str>,
    ) -> Result<ResolvedCall, AppError> {
        let candidates = self.compaction_candidates(session_override);
        let mut failures: Vec<(&str, AppError)> = Vec::new();
        for candidate in &candidates {
            if let Some((failed, error)) = failures.last() {
                warn!(
                    compaction_model = *failed,
                    fallback_model = candidate.as_str(),
                    error = %error,
                    "compaction model unavailable, falling back"
                );
            }
            match self.resolve_model_call(LlmScene::Compaction, candidate) {
                Ok(resolved) => return Ok(resolved),
                Err(error) => failures.push((candidate.as_str(), error)),
            }
        }
        let mut failures = failures.into_iter();
        let Some((selected, original_err)) = failures.next() else {
            return self.resolve_model_call(LlmScene::Compaction, "");
        };
        match failures.last() {
            None => Err(original_err),
            Some((fallback, fallback_err)) => Err(AppError::Config(tr(
                "model.compactionFallbackFailed",
                &[
                    ("selected", selected),
                    ("fallback", fallback),
                    ("original", &original_err.to_string()),
                    ("detail", &fallback_err.to_string()),
                ],
            ))),
        }
    }
}

impl LlmResolver for DefaultLlmResolver {
    fn resolve_metadata(
        &self,
        scene: LlmScene,
        session_override: Option<&str>,
    ) -> Result<ResolvedModelMetadata, AppError> {
        let id = self.select_model_id(scene, session_override);
        self.entry_metadata(scene, &self.lookup_entry(&id)?)
    }

    fn selected_model_id(&self, session_override: Option<&str>) -> String {
        self.select_model_id(LlmScene::Main, session_override)
    }

    fn model_ready(&self, session_override: Option<&str>) -> bool {
        let id = self.select_model_id(LlmScene::Main, session_override);
        self.lookup_entry(&id)
            .and_then(|entry| {
                self.entry_metadata(LlmScene::Main, &entry)?;
                let fallback_env = self.compatible_fallback_env(LlmScene::Main, &entry);
                self.credential_for(&entry, fallback_env.as_deref())
            })
            .is_ok()
    }

    fn resolve(
        &self,
        scene: LlmScene,
        session_override: Option<&str>,
    ) -> Result<ResolvedCall, AppError> {
        match scene {
            LlmScene::Compaction => self.resolve_compaction_call(session_override),
            _ => self.resolve_model_call(scene, &self.select_model_id(scene, session_override)),
        }
    }

    fn reload_config(&self, config: AppConfig) {
        *self.config.write() = config;
    }
}
