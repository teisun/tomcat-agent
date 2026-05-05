//! # Provider 注册表（同时负责 Provider 文件的 mod 声明）
//!
//! 按 [`LlmConfig::provider`] 字符串选实现：`"openai"` → [`OpenAiProvider`]（Chat Completions，
//! `POST /v1/chat/completions`），`"openai-responses"` → [`OpenAiResponsesProvider`]（Responses API，
//! `POST /v1/responses`）。详见 [`openspec/specs/architecture/llm-multiprovider-integration.md`] §6.5。
//!
//! ## 新增后端的标准动作（仅 2 步，集中在本文件 + 新文件）
//!
//! 1. 在 `core/llm/` 下新建 `<new>.rs`：`impl LlmProvider`，文件末尾按
//!    [RUST_FILE_LINES_SPEC §A 第 9 条] 自带
//!    `#[cfg(test)] #[path = "tests/<new>_test.rs"] mod tests;`，**禁止**为测试放宽可见性。
//! 2. 本文件追加两行：
//!    `#[path = "<new>.rs"] mod <new>;` 与 [`PROVIDERS`] 表里 `("<id>", build_<new>)`，
//!    并写一个 3 行的 `build_<new>(cfg)` 包成 `Arc<dyn LlmProvider>` 即可。
//!
//! 上层（`api/chat`、`compaction`、集成测试…）一律只通过 [`resolve_llm`] 拿
//! `Arc<dyn LlmProvider>`，**不感知**任何 concrete Provider 类型；因此 `core/llm/mod.rs`
//! 与 `lib.rs` 也不需要为新 Provider 增加任何 `mod` 或 `pub use`。
//!
//! ## 设计约束
//!
//! - **不**修改 [`LlmConfig`] schema 引入 vendor 专属字段（spec §6.5.2 「稳定 schema」）；
//! - 公共横切字段（`api_base` / `api_key_env` / `proxy` / `retry_count` / `stream_timeout_sec`
//!   / `api_base_fallback` / `max_concurrent_requests`）由所有 Provider 共享。

use std::sync::Arc;

use crate::infra::config::LlmConfig;
use crate::infra::error::AppError;

use super::provider::LlmProvider;

#[path = "openai.rs"]
mod openai;
#[path = "openai_responses.rs"]
mod openai_responses;

use openai::OpenAiProvider;
use openai_responses::OpenAiResponsesProvider;

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
