use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::LazyLock;

use futures_util::StreamExt;
use regex::Regex;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use reqwest::Url;

use crate::infra::AppError;

static SECRET_PREFIX_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(^|[^a-z0-9])(bearer\s+|sk-[a-z0-9_-]+|ghp_[a-z0-9_]+)")
        .expect("valid secret prefix regex")
});

#[derive(Debug, Clone)]
pub(crate) struct ValidatedHttpUrl {
    pub url: Url,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HttpSchemePolicy {
    AllowHttp,
    UpgradeToHttps,
    RequireHttps,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct UrlValidationOptions {
    pub max_url_length: usize,
    pub error_prefix: &'static str,
    pub scheme_policy: HttpSchemePolicy,
}

#[derive(Debug)]
pub(crate) struct BodyReadResult {
    pub bytes: Vec<u8>,
    pub truncated: bool,
    pub timed_out: bool,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct PublicIpDnsResolver;

pub(crate) fn validate_http_url(
    raw: &str,
    options: UrlValidationOptions,
) -> Result<ValidatedHttpUrl, AppError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(AppError::Tool(format!(
            "{}: 缺少必填字段 `url`",
            options.error_prefix
        )));
    }
    if trimmed.chars().count() > options.max_url_length {
        return Err(AppError::Tool(format!(
            "{}: `url` 过长（>{} 字符）",
            options.error_prefix, options.max_url_length
        )));
    }

    let mut url = Url::parse(trimmed)
        .map_err(|err| AppError::Tool(format!("{}: `url` 非法: {err}", options.error_prefix)))?;
    match url.scheme() {
        "http" | "https" => {}
        other => {
            return Err(AppError::Tool(format!(
                "{}: `url` 协议非法 `{other}`，仅允许 http/https",
                options.error_prefix
            )));
        }
    }
    if matches!(options.scheme_policy, HttpSchemePolicy::RequireHttps) && url.scheme() != "https" {
        return Err(AppError::Tool(format!(
            "{}: `url` 必须使用 https",
            options.error_prefix
        )));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(AppError::Tool(format!(
            "{}: URL with credentials rejected",
            options.error_prefix
        )));
    }

    let host = url
        .host_str()
        .map(|value| value.trim_end_matches('.').to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::Tool(format!("{}: `url` 缺少合法 host", options.error_prefix)))?;

    let ip_candidate = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = ip_candidate.parse::<IpAddr>() {
        if is_private_or_local_ip(ip) {
            return Err(AppError::Tool(format!(
                "{}: private or loopback IP rejected",
                options.error_prefix
            )));
        }
        return Err(AppError::Tool(format!(
            "{}: IP literal host rejected",
            options.error_prefix
        )));
    }
    if is_reserved_local_hostname(&host) {
        return Err(AppError::Tool(format!(
            "{}: local hostname rejected",
            options.error_prefix
        )));
    }
    if !host.contains('.') {
        return Err(AppError::Tool(format!(
            "{}: single-segment host rejected",
            options.error_prefix
        )));
    }

    let mut warnings = Vec::new();
    if contains_secret_prefix(trimmed) {
        warnings.push("secret_prefix_in_url".to_string());
    }

    url.set_host(Some(&host)).map_err(|err| {
        AppError::Tool(format!(
            "{}: `url` host 规范化失败: {err}",
            options.error_prefix
        ))
    })?;
    if matches!(options.scheme_policy, HttpSchemePolicy::UpgradeToHttps) && url.scheme() == "http" {
        url.set_scheme("https").map_err(|()| {
            AppError::Tool(format!("{}: `url` 无法升级到 https", options.error_prefix))
        })?;
    }

    Ok(ValidatedHttpUrl { url, warnings })
}

pub(crate) fn is_permitted_redirect(from: &Url, to: &Url) -> bool {
    if from.scheme() != to.scheme() {
        return false;
    }
    if from.port_or_known_default() != to.port_or_known_default() {
        return false;
    }

    let Some(from_host) = from.host_str().map(|value| value.to_ascii_lowercase()) else {
        return false;
    };
    let Some(to_host) = to.host_str().map(|value| value.to_ascii_lowercase()) else {
        return false;
    };

    from_host == to_host || is_www_variant(&from_host, &to_host)
}

impl Resolve for PublicIpDnsResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let addrs = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(|err| {
                    Box::<dyn std::error::Error + Send + Sync>::from(std::io::Error::other(
                        format!("dns lookup failed for `{host}`: {err}"),
                    ))
                })?
                .collect::<Vec<SocketAddr>>();
            if addrs.is_empty() {
                return Err(Box::<dyn std::error::Error + Send + Sync>::from(
                    std::io::Error::other(format!("dns lookup returned no addresses for `{host}`")),
                ));
            }
            if let Some(private_ip) = addrs
                .iter()
                .map(SocketAddr::ip)
                .find(|ip| is_private_or_local_ip(*ip))
            {
                return Err(Box::<dyn std::error::Error + Send + Sync>::from(
                    std::io::Error::other(format!(
                        "dns lookup for `{host}` resolved to disallowed IP `{private_ip}`"
                    )),
                ));
            }
            let addrs: Addrs = Box::new(addrs.into_iter());
            Ok(addrs)
        })
    }
}

pub(crate) async fn read_body_limited(
    response: reqwest::Response,
    max_bytes: usize,
    error_prefix: &str,
) -> Result<BodyReadResult, AppError> {
    let mut bytes = Vec::new();
    let mut truncated = false;
    let mut timed_out = false;
    let mut stream = response.bytes_stream();

    while let Some(item) = stream.next().await {
        match item {
            Ok(chunk) => {
                let remaining = max_bytes.saturating_sub(bytes.len());
                if chunk.len() > remaining {
                    bytes.extend_from_slice(&chunk[..remaining]);
                    truncated = true;
                    break;
                }
                bytes.extend_from_slice(&chunk);
            }
            Err(err) if err.is_timeout() => {
                timed_out = true;
                break;
            }
            Err(err) => {
                return Err(AppError::Tool(format!(
                    "{error_prefix}: 响应体读取失败: {err}"
                )));
            }
        }
    }

    Ok(BodyReadResult {
        bytes,
        truncated,
        timed_out,
    })
}

fn contains_secret_prefix(raw: &str) -> bool {
    SECRET_PREFIX_RE.is_match(raw)
}

fn is_private_or_local_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(addr) => is_private_v4(addr),
        IpAddr::V6(addr) => {
            addr.is_loopback()
                || addr.is_unique_local()
                || addr.is_unicast_link_local()
                || addr.is_unspecified()
        }
    }
}

fn is_private_v4(addr: Ipv4Addr) -> bool {
    addr.is_loopback() || addr.is_private() || addr.is_link_local() || addr.is_unspecified()
}

fn is_reserved_local_hostname(host: &str) -> bool {
    host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.ends_with(".internal")
        || host.ends_with(".localdomain")
        || host.ends_with(".home.arpa")
}

fn is_www_variant(left: &str, right: &str) -> bool {
    left.strip_prefix("www.") == Some(right) || right.strip_prefix("www.") == Some(left)
}

#[cfg(test)]
mod tests {
    use super::{
        validate_http_url, HttpSchemePolicy, PublicIpDnsResolver, Resolve, UrlValidationOptions,
    };
    use std::str::FromStr;

    const MAX_URL_LEN: usize = 2048;

    fn options(scheme_policy: HttpSchemePolicy) -> UrlValidationOptions {
        UrlValidationOptions {
            max_url_length: MAX_URL_LEN,
            error_prefix: "net_guard_test",
            scheme_policy,
        }
    }

    #[test]
    fn validate_http_url_rejects_localhost_style_hostnames() {
        for url in [
            "https://localhost/path",
            "https://svc.internal/path",
            "https://printer.local/path",
            "https://router.home.arpa/path",
        ] {
            let err = validate_http_url(url, options(HttpSchemePolicy::RequireHttps))
                .expect_err("reserved local hostname should be rejected");
            assert!(err.to_string().contains("local hostname rejected"), "{err}");
        }
    }

    #[test]
    fn validate_http_url_rejects_link_local_and_private_ip_literals() {
        for url in [
            "https://169.254.10.20/path",
            "https://192.168.1.2/path",
            "https://[fe80::1]/path",
        ] {
            let err = validate_http_url(url, options(HttpSchemePolicy::RequireHttps))
                .expect_err("local ip literal should be rejected");
            assert!(
                err.to_string().contains("private or loopback IP rejected"),
                "{err}"
            );
        }
    }

    #[test]
    fn validate_http_url_rejects_http_when_https_required() {
        let err = validate_http_url(
            "http://example.com/path",
            options(HttpSchemePolicy::RequireHttps),
        )
        .expect_err("http should be rejected");
        assert!(err.to_string().contains("必须使用 https"));
    }

    #[test]
    fn validate_http_url_can_upgrade_http_when_policy_allows() {
        let validated = validate_http_url(
            "http://example.com/path?q=1",
            options(HttpSchemePolicy::UpgradeToHttps),
        )
        .expect("http should upgrade to https");
        assert_eq!(validated.url.as_str(), "https://example.com/path?q=1");
    }

    #[tokio::test]
    async fn public_ip_dns_resolver_rejects_localhost_resolution() {
        let resolver = PublicIpDnsResolver;
        let err = resolver
            .resolve(reqwest::dns::Name::from_str("localhost").expect("valid name"))
            .await
            .err()
            .expect("localhost should resolve to disallowed local IP");
        assert!(err.to_string().contains("disallowed IP"));
    }
}
