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
        super::Speed::Ultrafast => "ultrafast",
    }
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
