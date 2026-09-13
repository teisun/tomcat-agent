use anyhow::Error as AnyhowError;
use serde::{Deserialize, Serialize};
use std::error::Error as StdError;
use std::fmt;

use super::AppError;

/// LLM 失败的语义类别。它刻意不等同于 HTTP 状态码：同一个 403 既可能是鉴权，
/// 也可能是余额不足；流内错误则根本没有 HTTP 状态码。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LlmFailureKind {
    ContextOverflow,
    Billing,
    Authentication,
    RateLimit,
    UpstreamTransient,
    StreamInterrupted,
    EmptyResponse,
    ContentFiltered,
    UnsupportedMultimodal,
    InvalidRequest,
    Unknown,
}

impl LlmFailureKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ContextOverflow => "context_overflow",
            Self::Billing => "billing",
            Self::Authentication => "authentication",
            Self::RateLimit => "rate_limit",
            Self::UpstreamTransient => "upstream_transient",
            Self::StreamInterrupted => "stream_interrupted",
            Self::EmptyResponse => "empty_response",
            Self::ContentFiltered => "content_filtered",
            Self::UnsupportedMultimodal => "unsupported_multimodal",
            Self::InvalidRequest => "invalid_request",
            Self::Unknown => "unknown",
        }
    }
}

/// 决定恢复策略的故障域。它与 [`LlmFailureKind`] 正交：例如 ContextOverflow
/// 属于内容域但可通过压缩取得进展；Billing 属于账户域但允许用户稍后手动 Retry。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureDomain {
    Context,
    Account,
    Transport,
    Content,
    Request,
    Unknown,
}

impl FailureDomain {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Context => "context",
            Self::Account => "account",
            Self::Transport => "transport",
            Self::Content => "content",
            Self::Request => "request",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmFailure {
    pub kind: LlmFailureKind,
    pub domain: FailureDomain,
}

impl LlmFailure {
    const fn new(kind: LlmFailureKind, domain: FailureDomain) -> Self {
        Self { kind, domain }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LlmErrorStage {
    Connect,
    Send,
    BodyRead,
    IdleTimeout,
    ReadTimeout,
    NonStreamStale,
    Parse,
}

impl fmt::Display for LlmErrorStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Connect => "Connect",
            Self::Send => "Send",
            Self::BodyRead => "BodyRead",
            Self::IdleTimeout => "IdleTimeout",
            Self::ReadTimeout => "ReadTimeout",
            Self::NonStreamStale => "NonStreamStale",
            Self::Parse => "Parse",
        };
        write!(f, "{label}")
    }
}

#[derive(Debug)]
pub struct LlmError {
    provider: Option<String>,
    stage: Option<LlmErrorStage>,
    explicit_kind: Option<LlmFailureKind>,
    http_status: Option<u16>,
    retry_after_ms: Option<u64>,
    code: Option<String>,
    summary: String,
    source: Option<AnyhowError>,
}

pub fn llm_error(
    provider: impl Into<String>,
    stage: LlmErrorStage,
    summary: impl Into<String>,
) -> AppError {
    AppError::LlmDetailed(Box::new(LlmError::new(provider, stage, summary)))
}

pub fn llm_stream_terminal_error(
    provider: impl Into<String>,
    summary: impl Into<String>,
    code: Option<String>,
) -> AppError {
    AppError::LlmDetailed(Box::new(LlmError::stream_terminal(provider, summary, code)))
}

/// 流在尚未解码任何有效事件前中断或收到无法归属的首帧。该错误属于传输故障，
/// 不是「provider 协议肯定不兼容」：中转站的 SSE keepalive / 截断最常见。
pub fn llm_stream_interrupted_error(
    provider: impl Into<String>,
    summary: impl Into<String>,
) -> AppError {
    AppError::LlmDetailed(Box::new(LlmError {
        provider: Some(provider.into()),
        stage: Some(LlmErrorStage::BodyRead),
        explicit_kind: None,
        http_status: None,
        retry_after_ms: None,
        code: Some("stream_interrupted".to_string()),
        summary: summary.into(),
        source: None,
    }))
}

/// Upstream completed a request but returned neither visible text nor a tool
/// call. This is a typed transient failure, rather than a message-text match.
pub fn llm_empty_response_error(
    provider: impl Into<String>,
    summary: impl Into<String>,
) -> AppError {
    AppError::LlmDetailed(Box::new(LlmError::semantic(
        provider,
        LlmFailureKind::EmptyResponse,
        summary,
    )))
}

pub fn llm_http_status_error(
    provider: impl Into<String>,
    http_status: u16,
    body: impl Into<String>,
) -> AppError {
    let body = body.into();
    llm_http_status_error_with_summary(
        provider,
        http_status,
        format!("API 错误 {}: {}", http_status, body),
    )
}

pub fn llm_http_status_error_with_stage(
    provider: impl Into<String>,
    stage: LlmErrorStage,
    http_status: u16,
    body: impl Into<String>,
) -> AppError {
    let body = body.into();
    llm_http_status_error_with_stage_and_summary(
        provider,
        stage,
        http_status,
        format!("API 错误 {}: {}", http_status, body),
    )
}

pub fn llm_http_status_error_with_stage_and_summary(
    provider: impl Into<String>,
    stage: LlmErrorStage,
    http_status: u16,
    summary: impl Into<String>,
) -> AppError {
    AppError::LlmDetailed(Box::new(LlmError::http_status_with_stage(
        provider,
        stage,
        http_status,
        summary,
    )))
}

pub fn llm_http_status_error_with_summary(
    provider: impl Into<String>,
    http_status: u16,
    summary: impl Into<String>,
) -> AppError {
    AppError::LlmDetailed(Box::new(LlmError::http_status(
        provider,
        http_status,
        summary,
    )))
}

pub fn llm_error_with_source<E>(
    provider: impl Into<String>,
    stage: LlmErrorStage,
    summary: impl Into<String>,
    source: E,
) -> AppError
where
    E: Into<AnyhowError>,
{
    AppError::LlmDetailed(Box::new(LlmError::with_source(
        provider, stage, summary, source,
    )))
}

impl LlmError {
    pub fn new(
        provider: impl Into<String>,
        stage: LlmErrorStage,
        summary: impl Into<String>,
    ) -> Self {
        Self {
            provider: Some(provider.into()),
            stage: Some(stage),
            explicit_kind: None,
            http_status: None,
            retry_after_ms: None,
            code: None,
            summary: summary.into(),
            source: None,
        }
    }

    pub fn with_source<E>(
        provider: impl Into<String>,
        stage: LlmErrorStage,
        summary: impl Into<String>,
        source: E,
    ) -> Self
    where
        E: Into<AnyhowError>,
    {
        Self {
            provider: Some(provider.into()),
            stage: Some(stage),
            explicit_kind: None,
            http_status: None,
            retry_after_ms: None,
            code: None,
            summary: summary.into(),
            source: Some(source.into()),
        }
    }

    pub fn stream_terminal(
        provider: impl Into<String>,
        summary: impl Into<String>,
        code: Option<String>,
    ) -> Self {
        Self {
            provider: Some(provider.into()),
            stage: None,
            explicit_kind: None,
            http_status: None,
            retry_after_ms: None,
            code,
            summary: summary.into(),
            source: None,
        }
    }

    pub fn semantic(
        provider: impl Into<String>,
        kind: LlmFailureKind,
        summary: impl Into<String>,
    ) -> Self {
        Self {
            provider: Some(provider.into()),
            stage: None,
            explicit_kind: Some(kind),
            http_status: None,
            retry_after_ms: None,
            code: None,
            summary: summary.into(),
            source: None,
        }
    }

    pub fn http_status(
        provider: impl Into<String>,
        http_status: u16,
        summary: impl Into<String>,
    ) -> Self {
        Self {
            provider: Some(provider.into()),
            stage: None,
            explicit_kind: None,
            http_status: Some(http_status),
            retry_after_ms: None,
            code: None,
            summary: summary.into(),
            source: None,
        }
    }

    pub fn http_status_with_stage(
        provider: impl Into<String>,
        stage: LlmErrorStage,
        http_status: u16,
        summary: impl Into<String>,
    ) -> Self {
        Self {
            provider: Some(provider.into()),
            stage: Some(stage),
            explicit_kind: None,
            http_status: Some(http_status),
            retry_after_ms: None,
            code: None,
            summary: summary.into(),
            source: None,
        }
    }

    pub fn summary(&self) -> &str {
        &self.summary
    }

    pub fn provider(&self) -> Option<&str> {
        self.provider.as_deref()
    }

    pub fn stage(&self) -> Option<LlmErrorStage> {
        self.stage
    }

    pub fn explicit_kind(&self) -> Option<LlmFailureKind> {
        self.explicit_kind
    }

    pub fn http_status_value(&self) -> Option<u16> {
        self.http_status
    }

    pub fn with_retry_after_ms(mut self, retry_after_ms: u64) -> Self {
        self.retry_after_ms = Some(retry_after_ms);
        self
    }

    pub fn retry_after_ms(&self) -> Option<u64> {
        self.retry_after_ms
    }

    pub fn code(&self) -> Option<&str> {
        self.code.as_deref()
    }

    pub fn source_chain(&self) -> Vec<String> {
        self.source
            .as_ref()
            .map(|err| err.chain().map(ToString::to_string).collect())
            .unwrap_or_default()
    }
}

impl fmt::Display for LlmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.summary)
    }
}

impl StdError for LlmError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.source.as_ref().map(|err| err.as_ref())
    }
}

pub fn llm_stage(err: &AppError) -> Option<LlmErrorStage> {
    match err {
        AppError::LlmDetailed(detail) => detail.stage(),
        _ => None,
    }
}

pub fn llm_http_status(err: &AppError) -> Option<u16> {
    match err {
        AppError::LlmDetailed(detail) => detail.http_status_value(),
        _ => None,
    }
}

pub fn llm_retry_after_ms(err: &AppError) -> Option<u64> {
    match err {
        AppError::LlmDetailed(detail) => detail.retry_after_ms(),
        _ => None,
    }
}

pub fn llm_summary(err: &AppError) -> Option<String> {
    match err {
        AppError::LlmDetailed(detail) => Some(detail.summary().to_string()),
        AppError::Llm(message) => Some(message.clone()),
        _ => None,
    }
}

pub fn llm_code(err: &AppError) -> Option<String> {
    match err {
        AppError::LlmDetailed(detail) => detail.code().map(ToString::to_string),
        _ => None,
    }
}

pub fn llm_source_chain(err: &AppError) -> Vec<String> {
    match err {
        AppError::LlmDetailed(detail) => detail.source_chain(),
        _ => Vec::new(),
    }
}

pub fn is_retryable_llm_error(err: &AppError) -> bool {
    matches!(
        classify_llm_failure(err).kind,
        LlmFailureKind::RateLimit
            | LlmFailureKind::UpstreamTransient
            | LlmFailureKind::StreamInterrupted
            | LlmFailureKind::EmptyResponse
    )
}

pub fn is_context_overflow_text(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("context_length_exceeded")
        || lower.contains("model_context_window_exceeded")
        || lower.contains("context length exceeded")
        || lower.contains("context limit exceeded")
        || lower.contains("exceeds the context window")
        || lower.contains("maximum context length")
        || lower.contains("maximum context token limit")
        || lower.contains("prompt is too long")
        || lower.contains("reduce the length")
        || (lower.contains("input length")
            && (lower.contains("max_tokens")
                || lower.contains("max tokens")
                || lower.contains("token limit")))
        || (lower.contains("context window")
            && (lower.contains("exceed") || lower.contains("too long") || lower.contains("limit")))
}

pub fn is_unsupported_multimodal_text(text: &str) -> bool {
    let lower = text.to_lowercase();
    (lower.contains("invalid_enum_value")
        && (lower.contains("input_file") || lower.contains("input_image")))
        || lower.contains("does not support image input")
        || lower.contains("does not support file input")
        || lower.contains("unsupported content type")
}

pub fn is_deterministic_stream_refusal_text(text: &str) -> bool {
    text.to_lowercase().contains("content_filter")
}

pub fn is_context_overflow(err: &AppError) -> bool {
    matches!(
        classify_llm_failure(err).kind,
        LlmFailureKind::ContextOverflow
    )
}

/// 归一化分类的唯一入口。
///
/// 顺序是刻意固定的：已知 code > error.type > 摘要文本 > 账户状态辅证。账户类在
/// 401/403/429 之前检查其 code/type/text 证据，避免把 `insufficient_quota` 错判为
/// RateLimit，或把「余额不足」错判为 Authentication。纯 `402 Payment Required` 没有
/// 结构化错误时也归 Billing；孤立的 403 仍然只能是鉴权。
pub fn classify_llm_failure(err: &AppError) -> LlmFailure {
    if let AppError::LlmDetailed(detail) = err {
        if let Some(kind) = detail.explicit_kind() {
            return LlmFailure::new(kind, FailureDomain::Transport);
        }
    }
    let summary = llm_summary(err).unwrap_or_default();
    let (json_code, json_type) = structured_error_fields(&summary);
    let code = llm_code(err).or(json_code);
    let error_type = json_type;
    let status = llm_http_status(err);

    if matches_any(
        code.as_deref(),
        &[
            "context_length_exceeded",
            "model_context_window_exceeded",
            "context_window_exceeded",
        ],
    ) || matches_any(
        error_type.as_deref(),
        &["context_length_exceeded", "context_window_exceeded"],
    ) || status == Some(413)
        || is_context_overflow_text(&summary)
    {
        return LlmFailure::new(LlmFailureKind::ContextOverflow, FailureDomain::Context);
    }

    if is_billing_signal(code.as_deref())
        || is_billing_signal(error_type.as_deref())
        || is_billing_text(&summary)
    {
        return LlmFailure::new(LlmFailureKind::Billing, FailureDomain::Account);
    }

    // 只把具备清晰支付语义的 402 作为第四层弱证据。它必须排在 billing 文本之后、
    // authentication 之前：不能让一般 403/401 沾染账户语义，也允许中转站返回仅含
    // “Payment Required” 的空 body 时仍给用户正确的充值后 Retry 路径。
    if status == Some(402) {
        return LlmFailure::new(LlmFailureKind::Billing, FailureDomain::Account);
    }

    if matches_any(
        code.as_deref(),
        &[
            "invalid_api_key",
            "authentication_error",
            "authentication_failed",
            "unauthorized",
            "forbidden",
        ],
    ) || matches_any(
        error_type.as_deref(),
        &[
            "authentication_error",
            "invalid_api_key",
            "permission_error",
        ],
    ) || matches!(status, Some(401 | 403))
        || contains_any(
            &summary,
            &["authentication failed", "invalid api key", "unauthorized"],
        )
    {
        return LlmFailure::new(LlmFailureKind::Authentication, FailureDomain::Account);
    }

    if matches_any(
        code.as_deref(),
        &[
            "rate_limit_exceeded",
            "rate_limit_error",
            "too_many_requests",
        ],
    ) || matches_any(
        error_type.as_deref(),
        &["rate_limit_error", "rate_limit_exceeded"],
    ) || status == Some(429)
        || contains_any(&summary, &["rate limit", "too many requests"])
    {
        return LlmFailure::new(LlmFailureKind::RateLimit, FailureDomain::Transport);
    }

    if matches_any(
        code.as_deref(),
        &["upstream_error", "bad_gateway", "gateway_error"],
    ) || matches_any(error_type.as_deref(), &["upstream_error", "gateway_error"])
        || matches!(status, Some(500 | 502 | 503 | 504))
        || contains_any(
            &summary,
            &[
                "upstream_error",
                "upstream request failed",
                "bad gateway",
                "gateway error",
            ],
        )
    {
        return LlmFailure::new(LlmFailureKind::UpstreamTransient, FailureDomain::Transport);
    }

    if matches_any(code.as_deref(), &["stream_interrupted"])
        || matches!(
            llm_stage(err),
            Some(
                LlmErrorStage::Connect
                    | LlmErrorStage::Send
                    | LlmErrorStage::BodyRead
                    | LlmErrorStage::IdleTimeout
                    | LlmErrorStage::ReadTimeout
            )
        )
    {
        return LlmFailure::new(LlmFailureKind::StreamInterrupted, FailureDomain::Transport);
    }

    if is_deterministic_stream_refusal_text(&summary) {
        return LlmFailure::new(LlmFailureKind::ContentFiltered, FailureDomain::Content);
    }
    if is_unsupported_multimodal_text(&summary) {
        return LlmFailure::new(
            LlmFailureKind::UnsupportedMultimodal,
            FailureDomain::Request,
        );
    }

    if matches!(status, Some(400)) {
        return LlmFailure::new(LlmFailureKind::InvalidRequest, FailureDomain::Request);
    }
    LlmFailure::new(LlmFailureKind::Unknown, FailureDomain::Unknown)
}

fn matches_any(value: Option<&str>, candidates: &[&str]) -> bool {
    value.is_some_and(|value| {
        candidates
            .iter()
            .any(|candidate| value.eq_ignore_ascii_case(candidate))
    })
}

fn contains_any(text: &str, candidates: &[&str]) -> bool {
    let lower = text.to_lowercase();
    candidates.iter().any(|candidate| lower.contains(candidate))
}

fn is_billing_signal(value: Option<&str>) -> bool {
    matches_any(
        value,
        &[
            "insufficient_quota",
            "insufficient_balance",
            "insufficient_funds",
            "billing_error",
            "payment_required",
            "spend_limit_reached",
            "credit_limit_exceeded",
        ],
    )
}

fn is_billing_text(text: &str) -> bool {
    contains_any(
        text,
        &[
            "insufficient quota",
            "insufficient balance",
            "insufficient funds",
            "insufficient credits",
            "billing",
            "payment required",
            "spend limit",
            "spend_limit",
            "credit limit",
            "余额不足",
            "额度不足",
            "余额不够",
        ],
    )
}

fn structured_error_fields(summary: &str) -> (Option<String>, Option<String>) {
    let payload = summary
        .find('{')
        .and_then(|offset| serde_json::from_str::<serde_json::Value>(&summary[offset..]).ok());
    let Some(payload) = payload else {
        return (None, None);
    };
    let error = payload.get("error").unwrap_or(&payload);
    let code = error
        .get("code")
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string);
    let error_type = error
        .get("type")
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string);
    (code, error_type)
}

pub fn llm_connect_or_network(err: &AppError) -> bool {
    match err {
        AppError::LlmDetailed(detail) => {
            matches!(
                detail.stage(),
                Some(LlmErrorStage::Connect | LlmErrorStage::Send)
            ) || matches!(detail.http_status_value(), Some(502..=504))
        }
        _ => false,
    }
}
