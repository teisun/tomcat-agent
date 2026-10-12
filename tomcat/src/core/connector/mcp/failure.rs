//! Structured, safe connector failures. Display/Debug never expose URLs,
//! credentials, challenges, JSON-RPC data, or response-body previews.
use crate::infra::i18n::tr;
use std::fmt;

use rmcp::service::ClientInitializeError;
use rmcp::transport::streamable_http_client::StreamableHttpError;
use rmcp::ServiceError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    Configuration,
    Authorization,
    Forbidden,
    SessionExpired,
    Transient,
    Timeout,
    Cancelled,
    Protocol,
    Unknown,
}

#[derive(Clone)]
pub struct McpFailure {
    pub kind: FailureKind,
    pub phase: &'static str,
    pub http_status: Option<u16>,
    pub rpc_code: Option<i32>,
    // Retained only for a checked, reactive OAuth discovery, never formatting.
    pub(crate) challenge: Option<String>,
}

impl McpFailure {
    pub fn new(kind: FailureKind, phase: &'static str) -> Self {
        Self {
            kind,
            phase,
            http_status: None,
            rpc_code: None,
            challenge: None,
        }
    }

    pub fn retryable_startup(&self) -> bool {
        matches!(self.kind, FailureKind::Transient | FailureKind::Timeout)
    }

    pub fn from_initialization(error: &ClientInitializeError) -> Self {
        match error {
            ClientInitializeError::TransportError { error, .. } => {
                Self::from_error(error.error.as_ref(), "initialize")
            }
            ClientInitializeError::LegacyFallbackFailed { fallback, .. } => {
                Self::from_initialization(fallback)
            }
            ClientInitializeError::ConnectionClosed(_) => {
                Self::new(FailureKind::Transient, "initialize")
            }
            ClientInitializeError::Cancelled => Self::new(FailureKind::Cancelled, "initialize"),
            ClientInitializeError::JsonRpcError(error) => {
                let mut failure = Self::new(FailureKind::Protocol, "initialize");
                failure.rpc_code = Some(error.code.0);
                failure
            }
            _ => Self::new(FailureKind::Protocol, "initialize"),
        }
    }

    pub fn from_service(error: &ServiceError, phase: &'static str) -> Self {
        match error {
            ServiceError::TransportSend(error) => Self::from_error(error.error.as_ref(), phase),
            ServiceError::Timeout { .. } => Self::new(FailureKind::Timeout, phase),
            ServiceError::Cancelled { .. } => Self::new(FailureKind::Cancelled, phase),
            ServiceError::McpError(error) => {
                let mut failure = Self::new(FailureKind::Protocol, phase);
                failure.rpc_code = Some(error.code.0);
                failure
            }
            // A closed response does not prove connection death. During startup
            // this permits another bounded attempt, not replay of a tools/call.
            ServiceError::TransportClosed => Self::new(FailureKind::Transient, phase),
            _ => Self::new(FailureKind::Unknown, phase),
        }
    }

    pub fn from_error(error: &(dyn std::error::Error + 'static), phase: &'static str) -> Self {
        let mut cursor = Some(error);
        while let Some(error) = cursor {
            if let Some(error) = error.downcast_ref::<StreamableHttpError<McpFailure>>() {
                let mut failure = match error {
                    StreamableHttpError::Client(failure) => failure.clone(),
                    StreamableHttpError::SessionExpired => {
                        let mut failure = Self::new(FailureKind::SessionExpired, phase);
                        failure.http_status = Some(404);
                        failure
                    }
                    _ => Self::new(FailureKind::Unknown, phase),
                };
                failure.phase = phase;
                return failure;
            }
            if let Some(failure) = error.downcast_ref::<Self>() {
                return Self {
                    phase,
                    ..failure.clone()
                };
            }
            // Direct SDK capability probes still use its plain reqwest client.
            if matches!(
                error.downcast_ref::<StreamableHttpError<reqwest::Error>>(),
                Some(StreamableHttpError::SessionExpired)
            ) {
                let mut failure = Self::new(FailureKind::SessionExpired, phase);
                failure.http_status = Some(404);
                return failure;
            }
            cursor = error.source();
        }
        Self::new(FailureKind::Unknown, phase)
    }

    pub fn to_app_error(&self, server: &str) -> crate::infra::error::AppError {
        crate::infra::error::AppError::Tool(tr(
            "mcp.sourceFailure",
            &[("source", server), ("detail", &self.to_string())],
        ))
    }
}

impl From<crate::infra::error::AppError> for McpFailure {
    fn from(error: crate::infra::error::AppError) -> Self {
        let kind = if matches!(error, crate::infra::error::AppError::Config(_)) {
            FailureKind::Configuration
        } else {
            FailureKind::Unknown
        };
        Self::new(kind, "prepare")
    }
}

impl fmt::Display for McpFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reason = tr(
            match self.kind {
                FailureKind::Configuration => "mcp.failure.configuration",
                FailureKind::Authorization => "mcp.failure.authorization",
                FailureKind::Forbidden => "mcp.failure.forbidden",
                FailureKind::SessionExpired => "mcp.failure.expired",
                FailureKind::Transient => "mcp.failure.transient",
                FailureKind::Timeout => "mcp.failure.timeout",
                FailureKind::Cancelled => "mcp.failure.cancelled",
                FailureKind::Protocol => "mcp.failure.protocol",
                FailureKind::Unknown => "mcp.failure.unknown",
            },
            &[],
        );
        let phase = match self.phase {
            "source explicitly stopped; use Reload" => tr("mcp.phase.stopped", &[]),
            "source overridden" => tr("mcp.phase.overridden", &[]),
            phase => phase.to_string(),
        };
        write!(f, "{phase}: {reason}")?;
        if let Some(status) = self.http_status {
            write!(f, " (HTTP {status})")?;
        }
        if let Some(code) = self.rpc_code {
            write!(f, " (JSON-RPC {code})")?;
        }
        Ok(())
    }
}
impl fmt::Debug for McpFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}
impl std::error::Error for McpFailure {}
