pub(crate) mod backend;
mod cache;
pub mod declarations;
pub mod plugin_backend;
pub mod types;

#[cfg(test)]
mod tests;

use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use parking_lot::RwLock;

use crate::core::llm::auth::credential_generation;
use crate::core::llm::{ModelCatalog, SharedModelCatalog};
use crate::infra::{AppConfig, AppError, ToolsWebSearchConfig};

use self::backend::{BackendFailure, BackendSearchResponse, WebSearchBackend};
use self::cache::{CacheKey, WebSearchCache};
use self::declarations::{effective_order, BackendDeclaration};
use self::plugin_backend::{PluginSearchInvoker, PluginWebSearchBackend};
use self::types::{normalize_hits, Stats, WebSearchArgs, WebSearchOutput, WebSearchRequest};

pub type SearchOrder = Arc<RwLock<Option<Vec<String>>>>;

#[derive(Clone)]
pub struct WebSearchRuntime {
    config: ToolsWebSearchConfig,
    model_catalog: SharedModelCatalog,
    cache: WebSearchCache,
    plugin_invoker: OnceLock<Arc<dyn PluginSearchInvoker>>,
    order: SearchOrder,
}

fn target_for<'a>(
    id: &str,
    declarations: &'a [BackendDeclaration],
    catalog: &ModelCatalog,
    saved_keys: bool,
) -> Result<(&'a BackendDeclaration, String), BackendFailure> {
    let declaration = declarations
        .iter()
        .find(|entry| entry.id == id)
        .ok_or_else(|| BackendFailure::Incompatible {
            detail: format!("backend_unknown:{id}. Check Settings → Tools → Web Search."),
        })?;
    let (slot, source) = declaration.key_status(catalog, saved_keys).ok_or_else(|| {
        BackendFailure::Incompatible {
            detail: if declaration.uses_host_key() {
                format!(
                    "Search backend {id} has no eligible key for {}. Check Settings → Models.",
                    declaration.host.as_deref().unwrap_or_default()
                )
            } else {
                format!(
                    "Search backend {id} requires model {}",
                    declaration.model.as_deref().unwrap_or_default()
                )
            },
        }
    })?;
    if source.is_none() {
        return Err(BackendFailure::MissingKey { env_name: slot });
    }
    Ok((declaration, slot))
}

impl WebSearchRuntime {
    pub fn new(
        config: &AppConfig,
        model_catalog: impl Into<SharedModelCatalog>,
    ) -> Result<Self, AppError> {
        let web_cfg = config.tools.web_search.clone();
        Ok(Self {
            cache: WebSearchCache::new(&web_cfg),
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
        let (catalog, model_revision) = self.model_catalog.snapshot_with_generation();
        let invoker = self.plugin_invoker.get();
        let plugin = invoker
            .map(|invoker| invoker.search_catalog())
            .unwrap_or_default();
        let order = effective_order(self.order.read().as_deref(), &plugin.declarations);
        let targets: Vec<_> = order
            .iter()
            .map(|id| target_for(id, &plugin.declarations, &catalog, plugin.saved_keys))
            .collect();
        let routes = order
            .iter()
            .zip(&targets)
            .map(|(id, target)| {
                let credential = target
                    .as_ref()
                    .ok()
                    .map(|(_, slot)| (slot.clone(), credential_generation(slot)));
                (id.clone(), credential)
            })
            .collect();
        let cache_key = CacheKey::from_request(&request, (plugin.revision, model_revision), routes);
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
                Ok((declaration, _)) => tokio::time::timeout(timeout, async {
                    let invoker = invoker.expect("declarations require a plugin invoker");
                    PluginWebSearchBackend::new(
                        invoker.clone(),
                        id.as_str(),
                        session_id,
                        plugin.revision,
                    )
                    .with_model(declaration.model.clone())
                    .search(&request)
                    .await
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
            .insert(CacheKey::from_request(&request, (0, 0), Vec::new()), value);
        Ok(())
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
