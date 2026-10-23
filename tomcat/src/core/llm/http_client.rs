use std::time::Duration;

use crate::infra::config::LlmRuntimeConfig;
use crate::infra::error::AppError;
use crate::infra::http_client::{
    build_outbound_client, is_loopback_url, OutboundClientErrorKind, OutboundClientOptions,
};

pub(crate) fn build_http_client(
    cfg: &LlmRuntimeConfig,
    proxy_override: Option<&str>,
    base_url: Option<&str>,
    fallback_url: Option<&str>,
) -> Result<reqwest::Client, AppError> {
    let mut options = OutboundClientOptions::new(proxy_override.or(cfg.proxy.as_deref()));
    let loopback = |url: &str| reqwest::Url::parse(url).is_ok_and(|url| is_loopback_url(&url));
    // A route and its fallback share one client. Unknown or remote targets keep discovery.
    options.bypass_system_proxy = options.explicit_proxy_url.is_none_or(|s| s.trim().is_empty())
        && base_url.is_some_and(loopback)
        && fallback_url.is_none_or(loopback);
    if cfg.http_read_timeout_sec > 0 {
        options.read_timeout = Some(Duration::from_secs(cfg.http_read_timeout_sec));
    }
    build_outbound_client(
        options,
        OutboundClientErrorKind::Llm,
        &crate::infra::i18n::tr("llm.clientFailed", &[]),
    )
}
