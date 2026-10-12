use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

use crate::infra::error::AppError;

pub const DEFAULT_MCP_STARTUP_TIMEOUT_MS: u64 = 30_000;
pub const DEFAULT_MCP_CALL_TIMEOUT_MS: u64 = 120_000;
pub const DEFAULT_MCP_MAX_CONCURRENT_CALLS: usize = 16;

/// One process-wide policy, shared by all configured MCP servers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct McpRuntimeConfig {
    pub startup_timeout_ms: u64,
    pub call_timeout_ms: u64,
    pub max_concurrent_calls: usize,
}

impl Default for McpRuntimeConfig {
    fn default() -> Self {
        Self {
            startup_timeout_ms: DEFAULT_MCP_STARTUP_TIMEOUT_MS,
            call_timeout_ms: DEFAULT_MCP_CALL_TIMEOUT_MS,
            max_concurrent_calls: DEFAULT_MCP_MAX_CONCURRENT_CALLS,
        }
    }
}

impl McpRuntimeConfig {
    /// Used both for layered TOML/env loading and directly constructed configs.
    pub fn validate(&self) -> Result<(), AppError> {
        for (name, millis) in [
            ("startup_timeout_ms", self.startup_timeout_ms),
            ("call_timeout_ms", self.call_timeout_ms),
        ] {
            let duration = Duration::from_millis(millis);
            // Some platforms accept enormous Instants that the async timer cannot
            // represent portably; keep the nanosecond conversion within i64.
            if millis == 0
                || duration.as_nanos() > i64::MAX as u128
                || Instant::now().checked_add(duration).is_none()
            {
                return Err(AppError::Config(crate::infra::i18n::tr(
                    "config.mcpTimeout",
                    &[("field", name), ("value", &millis.to_string())],
                )));
            }
        }
        if !(1..=64).contains(&self.max_concurrent_calls) {
            return Err(AppError::Config(crate::infra::i18n::tr(
                "config.mcpConcurrency",
                &[("value", &self.max_concurrent_calls.to_string())],
            )));
        }
        Ok(())
    }
}

/// External connector subsystem. It starts no process until an MCP server is
/// configured; the switch remains available for one-step global disablement.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ConnectorConfig {
    pub enabled: bool,
    pub disabled: Vec<String>,
    pub mcp: McpRuntimeConfig,
}

impl Default for ConnectorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            disabled: Vec::new(),
            mcp: McpRuntimeConfig::default(),
        }
    }
}
