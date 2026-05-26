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
