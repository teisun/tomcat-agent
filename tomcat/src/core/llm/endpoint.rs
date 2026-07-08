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

fn has_explicit_path(base_url: &str) -> bool {
    if let Ok(url) = Url::parse(base_url) {
        return !url.path().is_empty() && url.path() != "/";
    }
    let Some((_, rest)) = base_url.split_once("://") else {
        return false;
    };
    rest.contains('/')
}

#[cfg(test)]
mod tests {
    use super::build_path_aware_endpoint;

    #[test]
    fn adds_v1_for_bare_hosts() {
        assert_eq!(
            build_path_aware_endpoint("https://api.openai.com", "responses"),
            "https://api.openai.com/v1/responses"
        );
        assert_eq!(
            build_path_aware_endpoint("https://api.example.test", "chat/completions"),
            "https://api.example.test/v1/chat/completions"
        );
        assert_eq!(
            build_path_aware_endpoint("https://api.example.test", "messages"),
            "https://api.example.test/v1/messages"
        );
    }

    #[test]
    fn preserves_explicit_provider_paths() {
        assert_eq!(
            build_path_aware_endpoint(
                "https://open.bigmodel.cn/api/paas/v4/",
                "chat/completions",
            ),
            "https://open.bigmodel.cn/api/paas/v4/chat/completions"
        );
        assert_eq!(
            build_path_aware_endpoint("https://api.anthropic.com/v1", "messages"),
            "https://api.anthropic.com/v1/messages"
        );
    }
}
