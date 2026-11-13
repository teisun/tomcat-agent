use reqwest::Url;

pub(crate) fn build_path_aware_endpoint(base_url: &str, leaf: &str) -> String {
    let normalized_base = base_url.trim().trim_end_matches('/');
    let normalized_leaf = leaf.trim().trim_start_matches('/');
    if normalized_base.is_empty() {
        return format!("/v1/{normalized_leaf}");
    }
    if has_explicit_path(normalized_base) {
        format!("{normalized_base}/{normalized_leaf}")
    } else {
        format!("{normalized_base}/v1/{normalized_leaf}")
    }
}

/// OpenAI adapter mapping; product preferences remain provider-neutral.
pub(crate) fn openai_service_tier(speed: super::Speed) -> &'static str {
    match speed {
        super::Speed::Standard => "default",
        super::Speed::Fast => "priority",
    }
}

/// Normalizes a user/preset base URL to Gemini's API version root (`/v1beta` by default).
///
/// Only the final path segment is inspected:
/// - Pre-release Gemini version segments (`v1beta`, `v1alpha`, `v1beta1`, ...) stay intact.
/// - Plain major version segments (`v1`, `v2`, ...) — typically inherited from a shared
///   relay card whose placeholder teaches `https://.../v1` — are replaced with `v1beta`
///   because `parametersJsonSchema` is only served on `v1beta`.
/// - Bare hosts and gateway prefixes (`https://gw.example.com/acct/google-ai-studio`)
///   receive `/v1beta` appended.
pub(crate) fn gemini_api_root(base_url: &str) -> String {
    let trimmed = base_url.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return "/v1beta".to_string();
    }
    if !has_explicit_path(trimmed) {
        return format!("{trimmed}/v1beta");
    }
    let (prefix, last_seg) = trimmed.rsplit_once('/').unwrap_or(("", trimmed));
    let lower = last_seg.to_ascii_lowercase();
    if is_gemini_prerelease_version(&lower) {
        trimmed.to_string()
    } else if is_plain_major_version(&lower) {
        format!("{prefix}/v1beta")
    } else {
        format!("{trimmed}/v1beta")
    }
}

fn is_plain_major_version(segment: &str) -> bool {
    segment
        .strip_prefix('v')
        .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
}

fn is_gemini_prerelease_version(segment: &str) -> bool {
    let Some(rest) = segment.strip_prefix('v') else {
        return false;
    };
    let digit_len = rest.bytes().take_while(|b| b.is_ascii_digit()).count();
    if digit_len == 0 {
        return false;
    }
    let suffix = &rest[digit_len..];
    for tag in ["beta", "alpha"] {
        if let Some(tail) = suffix.strip_prefix(tag) {
            return tail.bytes().all(|b| b.is_ascii_digit());
        }
    }
    false
}

fn has_explicit_path(base_url: &str) -> bool {
    if let Ok(url) = Url::parse(base_url) {
        return !url.path().is_empty() && url.path() != "/";
    }
    let Some((_, rest)) = base_url.split_once("://") else {
        return false;
    };
    rest.contains('/')
}
