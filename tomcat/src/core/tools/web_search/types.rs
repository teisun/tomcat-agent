use std::collections::BTreeSet;
use std::net::IpAddr;

use serde::{Deserialize, Serialize};

use crate::infra::{AppError, ToolsWebSearchConfig};

use super::backend::BackendMode;

pub const MAX_QUERY_LEN: usize = 512;
pub const MAX_HIT_SNIPPET_CHARS: usize = 4_096;
pub const MAX_RESULT_SIZE_CHARS: usize = 60_000;

#[derive(Debug, Clone, Deserialize)]
pub struct WebSearchArgs {
    pub query: String,
    #[serde(default)]
    pub count: Option<u32>,
    #[serde(default)]
    pub freshness: Option<String>,
    #[serde(default)]
    pub country: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub domain_filter: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WebSearchFreshness {
    Day,
    Week,
    Month,
    Year,
}

impl WebSearchFreshness {
    pub fn parse(raw: &str) -> Result<Self, AppError> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "day" => Ok(Self::Day),
            "week" => Ok(Self::Week),
            "month" => Ok(Self::Month),
            "year" => Ok(Self::Year),
            other => Err(AppError::Tool(format!(
                "web_search: `freshness` 非法 `{other}`，允许 day/week/month/year"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Week => "week",
            Self::Month => "month",
            Self::Year => "year",
        }
    }

    pub fn as_brave_query(self) -> &'static str {
        match self {
            Self::Day => "pd",
            Self::Week => "pw",
            Self::Month => "pm",
            Self::Year => "py",
        }
    }

    pub fn as_serper_query(self) -> &'static str {
        match self {
            Self::Day => "qdr:d",
            Self::Week => "qdr:w",
            Self::Month => "qdr:m",
            Self::Year => "qdr:y",
        }
    }
}

#[derive(Debug, Clone)]
pub struct WebSearchRequest {
    pub backend: BackendMode,
    pub query: String,
    pub count: usize,
    pub freshness: Option<WebSearchFreshness>,
    pub country: Option<String>,
    pub language: Option<String>,
    /// Search-side domain allowlist, propagated into provider requests when supported.
    pub domain_filter: Vec<String>,
    /// Result-side allowlist, applied during hit normalization.
    pub allowed_domains: Vec<String>,
    /// Result-side denylist, applied during hit normalization.
    pub blocked_domains: Vec<String>,
    /// Optional per-provider base URL overrides that plugins can consume.
    pub tavily_base_url: Option<String>,
    pub brave_base_url: Option<String>,
    pub serper_base_url: Option<String>,
}

impl WebSearchRequest {
    pub fn from_tool_args(
        args: WebSearchArgs,
        cfg: &ToolsWebSearchConfig,
    ) -> Result<Self, AppError> {
        let query = args.query.trim();
        if query.is_empty() {
            return Err(AppError::Tool(
                "web_search: 缺少必填字段 `query`".to_string(),
            ));
        }
        if query.chars().count() > MAX_QUERY_LEN {
            return Err(AppError::Tool(format!(
                "web_search: `query` 过长（>{MAX_QUERY_LEN} 字符）"
            )));
        }

        let backend = BackendMode::parse(&cfg.backend)?;
        let count = args.count.unwrap_or(cfg.count) as usize;
        if !(1..=20).contains(&count) {
            return Err(AppError::Tool(format!(
                "web_search: `count` 非法 {count}，允许 [1, 20]"
            )));
        }

        let freshness = match args.freshness.as_deref().or(cfg.freshness.as_deref()) {
            Some(raw) => Some(WebSearchFreshness::parse(raw)?),
            None => None,
        };
        let country = normalize_optional_alpha_code(
            args.country.or_else(|| cfg.country.clone()),
            2,
            true,
            "country",
        )?;
        let language = normalize_optional_alpha_code(
            args.language.or_else(|| cfg.language.clone()),
            2,
            false,
            "language",
        )?;

        let domain_filter = normalize_domain_list(
            cfg.domain_filter
                .iter()
                .map(String::as_str)
                .chain(args.domain_filter.iter().map(String::as_str)),
            "domain_filter",
        )?;
        let blocked_domains = normalize_domain_list(
            cfg.blocked_domains.iter().map(String::as_str),
            "blocked_domains",
        )?;
        let allowed_domains = normalize_domain_list(
            cfg.allowed_domains
                .iter()
                .map(String::as_str)
                .chain(domain_filter.iter().map(String::as_str)),
            "allowed_domains",
        )?;

        Ok(Self {
            backend,
            query: query.to_string(),
            count,
            freshness,
            country,
            language,
            domain_filter,
            allowed_domains,
            blocked_domains,
            tavily_base_url: Some(cfg.tavily_base_url.clone()),
            brave_base_url: Some(cfg.brave_base_url.clone()),
            serper_base_url: Some(cfg.serper_base_url.clone()),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Hit {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub position: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Stats {
    pub elapsed_ms: u64,
    pub cached: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_before_filter: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WebSearchOutput {
    pub query: String,
    pub hits: Vec<Hit>,
    pub backend: String,
    pub stats: Stats,
    pub truncated: bool,
    pub warnings: Vec<String>,
}

impl WebSearchOutput {
    pub fn degraded(
        query: impl Into<String>,
        backend: impl Into<String>,
        elapsed_ms: u64,
        warnings: Vec<String>,
    ) -> Self {
        Self {
            query: query.into(),
            hits: Vec::new(),
            backend: backend.into(),
            stats: Stats {
                elapsed_ms,
                cached: false,
                total_before_filter: None,
            },
            truncated: true,
            warnings,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RawHit {
    pub title: Option<String>,
    pub url: String,
    pub snippet: Option<String>,
    pub published_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NormalizedHits {
    pub hits: Vec<Hit>,
    pub warnings: Vec<String>,
    pub truncated: bool,
    pub total_before_filter: Option<usize>,
}

pub fn normalize_hits(
    raw_hits: Vec<RawHit>,
    count: usize,
    allowed_domains: &[String],
    blocked_domains: &[String],
) -> NormalizedHits {
    let mut warnings = Vec::new();
    let mut truncated = false;
    let total_before_filter = Some(raw_hits.len());
    let mut hits = Vec::new();

    for raw in raw_hits {
        let normalized_url = raw.url.trim();
        if normalized_url.is_empty() {
            push_warning_once(&mut warnings, "skipped_invalid_url");
            continue;
        }
        let parsed = match reqwest::Url::parse(normalized_url) {
            Ok(url) => url,
            Err(_) => {
                push_warning_once(&mut warnings, "skipped_invalid_url");
                continue;
            }
        };
        let Some(host) = parsed.host_str().map(|value| value.to_ascii_lowercase()) else {
            push_warning_once(&mut warnings, "skipped_invalid_url");
            continue;
        };
        if is_private_or_local_host(&host) {
            push_warning_once(&mut warnings, "ssrf_filtered");
            continue;
        }
        if matches_domain_filter(&host, blocked_domains) {
            warnings.push(format!("domain_blocked:{host}"));
            continue;
        }
        if !allowed_domains.is_empty() && !matches_domain_filter(&host, allowed_domains) {
            warnings.push(format!("domain_filtered:{host}"));
            continue;
        }

        let mut snippet = raw.snippet.unwrap_or_default().trim().to_string();
        if snippet.chars().count() > MAX_HIT_SNIPPET_CHARS {
            snippet = truncate_chars(&snippet, MAX_HIT_SNIPPET_CHARS);
            push_warning_once(&mut warnings, "snippet_truncated");
            truncated = true;
        }

        let title = raw
            .title
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| parsed.as_str().to_string());

        hits.push(Hit {
            title,
            url: parsed.to_string(),
            snippet,
            position: 0,
            published_at: raw
                .published_at
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
        });
    }

    if hits.len() > count {
        hits.truncate(count);
        push_warning_once(&mut warnings, "count_limited");
        truncated = true;
    }

    while total_result_chars(&hits) > MAX_RESULT_SIZE_CHARS && !hits.is_empty() {
        hits.pop();
        push_warning_once(&mut warnings, "max_result_size_chars");
        truncated = true;
    }

    for (index, hit) in hits.iter_mut().enumerate() {
        hit.position = (index + 1) as u32;
    }

    NormalizedHits {
        hits,
        warnings,
        truncated,
        total_before_filter,
    }
}

fn total_result_chars(hits: &[Hit]) -> usize {
    hits.iter()
        .map(|hit| {
            hit.title.chars().count() + hit.url.chars().count() + hit.snippet.chars().count()
        })
        .sum()
}

fn push_warning_once(warnings: &mut Vec<String>, warning: &str) {
    if warnings.iter().any(|existing| existing == warning) {
        return;
    }
    warnings.push(warning.to_string());
}

fn truncate_chars(input: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (index, ch) in input.chars().enumerate() {
        if index >= max_chars {
            break;
        }
        out.push(ch);
    }
    out
}

fn normalize_optional_alpha_code(
    value: Option<String>,
    expected_len: usize,
    uppercase: bool,
    field: &str,
) -> Result<Option<String>, AppError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.chars().count() != expected_len
        || !trimmed.chars().all(|ch| ch.is_ascii_alphabetic())
    {
        return Err(AppError::Tool(format!(
            "web_search: `{field}` 非法 `{trimmed}`，要求 {expected_len} 位字母代码"
        )));
    }
    Ok(Some(if uppercase {
        trimmed.to_ascii_uppercase()
    } else {
        trimmed.to_ascii_lowercase()
    }))
}

fn normalize_domain_list<'a>(
    values: impl IntoIterator<Item = &'a str>,
    field: &str,
) -> Result<Vec<String>, AppError> {
    let mut set = BTreeSet::new();
    for value in values {
        let normalized = normalize_domain(value, field)?;
        if let Some(domain) = normalized {
            set.insert(domain);
        }
    }
    Ok(set.into_iter().collect())
}

fn normalize_domain(value: &str, field: &str) -> Result<Option<String>, AppError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }

    let maybe_url = if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        reqwest::Url::parse(trimmed)
            .ok()
            .and_then(|url| url.host_str().map(str::to_string))
    } else {
        Some(trimmed.split('/').next().unwrap_or(trimmed).to_string())
    };
    let domain = maybe_url
        .map(|value| value.trim_end_matches('.').to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::Tool(format!("web_search: `{field}` 包含非法域名 `{trimmed}`")))?;

    if !domain.contains('.') {
        return Err(AppError::Tool(format!(
            "web_search: `{field}` 包含非法域名 `{trimmed}`"
        )));
    }
    Ok(Some(domain))
}

fn matches_domain_filter(host: &str, domains: &[String]) -> bool {
    domains
        .iter()
        .any(|domain| host == domain || host.ends_with(&format!(".{domain}")))
}

fn is_private_or_local_host(host: &str) -> bool {
    if host.parse::<IpAddr>().is_ok() {
        return true;
    };
    !host.contains('.')
        || host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.ends_with(".internal")
        || host.ends_with(".localdomain")
        || host.ends_with(".home.arpa")
}
