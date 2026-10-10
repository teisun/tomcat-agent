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
mod tests {
    use super::materialize_default_mcp_json;
    use crate::AppConfig;

    #[test]
    fn materializes_cursor_style_pinned_playwright_config_idempotently() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());

        let path = materialize_default_mcp_json(&cfg).expect("materialize config");
        let first = std::fs::read_to_string(&path).expect("read generated config");
        let parsed: serde_json::Value = serde_json::from_str(&first).expect("valid JSON");
        assert_eq!(
            parsed["mcpServers"]["playwright"]["command"],
            serde_json::Value::String("npx".to_string())
        );
        assert!(parsed["mcpServers"]["playwright"]["args"][1]
            .as_str()
            .expect("pinned package")
            .starts_with("@playwright/mcp@"));
        for legacy in [
            "trusted",
            "integrity",
            "startupTimeoutMs",
            "callTimeoutMs",
            "maxConcurrentCalls",
        ] {
            assert!(
                parsed["mcpServers"]["playwright"].get(legacy).is_none(),
                "{legacy}"
            );
        }
        let expected_browser_path = temp
            .path()
            .join("work")
            .join("cache")
            .join("playwright")
            .to_string_lossy()
            .into_owned();
        assert_eq!(
            parsed["mcpServers"]["playwright"]["env"]["PLAYWRIGHT_BROWSERS_PATH"].as_str(),
            Some(expected_browser_path.as_str())
        );

        materialize_default_mcp_json(&cfg).expect("idempotent materialization");
        assert_eq!(
            std::fs::read_to_string(path).expect("re-read config"),
            first
        );
    }

    #[test]
    fn preserves_a_legacy_file_instead_of_replacing_it() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
        let path = crate::core::connector::mcp::config::global_mcp_path(&cfg)
            .expect("global MCP configuration path");
        std::fs::create_dir_all(path.parent().expect("config parent")).expect("config parent");
        let legacy = r#"{"mcpServers":{"playwright":{"command":"npx","startupTimeoutMs":60000,"trusted":true}}}"#;
        std::fs::write(&path, legacy).expect("write legacy file");

        materialize_default_mcp_json(&cfg).expect("do not overwrite legacy file");
        assert_eq!(
            std::fs::read_to_string(path).expect("read legacy file"),
            legacy
        );
    }
}
