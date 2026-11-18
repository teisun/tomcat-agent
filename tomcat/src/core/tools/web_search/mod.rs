pub(crate) mod backend;
mod cache;
pub mod declarations;
pub mod openai_server;
pub mod plugin_backend;
pub mod types;

#[cfg(test)]
mod tests;

use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use parking_lot::RwLock;

use crate::core::llm::auth::{
    credential_generation, key_present_for_env, process_env_value, read_env_value,
};
use crate::core::llm::catalog::infer_default_base_url;
use crate::core::llm::{ModelCatalog, SharedModelCatalog};
use crate::infra::http_client::{
    build_outbound_client, default_connect_timeout_for, OutboundClientErrorKind,
    OutboundClientOptions,
};
use crate::infra::{AppConfig, AppError, ToolsWebSearchConfig};

use self::backend::{
    discover_hosted_candidate, BackendFailure, BackendSearchResponse, HostedCandidateModel,
    WebSearchBackend,
};
use self::cache::{CacheKey, WebSearchCache};
use self::declarations::{effective_order, BackendDeclaration, OPENAI};
use self::plugin_backend::{PluginSearchInvoker, PluginWebSearchBackend};
use self::types::{normalize_hits, Stats, WebSearchArgs, WebSearchOutput, WebSearchRequest};

pub type SearchOrder = Arc<RwLock<Option<Vec<String>>>>;

#[derive(Clone)]
pub struct WebSearchRuntime {
    client: reqwest::Client,
    config: ToolsWebSearchConfig,
    model_catalog: SharedModelCatalog,
    cache: WebSearchCache,
    plugin_invoker: OnceLock<Arc<dyn PluginSearchInvoker>>,
    order: SearchOrder,
}

enum SearchTarget<'a> {
    Hosted(&'a HostedCandidateModel),
    Plugin(&'a BackendDeclaration, String),
}

fn target_for<'a>(
    id: &str,
    hosted: Option<&'a HostedCandidateModel>,
    declarations: &'a [BackendDeclaration],
    catalog: &ModelCatalog,
    saved_keys: bool,
) -> Result<SearchTarget<'a>, BackendFailure> {
    if id == OPENAI {
        let candidate = hosted.ok_or_else(|| BackendFailure::Incompatible {
            detail: "No hosted web_search model configured. Choose a model that supports hosted search in Settings → Models.".into(),
        })?;
        if candidate.api != "openai-responses" {
            return Err(BackendFailure::Incompatible {
                detail: format!(
                    "Hosted web_search model {} is misconfigured or unavailable",
                    candidate.id
                ),
            });
        }
        return if key_present_for_env(&candidate.key_env) {
            Ok(SearchTarget::Hosted(candidate))
        } else {
            Err(BackendFailure::MissingKey {
                env_name: candidate.key_env.clone(),
            })
        };
    }
    let declaration = declarations
        .iter()
        .find(|entry| entry.id == id)
        .ok_or_else(|| BackendFailure::Incompatible {
            detail: format!("backend_unknown:{id}. Check Settings → Tools → Web Search."),
        })?;
    let slot = declaration
        .key_slot(catalog)
        .ok_or_else(|| BackendFailure::Incompatible {
            detail: format!(
                "Search backend {id} requires model {}",
                declaration.model.as_deref().unwrap_or_default()
            ),
        })?;
    let present = if saved_keys || declaration.model.is_some() {
        key_present_for_env(&slot)
    } else {
        process_env_value(&slot).is_some()
    };
    if !present {
        return Err(BackendFailure::MissingKey { env_name: slot });
    }
    Ok(SearchTarget::Plugin(declaration, slot))
}

impl WebSearchRuntime {
    pub fn new(
        config: &AppConfig,
        model_catalog: impl Into<SharedModelCatalog>,
    ) -> Result<Self, AppError> {
        let web_cfg = config.tools.web_search.clone();
        let client = build_web_search_http_client(config, &web_cfg)?;
        Ok(Self {
            cache: WebSearchCache::new(&web_cfg),
            client,
            order: Arc::new(RwLock::new(web_cfg.configured_backends())),
            config: web_cfg,
            model_catalog: model_catalog.into(),
            plugin_invoker: OnceLock::new(),
        })
    }

    pub(crate) fn with_search_order(mut self, order: Option<SearchOrder>) -> Self {
        if let Some(order) = order {
            self.order = order;
        }
        self
    }

    pub fn set_plugin_invoker(&self, invoker: Arc<dyn PluginSearchInvoker>) {
        let _ = self.plugin_invoker.set(invoker);
    }

    pub async fn search(
        &self,
        args: WebSearchArgs,
        session_id: &str,
    ) -> Result<WebSearchOutput, AppError> {
        let request = WebSearchRequest::from_tool_args(args, &self.config)?;
        let catalog = self.model_catalog.snapshot();
        let invoker = self.plugin_invoker.get();
        let declarations = invoker
            .map(|invoker| invoker.backend_declarations())
            .unwrap_or_default();
        let order = effective_order(self.order.read().as_deref(), &declarations);
        let hosted = order
            .iter()
            .any(|id| id == OPENAI)
            .then(|| discover_hosted_candidate(&catalog))
            .flatten();
        let saved_keys = invoker.is_some_and(|invoker| invoker.winner_can_use_saved_keys());
        let targets: Vec<_> = order
            .iter()
            .map(|id| target_for(id, hosted.as_ref(), &declarations, &catalog, saved_keys))
            .collect();
        let available = order
            .iter()
            .zip(&targets)
            .filter_map(|(id, target)| {
                let slot = match target.as_ref().ok()? {
                    SearchTarget::Hosted(model) => &model.key_env,
                    SearchTarget::Plugin(_, slot) => slot,
                };
                Some((id.clone(), slot.clone(), credential_generation(slot)))
            })
            .collect();
        let cache_key = CacheKey::from_request(&request, available);
        if let Some(mut output) = self.cache.get(&cache_key) {
            output.stats.cached = true;
            return Ok(output);
        }
        let mut failures = Vec::new();
        let start = Instant::now();
        let timeout = Duration::from_millis(self.config.timeout_ms);
        for (id, target) in order.iter().zip(targets) {
            let result = match target {
                Err(failure) => Err(failure),
                Ok(target) => tokio::time::timeout(timeout, async {
                    match target {
                        SearchTarget::Hosted(candidate) => {
                            self.execute_hosted(&request, candidate).await
                        }
                        SearchTarget::Plugin(declaration, _) => {
                            let invoker = invoker.expect("declarations require a plugin invoker");
                            PluginWebSearchBackend::new(invoker.clone(), id.as_str(), session_id)
                                .with_model(declaration.model.clone())
                                .search(&request)
                                .await
                        }
                    }
                })
                .await
                .unwrap_or(Err(BackendFailure::Timeout)),
            };
            match result {
                Ok(response) => {
                    let mut output = build_output(&request, id, response, start);
                    for warning in output.warnings.drain(..) {
                        if !failures.contains(&warning) {
                            failures.push(warning);
                        }
                    }
                    output.warnings = failures;
                    if !output.hits.is_empty() || !output.truncated {
                        self.cache.insert(cache_key, output.clone());
                    }
                    return Ok(output);
                }
                Err(failure) => {
                    let error = failure.to_tool_error(id);
                    if order.len() == 1 {
                        return Err(error);
                    }
                    failures.push(format!("backend_unavailable:{id} ({error})"));
                }
            }
        }
        Err(AppError::Tool(crate::infra::i18n::tr(
            "search.allUnavailableDetails",
            &[("query", &request.query), ("details", &failures.join("; "))],
        )))
    }

    #[cfg(test)]
    pub(crate) fn insert_cached_output_for_test(
        &self,
        args: WebSearchArgs,
        value: WebSearchOutput,
    ) -> Result<(), AppError> {
        let request = WebSearchRequest::from_tool_args(args, &self.config)?;
        self.cache
            .insert(CacheKey::from_request(&request, Vec::new()), value);
        Ok(())
    }

    async fn execute_hosted(
        &self,
        request: &WebSearchRequest,
        candidate: &HostedCandidateModel,
    ) -> Result<BackendSearchResponse, BackendFailure> {
        let credential =
            read_env_value(&candidate.key_env).ok_or_else(|| BackendFailure::MissingKey {
                env_name: candidate.key_env.clone(),
            })?;
        let base_url = candidate
            .base_url
            .clone()
            .or_else(|| infer_default_base_url(Some(candidate.provider.as_str())))
            .unwrap_or_else(|| "https://api.openai.com".into());
        match openai_server::search_openai_hosted(
            &self.client,
            &base_url,
            &credential,
            &candidate.model_name,
            request,
        )
        .await
        {
            Err(BackendFailure::InvalidRequest { detail, .. })
                if looks_like_unsupported_hosted_tool(&detail) =>
            {
                Err(BackendFailure::Incompatible {
                    detail: format!(
                        "Hosted web_search model {} is misconfigured or unavailable",
                        candidate.id
                    ),
                })
            }
            result => result,
        }
    }
}

fn build_output(
    request: &WebSearchRequest,
    id: &str,
    response: BackendSearchResponse,
    start: Instant,
) -> WebSearchOutput {
    let normalized = normalize_hits(
        response.raw_hits,
        request.count,
        &request.allowed_domains,
        &request.blocked_domains,
    );
    let mut warnings = response.warnings;
    for warning in normalized.warnings {
        if !warnings.contains(&warning) {
            warnings.push(warning);
        }
    }
    WebSearchOutput {
        query: request.query.clone(),
        hits: normalized.hits,
        backend: id.to_owned(),
        stats: Stats {
            elapsed_ms: start.elapsed().as_millis() as u64,
            cached: false,
            total_before_filter: normalized.total_before_filter,
        },
        truncated: normalized.truncated,
        warnings,
    }
}

fn build_web_search_http_client(
    config: &AppConfig,
    web_cfg: &ToolsWebSearchConfig,
) -> Result<reqwest::Client, AppError> {
    let mut options = OutboundClientOptions::new(config.llm.proxy.as_deref());
    let timeout = Duration::from_millis(web_cfg.timeout_ms);
    options.timeout = Some(timeout);
    options.connect_timeout = Some(default_connect_timeout_for(timeout));
    build_outbound_client(
        options,
        OutboundClientErrorKind::Llm,
        &crate::infra::i18n::tr("search.client", &[]),
    )
}

fn looks_like_unsupported_hosted_tool(detail: &str) -> bool {
    let lower = detail.to_ascii_lowercase();
    lower.contains("web_search")
        && (lower.contains("unsupported")
            || lower.contains("not support")
            || lower.contains("unknown tool")
            || lower.contains("invalid tool"))
}
