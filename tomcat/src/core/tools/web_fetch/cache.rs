use std::time::Duration;

use moka::sync::Cache;

use crate::infra::ToolsWebFetchConfig;

use super::types::WebFetchOutput;

/// `web_fetch` 缓存键：同一个 canonical URL + format 才算命中。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct CacheKey {
    url: String,
    format: String,
}

impl CacheKey {
    pub(crate) fn new(url: &str, format: &str) -> Self {
        Self {
            url: url.to_string(),
            format: format.to_string(),
        }
    }
}

/// `web_fetch` runtime 的进程内缓存。
#[derive(Clone)]
pub(crate) struct WebFetchCache {
    inner: Cache<CacheKey, WebFetchOutput>,
}

impl WebFetchCache {
    pub(crate) fn new(cfg: &ToolsWebFetchConfig) -> Self {
        Self {
            inner: Cache::builder()
                .time_to_live(Duration::from_secs(cfg.cache_ttl_secs))
                .max_capacity(cfg.cache_capacity_bytes)
                .weigher(|_key, value| cache_weight(value))
                .build(),
        }
    }

    pub(crate) fn get(&self, key: &CacheKey) -> Option<WebFetchOutput> {
        self.inner.get(key)
    }

    pub(crate) fn insert(&self, key: CacheKey, value: WebFetchOutput) {
        self.inner.insert(key, value);
    }

    #[cfg(test)]
    pub(crate) fn run_pending_tasks(&self) {
        self.inner.run_pending_tasks();
    }

    #[cfg(test)]
    pub(crate) fn entry_count(&self) -> u64 {
        self.inner.entry_count()
    }
}

fn cache_weight(value: &WebFetchOutput) -> u32 {
    let meta = value
        .persisted_output_path
        .as_ref()
        .map(|path| path.len())
        .unwrap_or(0)
        + value.content_type.len()
        + value.url.len()
        + value.code_text.len()
        + 256;
    let total = value.result.len().saturating_add(meta);
    total.clamp(1, u32::MAX as usize) as u32
}
