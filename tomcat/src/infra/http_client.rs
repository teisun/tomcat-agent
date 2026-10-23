use std::sync::Arc;
use std::time::Duration;

use reqwest::redirect::Policy;

use super::error::AppError;
use super::net_guard::PublicIpDnsResolver;

pub(crate) const DEFAULT_OUTBOUND_CONNECT_TIMEOUT_MS: u64 = 8_000;
const MIN_TIMEOUT_BUDGET_BUFFER_MS: u64 = 100;
const MAX_TIMEOUT_BUDGET_BUFFER_MS: u64 = 1_000;

pub(crate) enum OutboundClientErrorKind {
    Llm,
    Tool,
}

pub(crate) struct OutboundClientOptions<'a> {
    pub explicit_proxy_url: Option<&'a str>,
    pub bypass_system_proxy: bool,
    pub timeout: Option<Duration>,
    pub read_timeout: Option<Duration>,
    pub connect_timeout: Option<Duration>,
    pub redirect_policy: Option<Policy>,
    pub use_public_ip_dns_resolver: bool,
}

impl<'a> OutboundClientOptions<'a> {
    pub(crate) fn new(explicit_proxy_url: Option<&'a str>) -> Self {
        Self {
            explicit_proxy_url,
            bypass_system_proxy: false,
            timeout: None,
            read_timeout: None,
            connect_timeout: None,
            redirect_policy: None,
            use_public_ip_dns_resolver: false,
        }
    }
}

pub(crate) fn default_connect_timeout_for(total_timeout: Duration) -> Duration {
    total_timeout.min(Duration::from_millis(DEFAULT_OUTBOUND_CONNECT_TIMEOUT_MS))
}

pub(crate) fn has_proxy_env() -> bool {
    [
        "HTTPS_PROXY",
        "HTTP_PROXY",
        "ALL_PROXY",
        "https_proxy",
        "http_proxy",
        "all_proxy",
    ]
    .into_iter()
    .any(|key| {
        std::env::var(key)
            .ok()
            .is_some_and(|value| !value.trim().is_empty())
    })
}

pub(crate) fn clamp_timeout_within_budget(timeout_ms: u64, budget_ms: u64) -> Duration {
    if budget_ms == 0 {
        return Duration::from_millis(timeout_ms.max(1));
    }
    let buffer_ms =
        (budget_ms / 10).clamp(MIN_TIMEOUT_BUDGET_BUFFER_MS, MAX_TIMEOUT_BUDGET_BUFFER_MS);
    let capped_ms = timeout_ms.min(budget_ms.saturating_sub(buffer_ms).max(1));
    Duration::from_millis(capped_ms.max(1))
}

/// Only literal loopback addresses and localhost; never DNS suffixes or private LANs.
pub(crate) fn is_loopback_url(url: &reqwest::Url) -> bool {
    url.domain() == Some("localhost")
        || url.host_str().and_then(|host| {
            host.trim_matches(['[', ']']).parse::<std::net::IpAddr>().ok()
        }).is_some_and(|ip| ip.is_loopback())
}

pub(crate) fn build_outbound_client(
    options: OutboundClientOptions<'_>,
    error_kind: OutboundClientErrorKind,
    build_error_context: &str,
) -> Result<reqwest::Client, AppError> {
    // Shared service startup removes launcher proxy variables. Keep system
    // discovery enabled here; standalone CLI callers retain their environment.
    let mut builder = reqwest::Client::builder();
    if options.bypass_system_proxy {
        builder = builder.no_proxy();
    }

    if options.use_public_ip_dns_resolver {
        builder = builder.dns_resolver(Arc::new(PublicIpDnsResolver));
    }
    if let Some(redirect_policy) = options.redirect_policy {
        builder = builder.redirect(redirect_policy);
    }
    if let Some(timeout) = options.timeout {
        builder = builder.timeout(timeout);
    }
    if let Some(read_timeout) = options.read_timeout {
        builder = builder.read_timeout(read_timeout);
    }
    if let Some(connect_timeout) = options.connect_timeout {
        builder = builder.connect_timeout(connect_timeout);
    }
    if let Some(proxy_url) = options
        .explicit_proxy_url
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let proxy = reqwest::Proxy::all(proxy_url)
            .map(|proxy| proxy.no_proxy(reqwest::NoProxy::from_env()))
            .map_err(|err| {
                AppError::Config(crate::infra::i18n::tr(
                    "config.proxyInvalid",
                    &[("detail", &err.without_url().to_string())],
                ))
            })?;
        builder = builder.proxy(proxy);
    }
    builder.build().map_err(|err| match error_kind {
        OutboundClientErrorKind::Llm => AppError::Llm(format!("{build_error_context}: {err}")),
        OutboundClientErrorKind::Tool => AppError::Tool(format!("{build_error_context}: {err}")),
    })
}
