use std::path::{Path, PathBuf};

use async_trait::async_trait;
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use rmcp::ServiceExt;

use super::failure::FailureKind;
pub use super::failure::McpFailure;
use crate::core::connector::mcp::config::ConfiguredMcpServer;
use crate::core::connector::mcp::oauth::{OAuthDiscovery, OAuthTokenStore};
use crate::infra::error::AppError;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

pub type McpClient = rmcp::service::RunningService<rmcp::RoleClient, ()>;

/// Bound cancellation delivery, including time queued inside rmcp. Dropping the
/// *inner send future* releases rmcp's per-request lifetime guard; merely timing
/// out RequestHandle::cancel would leave the independently owned send running.
/// Ordinary requests and progress-driven response waiting are not timed here.
pub struct CancellationBoundedTransport<T: rmcp::transport::Transport<rmcp::RoleClient> + 'static> {
    inner: Option<T>,
    cleanup: Option<TaskTracker>,
}

impl<T: rmcp::transport::Transport<rmcp::RoleClient> + 'static> CancellationBoundedTransport<T> {
    pub fn new(inner: T) -> Self {
        Self {
            inner: Some(inner),
            cleanup: None,
        }
    }
    fn managed(inner: T, cleanup: TaskTracker) -> Self {
        Self {
            inner: Some(inner),
            cleanup: Some(cleanup),
        }
    }
}

impl<T: rmcp::transport::Transport<rmcp::RoleClient> + 'static> Drop
    for CancellationBoundedTransport<T>
{
    fn drop(&mut self) {
        let Some(mut inner) = self.inner.take() else {
            return;
        };
        // Initialization can fail before a RunningService exists. Keep the
        // actual transport close/join owned, including stdio's kill AND wait.
        let receipt = self.cleanup.as_ref().map(TaskTracker::token);
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _receipt = receipt;
                let _ = inner.close().await;
            });
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CancellationTransportError<E: std::error::Error + 'static> {
    #[error("MCP transport operation failed")]
    Inner(#[source] E),
    #[error("MCP cancellation delivery timed out; remote completion is unknown")]
    DeliveryTimeout,
    #[error("MCP request was cancelled before transport submission")]
    NotSubmitted,
}

impl<T: rmcp::transport::Transport<rmcp::RoleClient> + 'static>
    rmcp::transport::Transport<rmcp::RoleClient> for CancellationBoundedTransport<T>
{
    type Error = CancellationTransportError<T::Error>;

    fn send(
        &mut self,
        message: rmcp::model::ClientJsonRpcMessage,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send + 'static {
        let cancellation = matches!(&message, rmcp::model::ClientJsonRpcMessage::Notification(n)
            if matches!(n.notification, rmcp::model::ClientNotification::CancelledNotification(_)));
        // Call synchronously: WorkerTransport registers local cancellation here,
        // before its returned future can wait on the remote control queue.
        use rmcp::model::GetExtensions;
        let rejected = if let rmcp::model::ClientJsonRpcMessage::Request(request) = &message {
            request
                .request
                .extensions()
                .get::<std::sync::Arc<super::call::CallIo>>()
                .is_some_and(|scope| {
                    let rejected = scope.rejected.load(std::sync::atomic::Ordering::SeqCst);
                    if !rejected {
                        scope
                            .possibly_sent
                            .store(true, std::sync::atomic::Ordering::SeqCst);
                    }
                    rejected
                })
        } else {
            false
        };
        let send = if rejected {
            None
        } else {
            self.inner.as_mut().map(|inner| inner.send(message))
        };
        async move {
            let send = send.ok_or(CancellationTransportError::NotSubmitted)?;
            if cancellation {
                // Leave four seconds of K=5s for response tasks/streams to reap.
                tokio::time::timeout(std::time::Duration::from_secs(1), send)
                    .await
                    .map_err(|_| CancellationTransportError::DeliveryTimeout)?
                    .map_err(CancellationTransportError::Inner)
            } else {
                send.await.map_err(CancellationTransportError::Inner)
            }
        }
    }

    async fn receive(&mut self) -> Option<rmcp::model::ServerJsonRpcMessage> {
        match self.inner.as_mut() {
            Some(inner) => inner.receive().await,
            None => None,
        }
    }

    async fn close(&mut self) -> Result<(), Self::Error> {
        let Some(mut inner) = self.inner.take() else {
            return Ok(());
        };
        let _receipt = self.cleanup.as_ref().map(TaskTracker::token);
        // Never drop graceful_shutdown halfway through: the SDK has already
        // taken the child out of its Drop guard at that point. Its own close
        // waits up to 3s then kills/reaps; HTTP DELETE is separately bounded.
        inner
            .close()
            .await
            .map_err(CancellationTransportError::Inner)
    }
}

#[async_trait]
pub trait McpTransport: Send + Sync {
    async fn connect(&self, server: &ConfiguredMcpServer) -> Result<McpClient, AppError> {
        let cancel = CancellationToken::new();
        let guard = cancel.clone().drop_guard();
        let result = self
            .connect_cancellable(server, cancel, TaskTracker::new())
            .await;
        if result.is_ok() {
            guard.disarm();
        }
        result.map_err(|error| error.to_app_error(&server.name))
    }
    async fn connect_cancellable(
        &self,
        server: &ConfiguredMcpServer,
        cancel: CancellationToken,
        cleanup: TaskTracker,
    ) -> Result<McpClient, McpFailure>;
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
            command.kill_on_drop(true);
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
    async fn connect_cancellable(
        &self,
        server: &ConfiguredMcpServer,
        cancel: CancellationToken,
        cleanup: TaskTracker,
    ) -> Result<McpClient, McpFailure> {
        if cancel.is_cancelled() {
            return Err(McpFailure::new(FailureKind::Cancelled, "spawn"));
        }
        let transport = TokioChildProcess::new(self.command(server))
            .map_err(|_| McpFailure::new(FailureKind::Configuration, "spawn"))?;
        ().serve_with_ct(
            CancellationBoundedTransport::managed(transport, cleanup),
            cancel,
        )
        .await
        .map_err(|error| McpFailure::from_initialization(&error))
    }
}

/// Client-side Streamable HTTP transport backed by rmcp's reqwest adapter.
///
/// Authentication is deliberately kept at the transport boundary: ordinary
/// headers are copied to every MCP request, while an Authorization bearer value
/// is passed through rmcp's dedicated auth slot so it cannot be rejected as a
/// reserved custom header. OAuth token acquisition/refresh will use the same
/// slot once the connector OAuth store is wired in.
#[derive(Clone)]
pub struct HttpTransport {
    oauth_store: OAuthTokenStore,
    refresh: Option<McpFailure>,
    refresh_used: std::sync::Arc<std::sync::atomic::AtomicBool>,
    token_commit: Option<super::oauth::TokenCommit>,
    max_concurrent_requests: usize,
}

impl std::fmt::Debug for HttpTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HttpTransport")
    }
}

impl HttpTransport {
    pub fn new(oauth_store: OAuthTokenStore) -> Self {
        Self {
            oauth_store,
            refresh: None,
            refresh_used: Default::default(),
            token_commit: None,
            max_concurrent_requests: crate::infra::config::DEFAULT_MCP_MAX_CONCURRENT_CALLS,
        }
    }

    pub(crate) fn with_refresh(
        mut self,
        failure: Option<McpFailure>,
        used: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        self.refresh = failure;
        self.refresh_used = used;
        self
    }
    pub(crate) fn with_max_concurrent_requests(mut self, limit: usize) -> Self {
        self.max_concurrent_requests = limit;
        self
    }

    pub(crate) fn with_token_commit(mut self, commit: super::oauth::TokenCommit) -> Self {
        self.token_commit = Some(commit);
        self
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
            .max_concurrent_requests(self.max_concurrent_requests)
            .reinit_on_expired_session(false))
    }
}

pub(crate) fn http_client_for(url: &str) -> Result<reqwest::Client, AppError> {
    http_client_builder(url)?
        .timeout(std::time::Duration::from_secs(30))
        .retry(reqwest::retry::never())
        .build()
        .map_err(|_| AppError::Tool("build HTTP MCP authorization client failed".into()))
}

fn mcp_http_client_for(url: &str) -> Result<reqwest::Client, AppError> {
    http_client_builder(url)?
        .connect_timeout(std::time::Duration::from_secs(30))
        .retry(super::scoped_http::no_replay_policy())
        .build()
        .map_err(|_| AppError::Tool("build HTTP MCP protocol client failed".into()))
}

fn http_client_builder(url: &str) -> Result<reqwest::ClientBuilder, AppError> {
    let parsed = reqwest::Url::parse(url)
        .map_err(|error| AppError::Config(format!("invalid HTTP MCP URL: {error}")))?;
    let mut builder = reqwest::Client::builder();
    if parsed
        .host_str()
        .is_some_and(|host| host == "localhost" || host == "127.0.0.1" || host == "::1")
    {
        builder = builder.no_proxy();
    }
    Ok(builder.redirect(reqwest::redirect::Policy::none()))
}

impl HttpTransport {
    async fn prepare(
        &self,
        server: &ConfiguredMcpServer,
    ) -> Result<
        (
            super::scoped_http::ScopedHttpClient,
            rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig,
        ),
        McpFailure,
    > {
        let url = server.config.url.as_deref().ok_or_else(|| {
            AppError::Config(format!("MCP server '{}' has no HTTP url", server.name))
        })?;
        let client = http_client_for(url)?;
        let mut config = self.config(server)?;
        let stored_snapshot = self.oauth_store.snapshot(&server.config_key)?;
        let stored_token = stored_snapshot.token.clone();
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
                            let discovery_matches = match OAuthDiscovery::discover_for_connection(
                                &client,
                                url,
                                self.refresh
                                    .as_ref()
                                    .and_then(|failure| failure.challenge.as_deref()),
                            )
                            .await
                            {
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
                                Err(_) => self.refresh.is_none() && token.access_token_is_valid(),
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
        let token = if let Some(token) = stored_token.filter(|_| can_use_stored_token) {
            if self.refresh.is_none() && token.access_token_is_valid() {
                Some(token.access_token)
            } else if token.refresh_token.is_some()
                && server.config.auth.as_deref() != Some("bearer")
                && !self
                    .refresh_used
                    .swap(true, std::sync::atomic::Ordering::SeqCst)
            {
                // Refresh the identity-checked snapshot, never reload a possibly
                // replaced credential after discovery. No extra SDK handshake.
                self.oauth_store
                    .refresh_checked(
                        &client,
                        &server.config_key,
                        stored_snapshot,
                        self.token_commit.as_ref(),
                    )
                    .await
                    .map_err(|_| {
                        McpFailure::new(FailureKind::Authorization, "credential refresh")
                    })?
            } else {
                None
            }
        } else {
            None
        };
        if let Some(token) = token {
            config = config.auth_header(token);
        } else if self.refresh.is_some() || server.config.auth.as_deref() == Some("oauth") {
            return Err(McpFailure::new(FailureKind::Authorization, "prepare"));
        }
        Ok((
            super::scoped_http::ScopedHttpClient::new(mcp_http_client_for(url)?),
            config,
        ))
    }
}

#[async_trait]
impl McpTransport for HttpTransport {
    async fn connect_cancellable(
        &self,
        server: &ConfiguredMcpServer,
        cancel: CancellationToken,
        cleanup: TaskTracker,
    ) -> Result<McpClient, McpFailure> {
        let (client, config) = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(McpFailure::new(FailureKind::Cancelled, "prepare")),
            result = self.prepare(server) => result?,
        };
        let transport = rmcp::transport::StreamableHttpClientTransport::with_client(
            client.with_lifetime(cancel.clone()),
            config,
        );
        // Exactly one SDK startup. A refresh followed by another handshake
        // must consume another manager-owned attempt, never a hidden retry.
        ().serve_with_ct(
            CancellationBoundedTransport::managed(transport, cleanup),
            cancel,
        )
        .await
        .map_err(|error| McpFailure::from_initialization(&error))
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
}
