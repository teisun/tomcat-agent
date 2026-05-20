//! 轻量 URL-like 判定 helper。
//!
//! 本轮只区分最小集合：`http://` / `https://`。
//! 不做完整 URL 解析，也不承诺 host / path / query 语义。

/// 判断一个字符串是否是本轮需要特殊处理的 URL-like 目标。
pub fn is_url_like(raw: &str) -> bool {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::is_url_like;

    #[test]
    fn detects_http_and_https() {
        assert!(is_url_like("http://127.0.0.1:4173/"));
        assert!(is_url_like("https://example.com/api"));
        assert!(is_url_like(" HTTPS://EXAMPLE.COM "));
    }

    #[test]
    fn rejects_plain_paths_and_other_strings() {
        assert!(!is_url_like("/tmp/file"));
        assert!(!is_url_like("./src/main.rs"));
        assert!(!is_url_like("localhost:4173"));
        assert!(!is_url_like(""));
    }
}
