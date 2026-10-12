use std::path::Path;

use super::StdioTransport;
use crate::core::connector::mcp::config::{
    ConfiguredMcpServer, McpConfigSource, McpServerConfig, ToolFilter,
};

#[test]
fn configured_env_is_added_after_environment_is_cleared() {
    let transport = StdioTransport::new(Some(Path::new("/workspace")));
    let server = ConfiguredMcpServer {
        config_key: "mcp:test".to_string(),
        name: "test".to_string(),
        source: McpConfigSource::Project,
        config: McpServerConfig {
            command: "echo".to_string(),
            args: Vec::new(),
            env: [("EXPLICIT".to_string(), "value".to_string())].into(),
            url: None,
            auth: None,
            headers: Default::default(),
            oauth: None,

            cwd: Some(std::path::PathBuf::from(".")),
            tool_filter: ToolFilter::default(),
        },
    };
    let command = transport.command(&server);
    assert_eq!(
        command.as_std().get_current_dir(),
        Some(Path::new("/workspace"))
    );
}

#[test]
fn curated_playwright_passes_bootstrap_system_browser_fallback_to_mcp() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let browser_root = temp.path().join("browser-cache");
    std::fs::create_dir_all(&browser_root).expect("browser cache");
    let executable = temp.path().join("chrome");
    std::fs::write(&executable, "").expect("system browser fixture");
    std::fs::write(
        browser_root.join("system-browser.json"),
        serde_json::json!({ "executablePath": executable }).to_string(),
    )
    .expect("fallback marker");
    let transport = StdioTransport::new(Some(Path::new("/workspace")));
    let server = ConfiguredMcpServer {
        config_key: "mcp:playwright".to_string(),
        name: "playwright".to_string(),
        source: McpConfigSource::Global,
        config: McpServerConfig {
            command: "npx".to_string(),
            args: Vec::new(),
            env: [(
                "PLAYWRIGHT_BROWSERS_PATH".to_string(),
                browser_root.to_string_lossy().into_owned(),
            )]
            .into(),
            url: None,
            auth: None,
            headers: Default::default(),
            oauth: None,
            cwd: None,
            tool_filter: ToolFilter::default(),
        },
    };

    let command = transport.command(&server);
    assert!(
        command.as_std().get_envs().any(|(key, value)| {
            key == "PLAYWRIGHT_MCP_EXECUTABLE_PATH" && value == Some(executable.as_os_str())
        }),
        "curated MCP must receive Phase 1's fallback browser path"
    );
}

#[test]
fn http_config_separates_bearer_from_custom_headers() {
    let temp = tempfile::tempdir().expect("temp");
    let mut app_config = crate::AppConfig::default();
    app_config.storage.work_dir = Some(temp.path().to_string_lossy().into_owned());
    let store =
        crate::core::connector::mcp::oauth::OAuthTokenStore::open(&app_config).expect("store");
    let transport = super::HttpTransport::new(store);
    let server = ConfiguredMcpServer {
        config_key: "mcp:http".to_string(),
        name: "http".to_string(),
        source: McpConfigSource::Global,
        config: McpServerConfig {
            command: String::new(),
            args: Vec::new(),
            url: Some("https://example.test/mcp".to_string()),
            auth: Some("bearer".to_string()),
            headers: [
                (
                    "Authorization".to_string(),
                    "Bearer static-token".to_string(),
                ),
                ("X-Test".to_string(), "ok".to_string()),
            ]
            .into(),
            oauth: None,
            env: Default::default(),
            cwd: None,
            tool_filter: ToolFilter::default(),
        },
    };
    let config = transport.config(&server).expect("HTTP config");
    assert_eq!(config.auth_header.as_deref(), Some("static-token"));
    assert_eq!(config.custom_headers.len(), 1);
    assert_eq!(config.custom_headers.values().next().unwrap(), "ok");
    assert_eq!(config.max_concurrent_requests, 16);
    assert!(!config.reinit_on_expired_session);
    for limit in [1, 32, 64] {
        let overridden = transport.clone().with_max_concurrent_requests(limit);
        assert_eq!(
            overridden.config(&server).unwrap().max_concurrent_requests,
            limit
        );
    }
}
