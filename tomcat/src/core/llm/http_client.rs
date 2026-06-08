use std::time::Duration;

use crate::infra::config::LlmConfig;
use crate::infra::error::AppError;

pub(crate) fn build_http_client(
    cfg: &LlmConfig,
    proxy_override: Option<&str>,
) -> Result<reqwest::Client, AppError> {
    let mut builder = reqwest::Client::builder();
    if cfg.http_read_timeout_sec > 0 {
        builder = builder.read_timeout(Duration::from_secs(cfg.http_read_timeout_sec));
    }
    let proxy_url = proxy_override.or(cfg.proxy.as_deref());
    if let Some(proxy_url) = proxy_url {
        let proxy = reqwest::Proxy::all(proxy_url)
            .map_err(|e| AppError::Config(format!("代理 URL 无效 {}: {}", proxy_url, e)))?;
        builder = builder.proxy(proxy);
    }
    builder
        .build()
        .map_err(|e| AppError::Llm(format!("创建 HTTP 客户端失败: {}", e)))
}
