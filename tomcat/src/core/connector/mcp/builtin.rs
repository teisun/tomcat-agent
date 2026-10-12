use std::path::PathBuf;

use crate::core::connector::mcp::config::global_mcp_path;
use crate::infra::config::get_work_dir;
use crate::infra::error::AppError;
use crate::AppConfig;

const PLAYWRIGHT_MCP_VERSION: &str = "0.0.79";

pub fn materialize_default_mcp_json(cfg: &AppConfig) -> Result<PathBuf, AppError> {
    let path = global_mcp_path(cfg)?;
    if path.exists() {
        return Ok(path);
    }
    let browser_path = get_work_dir(cfg)?.join("cache").join("playwright");
    let contents = serde_json::to_vec_pretty(&serde_json::json!({
        "mcpServers": {
            "playwright": {
                "command": "npx",
                "args": ["-y", format!("@playwright/mcp@{PLAYWRIGHT_MCP_VERSION}"), "--headless"],
                "env": {
                    "PLAYWRIGHT_BROWSERS_PATH": browser_path,
                },
            }
        }
    }))
    .map_err(|error| {
        AppError::Config(crate::infra::i18n::tr(
            "mcp.defaultSerialize",
            &[("detail", &error.to_string())],
        ))
    })?;
    crate::infra::platform::write_file_atomic(&path, &contents)?;
    Ok(path)
}

#[cfg(test)]
#[path = "tests/builtin_test.rs"]
mod tests;
