use std::path::{Path, PathBuf};

use async_trait::async_trait;
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use rmcp::ServiceExt;

use crate::core::connector::mcp::config::ConfiguredMcpServer;
use crate::core::connector::mcp::oauth::{OAuthDiscovery, OAuthTokenStore};
use crate::infra::error::AppError;

pub type McpClient = rmcp::service::RunningService<rmcp::RoleClient, ()>;

#[async_trait]
pub trait McpTransport: Send + Sync {
    async fn connect(&self, server: &ConfiguredMcpServer) -> Result<McpClient, AppError>;
}

/// Client-side stdio transport. Only PATH and HOME survive env_clear; configured
/// environment variables are added explicitly, so an MCP configuration cannot
/// silently inherit unrelated process secrets.
pub struct StdioTransport {
    workspace_root: Option<PathBuf>,
}

impl StdioTransport {
    pub fn new(workspace_root: Option<&Path>) -> Self {
        Self {
            workspace_root: workspace_root.map(Path::to_path_buf),
        }
    }

    fn command(&self, server: &ConfiguredMcpServer) -> tokio::process::Command {
        tokio::process::Command::new(&server.config.command).configure(|command| {
            command.args(&server.config.args);
            if let Some(cwd) = server.config.cwd.as_deref() {
                let cwd = Path::new(cwd);
                if cwd.is_absolute() {
                    command.current_dir(cwd);
                } else if let Some(workspace_root) = self.workspace_root.as_deref() {
                    command.current_dir(workspace_root.join(cwd));
                }
            }
            command.env_clear();
            for key in ["PATH", "HOME"] {
                if let Some(value) = std::env::var_os(key) {
                    command.env(key, value);
                }
            }
            command.envs(&server.config.env);
            if let Some(executable_path) = managed_playwright_executable(server) {
                command.env("PLAYWRIGHT_MCP_EXECUTABLE_PATH", executable_path);
            }
        })
    }
}

/// Phase 1's bootstrap records the system Chrome fallback here when Playwright
/// cannot download a bundled Chromium (notably macOS 13). `@playwright/mcp` is
/// a separate process and cannot discover that marker itself, so the curated
/// Playwright connector bridges it through the MCP server's documented env var.
fn managed_playwright_executable(server: &ConfiguredMcpServer) -> Option<PathBuf> {
    if server.name != "playwright"
        || server
            .config
            .env
            .contains_key("PLAYWRIGHT_MCP_EXECUTABLE_PATH")
    {
        return None;
    }
    let browser_root = server.config.env.get("PLAYWRIGHT_BROWSERS_PATH")?;
    let marker_path = Path::new(browser_root).join("system-browser.json");
    let marker = std::fs::read_to_string(marker_path).ok()?;
    let marker = serde_json::from_str::<serde_json::Value>(&marker).ok()?;
    let executable_path = marker.get("executablePath")?.as_str()?;
    let executable_path = PathBuf::from(executable_path);
    executable_path.is_file().then_some(executable_path)
}

#[async_trait]
impl McpTransport for StdioTransport {
    async fn connect(&self, server: &ConfiguredMcpServer) -> Result<McpClient, AppError> {
        let transport = TokioChildProcess::new(self.command(server)).map_err(|error| {
            AppError::Tool(format!("spawn MCP server '{}': {error}", server.name))
        })?;
        ().serve(transport).await.map_err(|error| {
            AppError::Tool(format!("initialize MCP server '{}': {error}", server.name))
        })
    }
}

/// Client-side Streamable HTTP transport backed by rmcp's reqwest adapter.
///
/// Authentication is deliberately kept at the transport boundary: ordinary
/// headers are copied to every MCP request, while an Authorization bearer value
/// is passed through rmcp's dedicated auth slot so it cannot be rejected as a
/// reserved custom header. OAuth token acquisition/refresh will use the same
/// slot once the connector OAuth store is wired in.
#[derive(Debug, Clone)]
pub struct HttpTransport {
    oauth_store: OAuthTokenStore,
}

impl HttpTransport {
    pub fn new(oauth_store: OAuthTokenStore) -> Self {
        Self { oauth_store }
    }

    fn config(
        &self,
        server: &ConfiguredMcpServer,
    ) -> Result<
        rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig,
        AppError,
    > {
        let url = server.config.url.as_deref().ok_or_else(|| {
            AppError::Config(format!("MCP server '{}' has no HTTP url", server.name))
        })?;
        let mut config =
            rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig::with_uri(
                url,
            );
        let mut custom_headers = std::collections::HashMap::new();
        let mut bearer = None;
        for (name, value) in &server.config.headers {
            if name.eq_ignore_ascii_case("authorization") {
                let token = value
                    .strip_prefix("Bearer ")
                    .or_else(|| value.strip_prefix("bearer "))
                    .ok_or_else(|| {
                        AppError::Config(format!(
                            "MCP server '{}' authorization header must use Bearer",
                            server.name
                        ))
                    })?;
                bearer = Some(token.to_string());
                continue;
            }
            let header_name = reqwest::header::HeaderName::try_from(name).map_err(|error| {
                AppError::Config(format!(
                    "MCP server '{}' has invalid header name: {error}",
                    server.name
                ))
            })?;
            let header_value = reqwest::header::HeaderValue::try_from(value).map_err(|error| {
                AppError::Config(format!(
                    "MCP server '{}' has invalid header value: {error}",
                    server.name
                ))
            })?;
            custom_headers.insert(header_name, header_value);
        }
        if let Some(token) = bearer {
            config = config.auth_header(token);
        }
        Ok(config
            .custom_headers(custom_headers)
            .reinit_on_expired_session(true))
    }
}

pub(crate) fn http_client_for(url: &str) -> Result<reqwest::Client, AppError> {
    let parsed = reqwest::Url::parse(url)
        .map_err(|error| AppError::Config(format!("invalid HTTP MCP URL: {error}")))?;
    let mut builder = reqwest::Client::builder();
    if parsed
        .host_str()
        .is_some_and(|host| host == "localhost" || host == "127.0.0.1" || host == "::1")
    {
        builder = builder.no_proxy();
    }
    builder
        .timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| AppError::Tool(format!("build HTTP MCP client: {error}")))
}

#[async_trait]
impl McpTransport for HttpTransport {
    async fn connect(&self, server: &ConfiguredMcpServer) -> Result<McpClient, AppError> {
        let url = server.config.url.as_deref().ok_or_else(|| {
            AppError::Config(format!("MCP server '{}' has no HTTP url", server.name))
        })?;
        let client = http_client_for(url)?;
        let mut config = self.config(server)?;
        let stored_token = self.oauth_store.load(&server.name)?;
        let stored_identity_matches = if let Some(token) = stored_token.as_ref() {
            if token.mcp_url.as_deref().or(token.resource.as_deref()) != Some(url) {
                false
            } else {
                match server.config.auth.as_deref() {
                    Some("bearer") => {
                        token.client_id == "static-bearer"
                            && !server
                                .config
                                .headers
                                .keys()
                                .any(|key| key.eq_ignore_ascii_case("authorization"))
                    }
                    Some("oauth") | None => {
                        if server
                            .config
                            .headers
                            .keys()
                            .any(|key| key.eq_ignore_ascii_case("authorization"))
                        {
                            false
                        } else {
                            let client_id_matches = server
                                .config
                                .oauth
                                .as_ref()
                                .and_then(|oauth| oauth.client_id.as_deref())
                                .is_none_or(|client_id| client_id == token.client_id);
                            let oauth_identity_matches = server.config.oauth.as_ref().map_or_else(
                                || token.client_metadata_url.is_none() && token.scopes.is_empty(),
                                |oauth| {
                                    token.client_metadata_url == oauth.client_metadata_url
                                        && token.scopes == oauth.scopes
                                },
                            );
                            let discovery_matches =
                                match OAuthDiscovery::discover(&client, url).await {
                                    Ok(discovery) => {
                                        token.token_endpoint
                                            == discovery.authorization_server.token_endpoint
                                            && token.issuer.as_deref()
                                                == discovery.authorization_server.issuer.as_deref()
                                    }
                                    // A live discovery lookup is a safety re-check, not a
                                    // prerequisite for an already usable access token. Never
                                    // refresh against a possibly migrated issuer without that
                                    // re-check, but do let an unexpired token reach its MCP
                                    // server during a transient discovery outage.
                                    Err(_) => token.access_token_is_valid(),
                                };
                            client_id_matches && oauth_identity_matches && discovery_matches
                        }
                    }
                    _ => false,
                }
            }
        } else {
            false
        };
        let can_use_stored_token = match server.config.auth.as_deref() {
            Some("none") => false,
            Some("bearer") | Some("oauth") | None => stored_identity_matches,
            _ => false,
        };
        let token = if can_use_stored_token {
            self.oauth_store
                .refresh_if_needed(&client, &server.name)
                .await?
        } else {
            None
        };
        if let Some(token) = token.clone() {
            config = config.auth_header(token);
        } else if token.is_none() && server.config.auth.as_deref() != Some("bearer") {
            let mut probe = client.get(url);
            for (name, value) in &config.custom_headers {
                probe = probe.header(name, value);
            }
            if let Some(auth_header) = config.auth_header.clone() {
                probe = probe.bearer_auth(auth_header);
            }
            let response = probe.send().await.map_err(|error| {
                AppError::Tool(format!("probe HTTP MCP authorization: {error}"))
            })?;
            if response.status() == reqwest::StatusCode::UNAUTHORIZED
                && server.config.auth.as_deref() != Some("none")
                || server.config.auth.as_deref() == Some("oauth")
            {
                let _ = OAuthDiscovery::discover(&client, url).await?;
                return Err(AppError::Tool(format!(
                    "authorization required for HTTP MCP server '{}'; run /connector login {}",
                    server.name, server.name
                )));
            }
        }
        let transport = rmcp::transport::StreamableHttpClientTransport::from_config(config);
        match ().serve(transport).await {
            Ok(client) => Ok(client),
            Err(error) => {
                if token.is_none()
                    && server.config.auth.as_deref() != Some("bearer")
                    && OAuthDiscovery::discover(&client, url).await.is_ok()
                {
                    return Err(AppError::Tool(format!(
                        "authorization required for HTTP MCP server '{}'; run /connector login {}",
                        server.name, server.name
                    )));
                }
                if token.is_some() && server.config.auth.as_deref() != Some("bearer") {
                    if let Some(refreshed) = self
                        .oauth_store
                        .force_refresh(&client, &server.name)
                        .await?
                    {
                        let retry_config = self.config(server)?.auth_header(refreshed);
                        let retry_transport =
                            rmcp::transport::StreamableHttpClientTransport::from_config(
                                retry_config,
                            );
                        return ().serve(retry_transport).await.map_err(|retry_error| {
                            AppError::Tool(format!(
                                "initialize HTTP MCP server '{}' after token refresh: {retry_error}",
                                server.name
                            ))
                        });
                    }
                }
                if token.is_some()
                    && server.config.auth.as_deref() != Some("bearer")
                    && OAuthDiscovery::discover(&client, url).await.is_ok()
                {
                    return Err(AppError::Tool(format!(
                        "authorization required for HTTP MCP server '{}'; run /connector login {}",
                        server.name, server.name
                    )));
                }
                Err(AppError::Tool(format!(
                    "initialize HTTP MCP server '{}': {error}",
                    server.name
                )))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::StdioTransport;
    use crate::core::connector::mcp::config::{
        ConfiguredMcpServer, McpConfigSource, McpServerConfig, ToolFilter,
    };

    #[test]
    fn configured_env_is_added_after_environment_is_cleared() {
        let transport = StdioTransport::new(Some(Path::new("/workspace")));
        let server = ConfiguredMcpServer {
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
                trusted: false,
                integrity: None,
                startup_timeout_ms: 30_000,
                call_timeout_ms: 120_000,
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
                trusted: false,
                integrity: None,
                startup_timeout_ms: 30_000,
                call_timeout_ms: 120_000,
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
                trusted: false,
                integrity: None,
                startup_timeout_ms: 30_000,
                call_timeout_ms: 120_000,
                tool_filter: ToolFilter::default(),
            },
        };
        let config = transport.config(&server).expect("HTTP config");
        assert_eq!(config.auth_header.as_deref(), Some("static-token"));
        assert_eq!(config.custom_headers.len(), 1);
        assert_eq!(config.custom_headers.values().next().unwrap(), "ok");
    }
}
