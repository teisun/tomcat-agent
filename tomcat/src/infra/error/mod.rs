//! # 统一错误模块 (AppError)
//!
//! 项目统一错误枚举，各层通过 [`Result`]`<T, AppError>` 或 `anyhow` 包装使用。
//! MVP 会话与审计均不使用 SQLite，故不包含 Db 变体。

use thiserror::Error;

mod llm;

#[cfg(test)]
pub use llm::is_context_overflow_text;
#[cfg(test)]
pub use llm::is_deterministic_stream_refusal_text;
#[cfg(test)]
pub use llm::llm_code;
pub use llm::{
    classify_llm_failure, is_context_overflow, is_retryable_llm_error,
    is_unsupported_multimodal_text, llm_connect_or_network, llm_empty_response_error, llm_error,
    llm_error_with_source, llm_http_status, llm_http_status_error,
    llm_http_status_error_with_stage, llm_http_status_error_with_summary, llm_retry_after_ms,
    llm_source_chain, llm_stage, llm_stream_interrupted_error, llm_stream_terminal_error,
    llm_summary, LlmError, LlmErrorStage, LlmFailureKind,
};
#[allow(unused_imports)]
pub use llm::{FailureDomain, LlmFailure};

/// 项目统一错误枚举，覆盖 IO、配置、插件、事件、4 原语、工具、序列化等场景。
#[derive(Debug, Error)]
pub enum AppError {
    /// IO 操作失败，如文件不存在、磁盘空间不足或权限不足。
    #[error("{prefix}: {0}", prefix = crate::infra::i18n::tr("error.prefix.io", &[]))]
    Io(#[from] std::io::Error),
    /// 大模型调用失败，如 API 超时、限流或返回错误。
    #[error("{prefix}: {0}", prefix = crate::infra::i18n::tr("error.prefix.llm", &[]))]
    Llm(String),
    /// 结构化 LLM 错误：保留 provider / stage / http_status / source chain，UI 仍只展示 summary。
    #[error("{prefix}: {0}", prefix = crate::infra::i18n::tr("error.prefix.llm", &[]))]
    LlmDetailed(#[from] Box<LlmError>),
    /// 插件运行时错误，如 WASM 加载失败或插件逻辑异常。
    #[error("{prefix}: {0}", prefix = crate::infra::i18n::tr("error.prefix.plugin", &[]))]
    Plugin(String),
    /// 工具底层执行异常（read/write/edit/bash 等）。
    #[error("{prefix}: {0}", prefix = crate::infra::i18n::tr("error.prefix.primitive", &[]))]
    Primitive(String),
    /// 事件总线回调返回错误；单 listener 错误会被捕获并记录，不中断其他 listener。
    #[error("{prefix}: {0}", prefix = crate::infra::i18n::tr("error.prefix.event", &[]))]
    Event(String),
    /// 配置加载、解析或合法性校验失败。
    #[error("{prefix}: {0}", prefix = crate::infra::i18n::tr("error.prefix.config", &[]))]
    Config(String),
    /// 权限校验失败，如路径不在白名单或命令被禁止。
    #[error("{prefix}: {0}", prefix = crate::infra::i18n::tr("error.prefix.permission", &[]))]
    Permission(String),
    /// 工具调用失败，如参数校验或执行异常。
    #[error("{prefix}: {0}", prefix = crate::infra::i18n::tr("error.prefix.tool", &[]))]
    Tool(String),
    /// JSON 序列化/反序列化错误。
    #[error("{prefix}: {0}", prefix = crate::infra::i18n::tr("error.prefix.serialize", &[]))]
    Serialize(#[from] serde_json::Error),
    /// QuickJS 执行错误。
    #[error("{prefix}: {0}", prefix = crate::infra::i18n::tr("error.prefix.quickjs", &[]))]
    QuickJS(String),
    /// 审计日志写入或查询错误。
    #[error("{prefix}: {0}", prefix = crate::infra::i18n::tr("error.prefix.audit", &[]))]
    Audit(String),
    /// 内部逻辑错误（不可恢复）。
    #[error("{prefix}: {0}", prefix = crate::infra::i18n::tr("error.prefix.internal", &[]))]
    Internal(String),
    /// 内部不变量失败（可诊断、不可恢复），例如消息链违规。
    #[error("{prefix}[{stage}]: {detail}", prefix = crate::infra::i18n::tr("error.prefix.invariant", &[]))]
    Invariant { stage: &'static str, detail: String },
    /// `apply_boundary` 时 `covered_end_id` 在当前 `messages` 中无法匹配（陈旧结果，不可重试 restore）。
    #[error("apply_boundary: {reason} ({covered_end_id:?})", reason = crate::infra::i18n::tr("error.applyBoundaryStale", &[]))]
    ApplyBoundaryStale { covered_end_id: String },
}

impl AppError {
    pub fn internal(msg: &str) -> Self {
        AppError::Internal(msg.to_string())
    }

    pub fn invariant(stage: &'static str, detail: impl Into<String>) -> Self {
        AppError::Invariant {
            stage,
            detail: detail.into(),
        }
    }
}

#[cfg(test)]
mod tests;
