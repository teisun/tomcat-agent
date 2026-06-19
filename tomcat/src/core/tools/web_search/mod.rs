pub(crate) mod backend;
mod cache;
pub mod openai_server;
pub mod plugin_backend;
pub mod types;

#[cfg(test)]
mod tests;

use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use crate::core::llm::catalog::infer_default_base_url;
use crate::core::llm::{env_name_for_provider, AuthStore, ModelCatalog};
use crate::infra::http_client::{
    build_outbound_client, default_connect_timeout_for, OutboundClientErrorKind,
    OutboundClientOptions,
};
use crate::infra::{AppConfig, AppError, ToolsWebSearchConfig};

use self::backend::{
    discover_hosted_candidate, pick_backend, BackendFailure, BackendName, BackendPlan,
    BackendSearchResponse, HostedCandidateModel, WebSearchBackend,
};
use self::cache::{CacheKey, WebSearchCache};
use self::plugin_backend::{PluginSearchInvoker, PluginWebSearchBackend};
use self::types::{normalize_hits, Stats, WebSearchArgs, WebSearchOutput, WebSearchRequest};

#[derive(Clone)]
pub struct WebSearchRuntime {
    client: reqwest::Client,
    config: ToolsWebSearchConfig,
    model_catalog: Arc<ModelCatalog>,
    auth: AuthStore,
    cache: WebSearchCache,
    plugin_invoker: OnceLock<Arc<dyn PluginSearchInvoker>>,
}

impl WebSearchRuntime {
    pub fn new(config: &AppConfig, model_catalog: Arc<ModelCatalog>) -> Result<Self, AppError> {
        let web_cfg = config.tools.web_search.clone();
        let client = build_web_search_http_client(config, &web_cfg)?;
        Ok(Self {
            cache: WebSearchCache::new(&web_cfg),
            client,
            config: web_cfg,
            model_catalog,
            auth: AuthStore,
            plugin_invoker: OnceLock::new(),
        })
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
        let cache_key = CacheKey::from_request(&request);
        if let Some(mut cached) = self.cache.get(&cache_key) {
            cached.stats.cached = true;
            return Ok(cached);
        }

        let hosted_candidate = discover_hosted_candidate(&self.model_catalog);
        let plan = pick_backend(request.backend.clone(), hosted_candidate)?;
        let output = match plan {
            BackendPlan::Auto {
                hosted_candidate,
                plugin_slot,
            } => {
                self.execute_auto(&request, hosted_candidate, plugin_slot, session_id)
                    .await?
            }
            BackendPlan::ExplicitPlugin(backend) => {
                self.execute_explicit_plugin(&request, &backend, session_id)
                    .await?
            }
            BackendPlan::HostedOnly(candidate) => {
                self.execute_explicit_openai(&request, &candidate).await?
            }
        };

        if should_cache(&output) {
            self.cache.insert(cache_key, output.clone());
        }
        Ok(output)
    }

    #[cfg(test)]
    pub(crate) fn insert_cached_output_for_test(
        &self,
        args: WebSearchArgs,
        value: WebSearchOutput,
    ) -> Result<(), AppError> {
        let request = WebSearchRequest::from_tool_args(args, &self.config)?;
        self.cache.insert(CacheKey::from_request(&request), value);
        Ok(())
    }

    async fn execute_auto(
        &self,
        request: &WebSearchRequest,
        hosted_candidate: Option<HostedCandidateModel>,
        plugin_slot: bool,
        session_id: &str,
    ) -> Result<WebSearchOutput, AppError> {
        let mut warnings = Vec::new();
        let hosted_present = hosted_candidate.is_some();
        let first_fallback = if plugin_slot { Some("auto") } else { None };

        if let Some(candidate) = hosted_candidate {
            match self.execute_openai_hosted(request, &candidate).await {
                Ok(output) => return Ok(output),
                Err(BackendFailure::Incompatible { .. }) => warnings.push(format!(
                    "hosted_candidate_unavailable, fallback={}",
                    first_fallback.unwrap_or("auto")
                )),
                Err(failure) if failure.is_retryable_unavailable() => {
                    warnings.push(format!(
                        "openai_unavailable, fallback={}",
                        first_fallback.unwrap_or("auto")
                    ));
                    extend_unique(
                        &mut warnings,
                        failure
                            .auto_fallback_warnings(BackendName::Openai.as_str(), first_fallback),
                    );
                }
                Err(failure) => {
                    return Err(log_hard_failure(
                        BackendName::Openai.as_str(),
                        failure.to_tool_error(BackendName::Openai.as_str()),
                    ))
                }
            }
        }

        if plugin_slot {
            if self.plugin_invoker.get().is_none() {
                return Err(plugin_invoker_missing_error());
            }
            match self
                .execute_plugin_backend(request, "auto", session_id)
                .await
            {
                Ok(mut output) => {
                    prepend_unique(&mut output.warnings, warnings);
                    return Ok(output);
                }
                Err(failure) if failure.is_retryable_unavailable() => {
                    extend_unique(&mut warnings, failure.auto_fallback_warnings("auto", None));
                }
                Err(failure) => {
                    return Err(log_hard_failure("auto", failure.to_tool_error("auto")))
                }
            }
        }

        if !hosted_present && !plugin_slot {
            return Err(AppError::Tool(
                "no web_search backend configured".to_string(),
            ));
        }
        warnings.push("all_backends_unavailable".to_string());
        Err(all_backends_unavailable_error(&request.query, &warnings))
    }

    async fn execute_explicit_plugin(
        &self,
        request: &WebSearchRequest,
        backend: &str,
        session_id: &str,
    ) -> Result<WebSearchOutput, AppError> {
        if self.plugin_invoker.get().is_none() {
            return Err(plugin_invoker_missing_error());
        }
        match self
            .execute_plugin_backend(request, backend, session_id)
            .await
        {
            Ok(output) => Ok(output),
            Err(failure) => Err(log_hard_failure(backend, failure.to_tool_error(backend))),
        }
    }

    async fn execute_explicit_openai(
        &self,
        request: &WebSearchRequest,
        candidate: &HostedCandidateModel,
    ) -> Result<WebSearchOutput, AppError> {
        match self.execute_openai_hosted(request, candidate).await {
            Ok(output) => Ok(output),
            Err(BackendFailure::Incompatible { .. }) => Err(log_hard_failure(
                BackendName::Openai.as_str(),
                AppError::Tool(format!(
                    "hosted web_search model {} is misconfigured or unavailable",
                    candidate.id
                )),
            )),
            Err(failure) => Err(log_hard_failure(
                BackendName::Openai.as_str(),
                failure.to_tool_error(BackendName::Openai.as_str()),
            )),
        }
    }

    async fn execute_openai_hosted(
        &self,
        request: &WebSearchRequest,
        candidate: &HostedCandidateModel,
    ) -> Result<WebSearchOutput, BackendFailure> {
        if candidate.api.trim() != "openai-responses" {
            return Err(BackendFailure::Incompatible {
                detail: format!(
                    "hosted web_search model {} is misconfigured or unavailable",
                    candidate.id
                ),
            });
        }

        let credential = self
            .auth
            .get_for_provider(&candidate.provider, None, None)
            .map_err(|_| BackendFailure::MissingKey {
                env_name: env_name_for_provider(&candidate.provider),
            })?;
        let base_url = candidate
            .base_url
            .clone()
            .or_else(|| infer_default_base_url(Some(candidate.provider.as_str())))
            .unwrap_or_else(|| "https://api.openai.com".to_string());

        let start = Instant::now();
        let raw = match openai_server::search_openai_hosted(
            &self.client,
            &base_url,
            &credential.value,
            &candidate.id,
            request,
        )
        .await
        {
            Err(BackendFailure::InvalidRequest { detail, .. })
                if looks_like_unsupported_hosted_tool(&detail) =>
            {
                return Err(BackendFailure::Incompatible {
                    detail: format!(
                        "hosted web_search model {} is misconfigured or unavailable",
                        candidate.id
                    ),
                });
            }
            other => other?,
        };
        Ok(self.build_output(request, BackendName::Openai.as_str(), raw, start))
    }

    async fn execute_plugin_backend(
        &self,
        request: &WebSearchRequest,
        backend: &str,
        session_id: &str,
    ) -> Result<WebSearchOutput, BackendFailure> {
        let invoker =
            self.plugin_invoker
                .get()
                .cloned()
                .ok_or_else(|| BackendFailure::Incompatible {
                    detail: "web_search plugin backend invoker not configured".to_string(),
                })?;
        let start = Instant::now();
        let raw = PluginWebSearchBackend::new(invoker, backend, session_id)
            .search(request)
            .await?;
        Ok(self.build_output(request, backend, raw, start))
    }

    fn build_output(
        &self,
        request: &WebSearchRequest,
        default_backend: &str,
        backend_response: BackendSearchResponse,
        start: Instant,
    ) -> WebSearchOutput {
        let BackendSearchResponse {
            backend_label,
            raw_hits,
            warnings,
        } = backend_response;
        let mut normalized = normalize_hits(
            raw_hits,
            request.count,
            &request.allowed_domains,
            &request.blocked_domains,
        );
        prepend_unique(&mut normalized.warnings, warnings);
        WebSearchOutput {
            query: request.query.clone(),
            hits: normalized.hits,
            backend: backend_label.unwrap_or_else(|| default_backend.to_string()),
            stats: Stats {
                elapsed_ms: elapsed_ms(start),
                cached: false,
                total_before_filter: normalized.total_before_filter,
            },
            truncated: normalized.truncated,
            warnings: normalized.warnings,
        }
    }
}

fn plugin_invoker_missing_error() -> AppError {
    AppError::Tool("web_search plugin backend invoker not configured".to_string())
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
        "创建 web_search HTTP 客户端失败",
    )
}

fn elapsed_ms(start: Instant) -> u64 {
    start.elapsed().as_millis() as u64
}

fn should_cache(output: &WebSearchOutput) -> bool {
    !(output
        .warnings
        .iter()
        .any(|warning| warning == "all_backends_unavailable")
        || (output.hits.is_empty() && output.truncated))
}

fn log_hard_failure(backend: &str, error: AppError) -> AppError {
    tracing::error!(backend, detail = %error, "web_search backend hard failure");
    error
}

fn all_backends_unavailable_error(query: &str, warnings: &[String]) -> AppError {
    if warnings.is_empty() {
        return AppError::Tool(format!("web_search 查询 `{query}` 所有后端均不可用。"));
    }
    AppError::Tool(format!(
        "web_search 查询 `{query}` 所有后端均不可用：{}",
        warnings.join("; ")
    ))
}

fn prepend_unique(target: &mut Vec<String>, prefix: Vec<String>) {
    let mut merged = prefix;
    extend_unique(&mut merged, target.clone());
    *target = merged;
}

fn extend_unique(target: &mut Vec<String>, extra: Vec<String>) {
    for warning in extra {
        if !target.iter().any(|existing| existing == &warning) {
            target.push(warning);
        }
    }
}

fn looks_like_unsupported_hosted_tool(detail: &str) -> bool {
    let lower = detail.to_ascii_lowercase();
    lower.contains("web_search")
        && (lower.contains("unsupported")
            || lower.contains("not support")
            || lower.contains("unknown tool")
            || lower.contains("invalid tool"))
}
