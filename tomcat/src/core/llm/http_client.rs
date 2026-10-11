use std::time::Duration;

use crate::infra::config::LlmRuntimeConfig;
use crate::infra::error::AppError;
use crate::infra::http_client::{
    build_outbound_client, OutboundClientErrorKind, OutboundClientOptions,
};

pub(crate) fn build_http_client(
    cfg: &LlmRuntimeConfig,
    proxy_override: Option<&str>,
) -> Result<reqwest::Client, AppError> {
    let mut options = OutboundClientOptions::new(proxy_override.or(cfg.proxy.as_deref()));
    if cfg.http_read_timeout_sec > 0 {
        options.read_timeout = Some(Duration::from_secs(cfg.http_read_timeout_sec));
    }
    build_outbound_client(
        options,
        OutboundClientErrorKind::Llm,
        &crate::infra::i18n::tr("llm.clientFailed", &[]),
    )
}
