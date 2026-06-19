//! # Provider 注册表（同时负责 Provider 文件的 mod 声明）
//!
//! 注册表按 `ModelEntry.api` 选择 wire adapter：
//! - `"openai"` -> [`OpenAiProvider`]（`POST /v1/chat/completions`）
//! - `"openai-responses"` -> [`OpenAiResponsesProvider`]（`POST /v1/responses`）
//!
//! 模型怎么连（`base_url` / `api_key_env` / `model_name`）来自 [`ModelEntry`]；
//! 全局重试/超时/proxy 等运行时参数来自 [`LlmRuntimeConfig`]；
//! 凭证值来自 [`Credential`]。
//! 当前内置模型只有 `gpt-5.4` / `deepseek-v4-pro`；其余模型通常来自 `models.toml`。
//!
//! 因此这里不再接收整个 [`LlmConfig`]，也不再做“克隆配置再覆写四个字段”的桥接。

use std::sync::Arc;

use crate::infra::config::LlmRuntimeConfig;
use crate::infra::error::AppError;

use super::auth::Credential;
use super::catalog::ModelEntry;
use super::provider::LlmProvider;

#[path = "openai.rs"]
mod openai;
// `openai_responses` 已升级为目录模块（L-3 拆分整改：mod / payload / stream 三文件）。
// 用 `#[path = "<dir>/mod.rs"]` 显式锁定入口，与既有「在 registry 内本地声明 mod」
// 风格对齐；新增 single-file Provider 仍可走 `#[path = "<new>.rs"]`。
#[path = "openai_responses/mod.rs"]
mod openai_responses;

use openai::OpenAiProvider;
use openai_responses::OpenAiResponsesProvider;

type ProviderCtor =
    fn(&ModelEntry, &LlmRuntimeConfig, &Credential) -> Result<Arc<dyn LlmProvider>, AppError>;

/// 已注册 Provider 列表；新增条目即扩展，无需改其他位置。
const PROVIDERS: &[(&str, ProviderCtor)] = &[
    ("openai", build_openai_completions),
    ("openai-responses", build_openai_responses),
];

fn build_openai_completions(
    entry: &ModelEntry,
    runtime: &LlmRuntimeConfig,
    credential: &Credential,
) -> Result<Arc<dyn LlmProvider>, AppError> {
    Ok(Arc::new(OpenAiProvider::new(entry, runtime, credential)?))
}

fn build_openai_responses(
    entry: &ModelEntry,
    runtime: &LlmRuntimeConfig,
    credential: &Credential,
) -> Result<Arc<dyn LlmProvider>, AppError> {
    Ok(Arc::new(OpenAiResponsesProvider::new(
        entry, runtime, credential,
    )?))
}

/// 按 `entry.api` 字符串查表构造 [`Arc<dyn LlmProvider>`]；未知 id 返回 [`AppError::Config`]
/// 并列出当前已注册的 id 集合，便于用户排查 `models.toml` 中的 `api = ?`。
pub fn build_provider(
    entry: &ModelEntry,
    runtime: &LlmRuntimeConfig,
    credential: &Credential,
) -> Result<Arc<dyn LlmProvider>, AppError> {
    match PROVIDERS.iter().find(|(id, _)| *id == entry.api) {
        Some((_, ctor)) => ctor(entry, runtime, credential),
        None => Err(AppError::Config(format!(
            "未知模型 `{}` 的 api = {:?}; 已注册: {:?}",
            entry.id,
            entry.api,
            PROVIDERS.iter().map(|(id, _)| *id).collect::<Vec<_>>()
        ))),
    }
}

/// 已注册的 wire api id 集合（供测试与文档/工具引用，避免在外部硬编码）。
pub fn registered_provider_ids() -> Vec<&'static str> {
    PROVIDERS.iter().map(|(id, _)| *id).collect()
}
