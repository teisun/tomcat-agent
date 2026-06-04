use std::path::Path;

use reqwest::Url;
use xxhash_rust::xxh32::xxh32;

use crate::infra::AppError;

use super::markdownify::normalized_content_type;

/// 根据响应头和 magic 纠正最终的 content-type。
pub(crate) fn effective_content_type(header_value: &str, body: &[u8]) -> String {
    if let Some(magic) = detect_magic_mime(body) {
        let normalized = normalized_content_type(header_value);
        if normalized != magic {
            return magic.to_string();
        }
    }
    header_value.trim().to_string()
}

/// 二进制响应的 magic 检测。
pub(crate) fn detect_magic_mime(body: &[u8]) -> Option<&'static str> {
    if body.starts_with(b"%PDF-") {
        return Some("application/pdf");
    }
    if body.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        return Some("image/png");
    }
    if body.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if body.starts_with(b"GIF8") {
        return Some("image/gif");
    }
    if body.len() >= 12 && &body[0..4] == b"RIFF" && &body[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

pub(crate) async fn persist_binary(
    persist_dir: &Path,
    url: &Url,
    content_type: &str,
    body: &[u8],
) -> Result<String, AppError> {
    persist_bytes(persist_dir, url, binary_extension(content_type), body).await
}

pub(crate) async fn persist_text(
    persist_dir: &Path,
    url: &Url,
    extension: &str,
    body: &str,
) -> Result<String, AppError> {
    persist_bytes(persist_dir, url, extension, body.as_bytes()).await
}

async fn persist_bytes(
    persist_dir: &Path,
    url: &Url,
    extension: &str,
    body: &[u8],
) -> Result<String, AppError> {
    tokio::fs::create_dir_all(persist_dir)
        .await
        .map_err(AppError::Io)?;
    let path = persist_dir.join(format!("web-fetch-{}.{}", short_url_hash(url), extension));
    tokio::fs::write(&path, body).await.map_err(|err| {
        AppError::Tool(format!(
            "web_fetch: persist failed for {}: {}",
            path.display(),
            err
        ))
    })?;
    Ok(path.to_string_lossy().into_owned())
}

fn short_url_hash(url: &Url) -> String {
    format!("{:06x}", xxh32(url.as_str().as_bytes(), 0) & 0x00ff_ffff)
}

fn binary_extension(content_type: &str) -> &'static str {
    match normalized_content_type(content_type).as_str() {
        "application/pdf" => "pdf",
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => "bin",
    }
}
