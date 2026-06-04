use reqwest::Url;

/// 仅允许同源或 `apex <-> www` 变体重定向。
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

fn is_www_variant(left: &str, right: &str) -> bool {
    left.strip_prefix("www.") == Some(right) || right.strip_prefix("www.") == Some(left)
}
