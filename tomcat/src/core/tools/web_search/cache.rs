use std::time::Duration;

use moka::sync::Cache;

use crate::infra::ToolsWebSearchConfig;

use super::types::{WebSearchFreshness, WebSearchOutput, WebSearchRequest};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CacheKey {
    /// Effective order with each backend's precheck result: adding, removing or
    /// blocking a key changes the route, rotating its value does not.
    routes: Vec<(String, bool)>,
    query: String,
    count: usize,
    freshness: Option<String>,
    country: Option<String>,
    language: Option<String>,
    domain_filter: Vec<String>,
    allowed_domains: Vec<String>,
    blocked_domains: Vec<String>,
}

impl CacheKey {
    pub fn from_request(request: &WebSearchRequest, routes: Vec<(String, bool)>) -> Self {
        Self {
            routes,
            query: request.query.clone(),
            count: request.count,
            freshness: request
                .freshness
                .map(WebSearchFreshness::as_str)
                .map(str::to_string),
            country: request.country.clone(),
            language: request.language.clone(),
            domain_filter: request.domain_filter.clone(),
            allowed_domains: request.allowed_domains.clone(),
            blocked_domains: request.blocked_domains.clone(),
        }
    }
}

#[derive(Clone)]
pub struct WebSearchCache {
    inner: Cache<CacheKey, WebSearchOutput>,
}

impl WebSearchCache {
    pub fn new(cfg: &ToolsWebSearchConfig) -> Self {
        Self {
            inner: Cache::builder()
                .time_to_live(Duration::from_secs(cfg.cache_ttl_secs))
                .max_capacity(cfg.cache_capacity)
                .build(),
        }
    }

    pub fn get(&self, key: &CacheKey) -> Option<WebSearchOutput> {
        self.inner.get(key)
    }

    pub fn insert(&self, key: CacheKey, value: WebSearchOutput) {
        self.inner.insert(key, value);
    }
}
