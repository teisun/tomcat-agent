use anyhow::Error as AnyhowError;
use serde::{Deserialize, Serialize};
use std::error::Error as StdError;
use std::fmt;

use super::AppError;

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
    http_status: Option<u16>,
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
            http_status: None,
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
            http_status: None,
            summary: summary.into(),
            source: Some(source.into()),
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
            http_status: Some(http_status),
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
            http_status: Some(http_status),
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

    pub fn http_status_value(&self) -> Option<u16> {
        self.http_status
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

pub fn llm_summary(err: &AppError) -> Option<String> {
    match err {
        AppError::LlmDetailed(detail) => Some(detail.summary().to_string()),
        AppError::Llm(message) => Some(message.clone()),
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
    if let Some(status) = llm_http_status(err) {
        return matches!(status, 429 | 500 | 502 | 503 | 504);
    }
    matches!(
        llm_stage(err),
        Some(
            LlmErrorStage::Connect
                | LlmErrorStage::Send
                | LlmErrorStage::BodyRead
                | LlmErrorStage::IdleTimeout
                | LlmErrorStage::ReadTimeout
        )
    )
}

pub fn is_context_overflow_text(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("context_length_exceeded")
        || lower.contains("reduce the length")
        || (lower.contains("context")
            && (lower.contains("length") || lower.contains("token") || lower.contains("limit")))
}

pub fn is_context_overflow(err: &AppError) -> bool {
    llm_http_status(err) == Some(400)
        && llm_summary(err)
            .as_deref()
            .is_some_and(is_context_overflow_text)
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
