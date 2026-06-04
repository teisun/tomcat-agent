use std::net::{IpAddr, Ipv4Addr};
use std::sync::LazyLock;

use regex::Regex;
use reqwest::Url;

use crate::infra::AppError;

use super::types::MAX_URL_LENGTH;

static SECRET_PREFIX_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(^|[^a-z0-9])(bearer\s+|sk-[a-z0-9_-]+|ghp_[a-z0-9_]+)")
        .expect("valid secret prefix regex")
});

/// 通过校验后的 URL。
#[derive(Debug, Clone)]
pub(crate) struct ValidatedUrl {
    pub url: Url,
    pub warnings: Vec<String>,
}

/// 校验模型直接传入的 `url`，并在首跳前把 `http` 升为 `https`。
pub(crate) fn validate_input_url(raw: &str) -> Result<ValidatedUrl, AppError> {
    validate_url(raw, true)
}

/// 校验重定向目标；redirect 场景不做 `http -> https` 自动升级。
pub(crate) fn validate_redirect_url(raw: &str) -> Result<ValidatedUrl, AppError> {
    validate_url(raw, false)
}

fn validate_url(raw: &str, upgrade_insecure_http: bool) -> Result<ValidatedUrl, AppError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(AppError::Tool("web_fetch: 缺少必填字段 `url`".to_string()));
    }
    if trimmed.chars().count() > MAX_URL_LENGTH {
        return Err(AppError::Tool(format!(
            "web_fetch: `url` 过长（>{MAX_URL_LENGTH} 字符）"
        )));
    }

    let mut url = Url::parse(trimmed)
        .map_err(|err| AppError::Tool(format!("web_fetch: `url` 非法: {err}")))?;
    match url.scheme() {
        "http" | "https" => {}
        other => {
            return Err(AppError::Tool(format!(
                "web_fetch: `url` 协议非法 `{other}`，仅允许 http/https"
            )));
        }
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(AppError::Tool(
            "web_fetch: URL with credentials rejected".to_string(),
        ));
    }

    let host = url
        .host_str()
        .map(|value| value.trim_end_matches('.').to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::Tool("web_fetch: `url` 缺少合法 host".to_string()))?;

    if !host.contains('.') {
        return Err(AppError::Tool(
            "web_fetch: single-segment host rejected".to_string(),
        ));
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_private_or_local_ip(ip) {
            return Err(AppError::Tool(
                "web_fetch: private or loopback IP rejected".to_string(),
            ));
        }
        return Err(AppError::Tool(
            "web_fetch: IP literal host rejected".to_string(),
        ));
    }

    let mut warnings = Vec::new();
    if contains_secret_prefix(trimmed) {
        warnings.push("secret_prefix_in_url".to_string());
    }

    url.set_host(Some(&host))
        .map_err(|err| AppError::Tool(format!("web_fetch: `url` host 规范化失败: {err}")))?;
    if upgrade_insecure_http && url.scheme() == "http" {
        url.set_scheme("https")
            .map_err(|()| AppError::Tool("web_fetch: `url` 无法升级到 https".to_string()))?;
    }

    Ok(ValidatedUrl { url, warnings })
}

fn contains_secret_prefix(raw: &str) -> bool {
    SECRET_PREFIX_RE.is_match(raw)
}

fn is_private_or_local_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(addr) => is_private_v4(addr),
        IpAddr::V6(addr) => addr.is_loopback() || addr.is_unique_local(),
    }
}

fn is_private_v4(addr: Ipv4Addr) -> bool {
    addr.is_loopback()
        || addr.octets()[0] == 10
        || (addr.octets()[0] == 172 && (16..=31).contains(&addr.octets()[1]))
        || (addr.octets()[0] == 192 && addr.octets()[1] == 168)
}
