mod backend;
mod brave;
mod cache;
pub mod openai_server;
pub mod serper;
pub mod tavily;
pub mod types;

#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::core::llm::catalog::infer_default_base_url;
use crate::core::llm::{env_name_for_provider, AuthStore, ModelCatalog};
use crate::infra::{AppConfig, AppError, ToolsWebSearchConfig};

use self::backend::{
    discover_hosted_candidate, pick_backend, BackendFailure, BackendName, BackendPlan,
    BackendSearchResponse, HostedCandidateModel, WebSearchBackend,
};
use self::brave::BraveBackend;
use self::cache::{CacheKey, WebSearchCache};
use self::serper::SerperBackend;
use self::tavily::TavilyBackend;
use self::types::{normalize_hits, Stats, WebSearchArgs, WebSearchOutput, WebSearchRequest};

#[derive(Clone)]
pub struct WebSearchRuntime {
    client: reqwest::Client,
    config: ToolsWebSearchConfig,
    model_catalog: Arc<ModelCatalog>,
    auth: AuthStore,
    llm_fallback_env: Option<String>,
    cache: WebSearchCache,
    tavily: TavilyBackend,
    brave: BraveBackend,
    serper: SerperBackend,
}

impl WebSearchRuntime {
    pub fn new(config: &AppConfig, model_catalog: Arc<ModelCatalog>) -> Result<Self, AppError> {
        let web_cfg = config.tools.web_search.clone();
        let client = build_web_search_http_client(config, &web_cfg)?;
        Ok(Self {
            tavily: TavilyBackend::new(client.clone(), web_cfg.tavily_base_url.clone()),
            brave: BraveBackend::new(client.clone(), web_cfg.brave_base_url.clone()),
            serper: SerperBackend::new(client.clone(), web_cfg.serper_base_url.clone()),
            cache: WebSearchCache::new(&web_cfg),
            client,
            config: web_cfg,
            model_catalog,
            auth: AuthStore,
            llm_fallback_env: config.llm.api_key_env.clone(),
        })
    }

    pub async fn search(&self, args: WebSearchArgs) -> Result<WebSearchOutput, AppError> {
        let request = WebSearchRequest::from_tool_args(args, &self.config)?;
        let cache_key = CacheKey::from_request(&request);
        if let Some(mut cached) = self.cache.get(&cache_key) {
            cached.stats.cached = true;
            return Ok(cached);
        }

        let hosted_candidate = discover_hosted_candidate(&self.model_catalog);
        let plan = pick_backend(request.backend, hosted_candidate)?;
        let output = match plan {
            BackendPlan::Auto {
                hosted_candidate,
                http_chain,
            } => {
                self.execute_auto(&request, hosted_candidate, &http_chain)
                    .await?
            }
            BackendPlan::ExplicitHttp(backend) => {
                self.execute_explicit_http(&request, backend).await?
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
        http_chain: &[BackendName],
    ) -> Result<WebSearchOutput, AppError> {
        let start = Instant::now();
        let mut warnings = Vec::new();
        let hosted_present = hosted_candidate.is_some();
        let mut last_backend = hosted_candidate
            .as_ref()
            .map(|_| BackendName::Openai)
            .or_else(|| http_chain.last().copied())
            .unwrap_or(BackendName::Tavily);
        let mut all_http_missing_keys = true;

        if let Some(candidate) = hosted_candidate {
            last_backend = BackendName::Openai;
            match self.execute_openai_hosted(request, &candidate).await {
                Ok(output) => return Ok(output),
                Err(BackendFailure::Incompatible { .. }) => warnings.push(format!(
                    "hosted_candidate_unavailable, fallback={}",
                    http_chain[0].as_str()
                )),
                Err(failure) if failure.is_retryable_unavailable() => {
                    warnings.push(format!(
                        "openai_unavailable, fallback={}",
                        http_chain[0].as_str()
                    ));
                    extend_unique(
                        &mut warnings,
                        failure.auto_fallback_warnings(
                            BackendName::Openai,
                            http_chain.first().copied(),
                        ),
                    );
                }
                Err(failure) => return Err(failure.to_tool_error(BackendName::Openai)),
            }
        }

        for (index, backend) in http_chain.iter().copied().enumerate() {
            last_backend = backend;
            match self.execute_http_backend(request, backend).await {
                Ok(mut output) => {
                    prepend_unique(&mut output.warnings, warnings);
                    return Ok(output);
                }
                Err(failure @ BackendFailure::MissingKey { .. }) => {
                    extend_unique(
                        &mut warnings,
                        failure.auto_fallback_warnings(backend, http_chain.get(index + 1).copied()),
                    );
                }
                Err(failure) if failure.is_retryable_unavailable() => {
                    all_http_missing_keys = false;
                    extend_unique(
                        &mut warnings,
                        failure.auto_fallback_warnings(backend, http_chain.get(index + 1).copied()),
                    );
                }
                Err(failure) => return Err(failure.to_tool_error(backend)),
            }
        }

        if !hosted_present && all_http_missing_keys {
            return Err(AppError::Tool(
                "no web_search backend configured".to_string(),
            ));
        }
        warnings.push("all_backends_unavailable".to_string());
        Ok(WebSearchOutput::degraded(
            request.query.clone(),
            last_backend.as_str(),
            elapsed_ms(start),
            warnings,
        ))
    }

    async fn execute_explicit_http(
        &self,
        request: &WebSearchRequest,
        backend: BackendName,
    ) -> Result<WebSearchOutput, AppError> {
        let start = Instant::now();
        match self.execute_http_backend(request, backend).await {
            Ok(output) => Ok(output),
            Err(failure) if failure.is_explicit_degraded() => Ok(WebSearchOutput::degraded(
                request.query.clone(),
                backend.as_str(),
                elapsed_ms(start),
                failure.explicit_degraded_warnings(backend),
            )),
            Err(failure) => Err(failure.to_tool_error(backend)),
        }
    }

    async fn execute_explicit_openai(
        &self,
        request: &WebSearchRequest,
        candidate: &HostedCandidateModel,
    ) -> Result<WebSearchOutput, AppError> {
        let start = Instant::now();
        match self.execute_openai_hosted(request, candidate).await {
            Ok(output) => Ok(output),
            Err(failure) if failure.is_explicit_degraded() => Ok(WebSearchOutput::degraded(
                request.query.clone(),
                BackendName::Openai.as_str(),
                elapsed_ms(start),
                failure.explicit_degraded_warnings(BackendName::Openai),
            )),
            Err(BackendFailure::Incompatible { .. }) => Err(AppError::Tool(format!(
                "hosted web_search model {} is misconfigured or unavailable",
                candidate.id
            ))),
            Err(failure) => Err(failure.to_tool_error(BackendName::Openai)),
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
            .get(&candidate.provider, self.llm_fallback_env.as_deref())
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
        Ok(self.build_output(request, BackendName::Openai, raw, start))
    }

    async fn execute_http_backend(
        &self,
        request: &WebSearchRequest,
        backend: BackendName,
    ) -> Result<WebSearchOutput, BackendFailure> {
        let start = Instant::now();
        let raw = self.http_backend(backend).search(request).await?;
        Ok(self.build_output(request, backend, raw, start))
    }

    fn build_output(
        &self,
        request: &WebSearchRequest,
        backend: BackendName,
        backend_response: BackendSearchResponse,
        start: Instant,
    ) -> WebSearchOutput {
        let mut normalized = normalize_hits(
            backend_response.raw_hits,
            request.count,
            &request.allowed_domains,
            &request.blocked_domains,
        );
        prepend_unique(&mut normalized.warnings, backend_response.warnings);
        WebSearchOutput {
            query: request.query.clone(),
            hits: normalized.hits,
            backend: backend.as_str().to_string(),
            stats: Stats {
                elapsed_ms: elapsed_ms(start),
                cached: false,
                total_before_filter: normalized.total_before_filter,
            },
            truncated: normalized.truncated,
            warnings: normalized.warnings,
        }
    }

    fn http_backend(&self, backend: BackendName) -> &dyn WebSearchBackend {
        match backend {
            BackendName::Tavily => &self.tavily,
            BackendName::Brave => &self.brave,
            BackendName::Serper => &self.serper,
            BackendName::Openai => unreachable!("openai hosted path does not use http_backend"),
        }
    }
}

fn build_web_search_http_client(
    config: &AppConfig,
    web_cfg: &ToolsWebSearchConfig,
) -> Result<reqwest::Client, AppError> {
    let mut builder = reqwest::Client::builder().timeout(Duration::from_millis(web_cfg.timeout_ms));
    if let Some(proxy_url) = config.llm.proxy.as_deref() {
        let proxy = reqwest::Proxy::all(proxy_url)
            .map_err(|e| AppError::Config(format!("代理 URL 无效 {}: {}", proxy_url, e)))?;
        builder = builder.proxy(proxy);
    }
    builder
        .build()
        .map_err(|e| AppError::Llm(format!("创建 web_search HTTP 客户端失败: {}", e)))
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
