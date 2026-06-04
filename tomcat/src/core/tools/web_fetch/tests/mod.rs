mod cache_test;
mod fetcher_test;
mod markdownify_test;
mod persist_test;
mod redirect_test;
mod validate_test;

use std::path::PathBuf;

use crate::AppConfig;

use super::types::{WebFetchArgs, WebFetchFormat, WebFetchOutput};
use super::validate::ValidatedUrl;
use super::WebFetchRuntime;

pub(super) fn test_config() -> AppConfig {
    let mut cfg = AppConfig::default();
    cfg.tools.web_fetch.fetch_timeout_ms = 800;
    cfg.tools.web_fetch.max_http_content_bytes = 1024;
    cfg.tools.web_fetch.max_markdown_chars = 80;
    cfg.tools.web_fetch.markdown_head_chars = 24;
    cfg.tools.web_fetch.cache_ttl_secs = 1;
    cfg.tools.web_fetch.cache_capacity_bytes = 1024;
    cfg
}

pub(super) fn build_runtime(config: AppConfig, persist_dir: PathBuf) -> WebFetchRuntime {
    WebFetchRuntime::new(&config, persist_dir).expect("build runtime")
}

pub(super) fn validated_url(raw: &str) -> ValidatedUrl {
    ValidatedUrl {
        url: reqwest::Url::parse(raw).expect("valid url"),
        warnings: Vec::new(),
    }
}

pub(super) fn args(url: &str) -> WebFetchArgs {
    WebFetchArgs {
        url: url.to_string(),
        prompt: None,
        format: None,
    }
}

pub(super) fn text_output(url: &str, format: WebFetchFormat, result: &str) -> WebFetchOutput {
    WebFetchOutput::new(
        url.to_string(),
        200,
        "OK".to_string(),
        "text/html; charset=utf-8".to_string(),
        result.len() as u64,
        result.to_string(),
        result.chars().count() as u64,
        12,
        None,
        None,
        false,
        vec![format!("format={}", format.as_str())],
    )
}
