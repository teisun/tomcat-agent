//! `web_fetch` runtime：负责 URL 校验、HTTP 抓取、正文/二进制分流与缓存。

mod cache;
mod fetcher;
mod markdownify;
mod persist;
mod redirect;
pub mod types;
mod validate;

#[cfg(test)]
mod tests;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use reqwest::redirect::Policy;

use crate::infra::http_client::{
    build_outbound_client, OutboundClientErrorKind, OutboundClientOptions,
};
use crate::infra::{AppConfig, AppError, ToolsWebFetchConfig};

use self::cache::{CacheKey, WebFetchCache};
use self::fetcher::fetch_url;
use self::types::{WebFetchArgs, WebFetchOutput, WebFetchRequest};

pub use self::types::{RedirectInfo, WebFetchFormat};

const PROMPT_IGNORED_MVP: &str = "prompt_ignored_mvp";

/// `web_fetch` 会话级 runtime：复用同一个 HTTP client、缓存和落盘目录。
#[derive(Clone)]
pub struct WebFetchRuntime {
    client: reqwest::Client,
    config: ToolsWebFetchConfig,
    persist_dir: PathBuf,
    cache: WebFetchCache,
}

impl WebFetchRuntime {
    /// 构造 `web_fetch` runtime。
    ///
    /// `persist_dir` 应指向当前 agent 的 `tool-results/` 目录。
    pub fn new(config: &AppConfig, persist_dir: PathBuf) -> Result<Self, AppError> {
        let web_cfg = config.tools.web_fetch.clone();
        let client = build_web_fetch_http_client(config, &web_cfg)?;
        Ok(Self {
            client,
            cache: WebFetchCache::new(&web_cfg),
            config: web_cfg,
            persist_dir,
        })
    }

    /// 执行一次 `web_fetch`。
    pub async fn fetch(&self, args: WebFetchArgs) -> Result<WebFetchOutput, AppError> {
        let request = WebFetchRequest::from_tool_args(args)?;
        let validated = validate::validate_input_url(&request.raw_url)?;
        let cache_key = CacheKey::new(validated.url.as_str(), request.format.as_str());
        if let Some(mut cached) = self.cache.get(&cache_key) {
            cached.cached = true;
            apply_prompt_warning(&mut cached, request.prompt.is_some());
            return Ok(cached);
        }

        let output = fetch_url(
            &self.client,
            &self.persist_dir,
            &self.config,
            &request,
            &validated,
        )
        .await?;

        let mut cached_output = output.clone();
        apply_prompt_warning(&mut cached_output, false);
        if should_cache(&cached_output) {
            self.cache.insert(cache_key, cached_output);
        }

        let mut response = output;
        apply_prompt_warning(&mut response, request.prompt.is_some());
        Ok(response)
    }

    #[cfg(test)]
    pub(crate) fn insert_cached_output_for_test(
        &self,
        url: &str,
        format: WebFetchFormat,
        value: WebFetchOutput,
    ) {
        self.cache
            .insert(CacheKey::new(url, format.as_str()), value);
    }
}

fn build_web_fetch_http_client(
    config: &AppConfig,
    web_cfg: &ToolsWebFetchConfig,
) -> Result<reqwest::Client, AppError> {
    let mut options = OutboundClientOptions::new(config.llm.proxy.as_deref());
    options.redirect_policy = Some(Policy::none());
    options.timeout = Some(Duration::from_millis(web_cfg.fetch_timeout_ms));
    build_outbound_client(
        options,
        OutboundClientErrorKind::Tool,
        "创建 web_fetch HTTP 客户端失败",
    )
}

pub(crate) fn elapsed_ms(start: Instant) -> u64 {
    start.elapsed().as_millis() as u64
}

fn should_cache(output: &WebFetchOutput) -> bool {
    !output.truncated
        && output.redirect.is_none()
        && !output.warnings.iter().any(|warning| {
            warning.starts_with("timeout")
                || warning.starts_with("rate_limited")
                || warning.starts_with("server_error")
        })
}

fn apply_prompt_warning(output: &mut WebFetchOutput, has_prompt: bool) {
    if has_prompt {
        if !output
            .warnings
            .iter()
            .any(|warning| warning == PROMPT_IGNORED_MVP)
        {
            output.warnings.push(PROMPT_IGNORED_MVP.to_string());
        }
        return;
    }

    output
        .warnings
        .retain(|warning| warning != PROMPT_IGNORED_MVP);
}
