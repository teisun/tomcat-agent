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
    RequestTimeout,
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
            Self::RequestTimeout => "RequestTimeout",
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
            summary: summary.into(),
            source: Some(source.into()),
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

fn parse_legacy_stage(message: &str) -> Option<LlmErrorStage> {
    if message.contains("流式空闲超时") {
        return Some(LlmErrorStage::IdleTimeout);
    }
    if message.contains("读取响应失败") || message.contains("流读取错误") {
        return Some(LlmErrorStage::BodyRead);
    }
    if message.contains("解析响应失败")
        || message.contains("解析 SSE 行失败")
        || message.contains("解析 Responses chunk 失败")
        || message.contains("UTF-8 错误")
    {
        return Some(LlmErrorStage::Parse);
    }
    if message.contains("流式请求失败") || message.contains("请求失败") {
        return Some(LlmErrorStage::Send);
    }
    None
}

pub fn llm_stage(err: &AppError) -> Option<LlmErrorStage> {
    match err {
        AppError::LlmDetailed(detail) => detail.stage(),
        AppError::Llm(message) => parse_legacy_stage(message),
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

pub fn llm_connect_or_network(err: &AppError) -> bool {
    match err {
        AppError::LlmDetailed(detail) => matches!(detail.stage(), Some(LlmErrorStage::Connect)),
        AppError::Llm(message) => {
            message.contains("请求失败")
                && (message.contains("Connect")
                    || message.contains("connection")
                    || message.contains("timed out")
                    || message.contains("timeout")
                    || message.contains("dns")
                    || message.contains("connection refused"))
        }
        _ => false,
    }
}
