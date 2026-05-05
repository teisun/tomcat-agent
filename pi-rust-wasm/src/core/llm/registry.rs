//! # Provider 注册表
//!
//! 按 [`LlmConfig::provider`] 字符串选实现：`"openai"` → [`OpenAiProvider`]（Chat Completions，
//! `POST /v1/chat/completions`），`"openai-responses"` → [`OpenAiResponsesProvider`]（Responses API，
//! `POST /v1/responses`）。详见 [`openspec/specs/architecture/llm-multiprovider-integration.md`] §6.5。
//!
//! 新增后端的标准动作：
//! 1. 在 `core/llm/` 下新增 `xxx.rs`，`impl LlmProvider`；
//! 2. 在 [`PROVIDERS`] 表里追加一行 `("xxx", |cfg| ...)` 即可；
//! 3. **不**修改 [`LlmConfig`] schema 引入 vendor 专属字段（spec §6.5.2 「稳定 schema」）。

use std::sync::Arc;

use crate::infra::config::LlmConfig;
use crate::infra::error::AppError;

use super::openai::OpenAiProvider;
use super::openai_responses::OpenAiResponsesProvider;
use super::provider::LlmProvider;

type ProviderCtor = fn(&LlmConfig) -> Result<Arc<dyn LlmProvider>, AppError>;

/// 已注册 Provider 列表；新增条目即扩展，无需改其他位置。
const PROVIDERS: &[(&str, ProviderCtor)] = &[
    ("openai", build_openai_completions),
    ("openai-responses", build_openai_responses),
];

fn build_openai_completions(cfg: &LlmConfig) -> Result<Arc<dyn LlmProvider>, AppError> {
    Ok(Arc::new(OpenAiProvider::new(cfg)?))
}

fn build_openai_responses(cfg: &LlmConfig) -> Result<Arc<dyn LlmProvider>, AppError> {
    Ok(Arc::new(OpenAiResponsesProvider::new(cfg)?))
}

/// 按 `cfg.provider` 字符串查表构造 [`Arc<dyn LlmProvider>`]；未知 id 返回 [`AppError::Config`]
/// 并列出当前已注册的 id 集合，便于用户排查 `[llm] provider = ?`。
pub fn resolve_llm(cfg: &LlmConfig) -> Result<Arc<dyn LlmProvider>, AppError> {
    match PROVIDERS.iter().find(|(id, _)| *id == cfg.provider) {
        Some((_, ctor)) => ctor(cfg),
        None => Err(AppError::Config(format!(
            "未知 [llm] provider = {:?}; 已注册: {:?}",
            cfg.provider,
            PROVIDERS.iter().map(|(id, _)| *id).collect::<Vec<_>>()
        ))),
    }
}

/// 已注册的 provider id 集合（供测试与文档/工具引用，避免在外部硬编码）。
pub fn registered_provider_ids() -> Vec<&'static str> {
    PROVIDERS.iter().map(|(id, _)| *id).collect()
}
