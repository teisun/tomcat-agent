//! OAuth discovery and token persistence for remote MCP servers.
//!
//! The MCP URL is the only input needed to start discovery. The server tells us
//! which protected-resource metadata to read, and that metadata tells us which
//! authorization server to use. No provider-specific login URL is hard-coded.

use parking_lot::Mutex;
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use fs2::FileExt;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::core::connector::mcp::config::McpOAuthConfig;
use crate::infra::config::get_work_dir;
use crate::infra::error::AppError;
use crate::AppConfig;

const TOKEN_FILE_NAME: &str = "connector-oauth.json";
const TOKEN_EXPIRY_SKEW_SECS: u64 = 60;

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StoredOAuthToken {
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    pub token_endpoint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issuer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_metadata_url: Option<String>,
    #[serde(default)]
    pub scopes: Vec<String>,
    pub client_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
}

impl std::fmt::Debug for StoredOAuthToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StoredOAuthToken")
            .field("access_token", &"[REDACTED]")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field("expires_at", &self.expires_at)
            .field("token_endpoint", &self.token_endpoint)
            .field("issuer", &self.issuer)
            .field("client_id", &self.client_id)
            .field(
                "client_secret",
                &self.client_secret.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
}

impl StoredOAuthToken {
    pub(crate) fn access_token_is_valid(&self) -> bool {
        self.expires_at
            .is_none_or(|expires| expires > now_secs() + TOKEN_EXPIRY_SKEW_SECS)
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct TokenFile {
    #[serde(default)]
    servers: BTreeMap<String, StoredOAuthToken>,
    #[serde(default)]
    revisions: BTreeMap<String, u64>,
}
#[derive(Debug, Clone)]
pub struct OAuthTokenStore {
    path: std::path::PathBuf,
    lock: Arc<Mutex<()>>,
    lock_path: std::path::PathBuf,
}

impl TokenFile {
    fn bump(&mut self, key: &str) -> Result<(), AppError> {
        let revision = self.revisions.entry(key.to_owned()).or_default();
        *revision = revision
            .checked_add(1)
            .ok_or_else(|| AppError::Config("OAuth credential revision exhausted".into()))?;
        Ok(())
    }
}

#[derive(Clone)]
pub(crate) struct CredentialSnapshot {
    pub token: Option<StoredOAuthToken>,
    revision: u64,
}

/// Commit capability supplied by the source owner. The caller holds its
/// permission/generation boundary while the store performs credential CAS.
pub(crate) type TokenCommit =
    Arc<dyn Fn(&CredentialSnapshot, StoredOAuthToken) -> Result<bool, AppError> + Send + Sync>;

impl OAuthTokenStore {
    pub fn open(cfg: &AppConfig) -> Result<Self, AppError> {
        let path = get_work_dir(cfg)?.join(TOKEN_FILE_NAME);
        if path.exists() {
            set_private_file_permissions(&path)?;
        }
        Ok(Self {
            path: path.clone(),
            lock_path: path.with_extension("lock"),
            lock: Arc::new(Mutex::new(())),
        })
    }
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    pub fn load(&self, server_name: &str) -> Result<Option<StoredOAuthToken>, AppError> {
        Ok(self.snapshot(server_name)?.token)
    }

    pub(crate) fn snapshot(&self, server_name: &str) -> Result<CredentialSnapshot, AppError> {
        let _guard = self.lock.lock();
        let _file_guard = self.acquire_file_lock()?;
        let file = self.read_file()?;
        Ok(CredentialSnapshot {
            token: file.servers.get(server_name).cloned(),
            revision: file.revisions.get(server_name).copied().unwrap_or_default(),
        })
    }

    pub fn save(&self, server_name: &str, token: StoredOAuthToken) -> Result<(), AppError> {
        let _guard = self.lock.lock();
        let _file_guard = self.acquire_file_lock()?;
        let mut file = self.read_file()?;
        file.bump(server_name)?;
        file.servers.insert(server_name.to_string(), token);
        self.write_file(&file)
    }

    #[cfg(test)]
    fn save_if_current(
        &self,
        server_name: &str,
        expected: &StoredOAuthToken,
        updated: StoredOAuthToken,
    ) -> Result<bool, AppError> {
        let snapshot = self.snapshot(server_name)?;
        if snapshot.token.as_ref() != Some(expected) {
            return Ok(false);
        }
        self.save_if_snapshot_current(server_name, &snapshot, updated)
    }

    pub(crate) fn save_if_snapshot_current(
        &self,
        server_name: &str,
        expected: &CredentialSnapshot,
        updated: StoredOAuthToken,
    ) -> Result<bool, AppError> {
        let _guard = self.lock.lock();
        let _file_guard = self.acquire_file_lock()?;
        let mut file = self.read_file()?;
        if file.servers.get(server_name) != expected.token.as_ref()
            || file.revisions.get(server_name).copied().unwrap_or_default() != expected.revision
        {
            return Ok(false);
        }
        file.bump(server_name)?;
        file.servers.insert(server_name.to_string(), updated);
        self.write_file(&file)?;
        Ok(true)
    }

    pub fn save_static_bearer(
        &self,
        server_name: &str,
        access_token: String,
        resource: Option<String>,
    ) -> Result<(), AppError> {
        self.save(
            server_name,
            StoredOAuthToken {
                access_token,
                refresh_token: None,
                expires_at: None,
                token_endpoint: String::new(),
                issuer: None,
                resource: resource.clone(),
                mcp_url: resource,
                client_metadata_url: None,
                scopes: Vec::new(),
                client_id: "static-bearer".to_string(),
                client_secret: None,
            },
        )
    }

    pub fn remove(&self, server_name: &str) -> Result<bool, AppError> {
        let _guard = self.lock.lock();
        let _file_guard = self.acquire_file_lock()?;
        let mut file = self.read_file()?;
        let removed = file.servers.remove(server_name).is_some();
        file.bump(server_name)?;
        self.write_file(&file)?;
        Ok(removed)
    }
    pub fn valid_access_token(&self, server_name: &str) -> Result<Option<String>, AppError> {
        let Some(token) = self.load(server_name)? else {
            return Ok(None);
        };
        if !token.access_token_is_valid() {
            return Ok(None);
        }
        Ok(Some(token.access_token))
    }

    pub async fn refresh_if_needed(
        &self,
        client: &reqwest::Client,
        server_name: &str,
    ) -> Result<Option<String>, AppError> {
        let snapshot = self.snapshot(server_name)?;
        let Some(token) = snapshot.token.as_ref() else {
            return Ok(None);
        };
        if token.access_token_is_valid() {
            return Ok(Some(token.access_token.clone()));
        }
        if token.refresh_token.is_none() {
            return Ok(None);
        }
        self.refresh_checked(client, server_name, snapshot, None)
            .await
    }

    pub async fn force_refresh(
        &self,
        client: &reqwest::Client,
        server_name: &str,
    ) -> Result<Option<String>, AppError> {
        let snapshot = self.snapshot(server_name)?;
        if snapshot
            .token
            .as_ref()
            .is_none_or(|token| token.refresh_token.is_none())
        {
            return Ok(None);
        }
        self.refresh_checked(client, server_name, snapshot, None)
            .await
    }

    pub(crate) async fn refresh_checked(
        &self,
        client: &reqwest::Client,
        server_name: &str,
        snapshot: CredentialSnapshot,
        commit: Option<&TokenCommit>,
    ) -> Result<Option<String>, AppError> {
        let Some(token) = snapshot.token.as_ref() else {
            return Ok(None);
        };
        let refresh_token = token
            .refresh_token
            .clone()
            .expect("refresh token checked above");
        let mut form = vec![
            ("grant_type", "refresh_token".to_string()),
            ("refresh_token", refresh_token),
            ("client_id", token.client_id.clone()),
        ];
        if let Some(resource) = token.resource.clone() {
            form.push(("resource", resource));
        }
        if let Some(secret) = token.client_secret.clone() {
            form.push(("client_secret", secret));
        }
        let response = client
            .post(&token.token_endpoint)
            .form(&form)
            .send()
            .await
            .map_err(|error| AppError::Tool(format!("OAuth token refresh failed: {error}")))?;
        if !response.status().is_success() {
            return Ok(None);
        }
        let refreshed: TokenResponse = response.json().await.map_err(|error| {
            AppError::Tool(format!(
                "OAuth token refresh returned invalid JSON: {error}"
            ))
        })?;
        let updated = StoredOAuthToken {
            access_token: refreshed.access_token.clone(),
            refresh_token: refreshed.refresh_token.or(token.refresh_token.clone()),
            expires_at: refreshed
                .expires_in
                .map(|secs| now_secs().saturating_add(secs)),
            token_endpoint: token.token_endpoint.clone(),
            issuer: token.issuer.clone(),
            resource: token.resource.clone(),
            mcp_url: token.mcp_url.clone(),
            client_metadata_url: token.client_metadata_url.clone(),
            scopes: token.scopes.clone(),
            client_id: token.client_id.clone(),
            client_secret: token.client_secret.clone(),
        };
        // The refresh runs outside the file lock. Commit only if the exact
        // credential that started it is still current: logout, removal, or a
        // newer login wins over this late network response.
        let saved = match commit {
            Some(commit) => commit(&snapshot, updated)?,
            None => self.save_if_snapshot_current(server_name, &snapshot, updated)?,
        };
        if !saved {
            return Ok(None);
        }
        Ok(Some(refreshed.access_token))
    }
    fn acquire_file_lock(&self) -> Result<File, AppError> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&self.lock_path)?;
        file.lock_exclusive()?;
        Ok(file)
    }

    fn read_file(&self) -> Result<TokenFile, AppError> {
        if !self.path.exists() {
            return Ok(TokenFile::default());
        }
        let content = std::fs::read_to_string(&self.path)?;
        serde_json::from_str(&content).map_err(|error| {
            AppError::Config(format!(
                "parse OAuth token store '{}': {error}",
                self.path.display()
            ))
        })
    }
    fn write_file(&self, file: &TokenFile) -> Result<(), AppError> {
        let contents = serde_json::to_vec_pretty(file)
            .map_err(|error| AppError::Config(format!("serialize OAuth token store: {error}")))?;
        let parent = self
            .path
            .parent()
            .ok_or_else(|| AppError::Config("OAuth token store has no parent".to_string()))?;
        std::fs::create_dir_all(parent)?;
        let temp_path = parent.join(format!(
            ".{}.{}",
            self.path.file_name().unwrap_or_default().to_string_lossy(),
            uuid::Uuid::new_v4()
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut output = options.open(&temp_path)?;
        set_private_file_permissions(&temp_path)?;
        output.write_all(&contents)?;
        output.sync_all()?;
        std::fs::rename(&temp_path, &self.path)?;
        set_private_file_permissions(&self.path)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ProtectedResourceMetadata {
    #[serde(default)]
    pub resource: Option<String>,
    #[serde(default)]
    pub authorization_servers: Vec<String>,
    #[serde(default)]
    pub scopes_supported: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AuthorizationServerMetadata {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    #[serde(default)]
    pub registration_endpoint: Option<String>,
    #[serde(default)]
    pub issuer: Option<String>,
    #[serde(default)]
    pub scopes_supported: Vec<String>,
    #[serde(default)]
    pub code_challenge_methods_supported: Vec<String>,
    #[serde(flatten)]
    pub additional: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct OAuthDiscovery {
    pub protected_resource: ProtectedResourceMetadata,
    pub authorization_server: AuthorizationServerMetadata,
    pub metadata_url: Url,
}

impl OAuthDiscovery {
    pub async fn discover(client: &reqwest::Client, mcp_url: &str) -> Result<Self, AppError> {
        Self::discover_inner(client, mcp_url, None, true).await
    }

    /// Background connection preparation must not perform an uncounted MCP
    /// initialize POST. Reuse a real handshake's challenge when available.
    pub(crate) async fn discover_for_connection(
        client: &reqwest::Client,
        mcp_url: &str,
        challenge: Option<&str>,
    ) -> Result<Self, AppError> {
        Self::discover_inner(client, mcp_url, challenge, false).await
    }

    async fn discover_inner(
        client: &reqwest::Client,
        mcp_url: &str,
        supplied_challenge: Option<&str>,
        allow_initialize_probe: bool,
    ) -> Result<Self, AppError> {
        let base = Url::parse(mcp_url)
            .map_err(|error| AppError::Config(format!("invalid MCP URL: {error}")))?;
        let challenge = if let Some(challenge) = supplied_challenge {
            Some(challenge.to_owned())
        } else {
            let response = client
                .get(base.clone())
                .send()
                .await
                .map_err(|_| AppError::Tool("MCP OAuth discovery request failed".into()))?;
            response
                .headers()
                .get(reqwest::header::WWW_AUTHENTICATE)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned)
        };
        let challenge = if challenge.is_some() || !allow_initialize_probe {
            challenge
        } else {
            let response = client
                .post(base.clone())
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .header(reqwest::header::ACCEPT, "application/json, text/event-stream")
                .json(&serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 0,
                    "method": "initialize",
                    "params": {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "Tomcat", "version": env!("CARGO_PKG_VERSION")}}
                }))
                .send()
                .await
                .map_err(|error| AppError::Tool(format!("MCP OAuth POST discovery request failed: {error}")))?;
            response
                .headers()
                .get(reqwest::header::WWW_AUTHENTICATE)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned)
        };
        let resource_url = if let Some(value) = challenge
            .as_deref()
            .and_then(|value| extract_parameter(value, "resource_metadata"))
        {
            base.join(&value).ok()
        } else {
            let mut found = None;
            for candidate in well_known_candidates(&base, "oauth-protected-resource") {
                let response = client
                    .get(candidate.clone())
                    .send()
                    .await
                    .map_err(|error| {
                        AppError::Tool(format!("fetch MCP protected-resource metadata: {error}"))
                    })?;
                if response.status().is_success() {
                    found = Some(candidate);
                    break;
                }
            }
            found
        };
        let resource_url = resource_url.ok_or_else(|| {
            AppError::Tool("MCP server did not advertise protected-resource metadata".into())
        })?;
        let resource_response = client.get(resource_url).send().await.map_err(|error| {
            AppError::Tool(format!("fetch MCP protected-resource metadata: {error}"))
        })?;
        if !resource_response.status().is_success() {
            return Err(AppError::Tool(format!(
                "protected-resource metadata returned HTTP {}",
                resource_response.status()
            )));
        }
        let protected: ProtectedResourceMetadata =
            resource_response.json().await.map_err(|error| {
                AppError::Tool(format!("parse protected-resource metadata: {error}"))
            })?;
        let issuer = protected.authorization_servers.first().ok_or_else(|| {
            AppError::Tool("protected-resource metadata has no authorization_servers".to_string())
        })?;
        let issuer_url = Url::parse(issuer).map_err(|error| {
            AppError::Tool(format!("invalid authorization server URL: {error}"))
        })?;
        let mut candidates = well_known_candidates(&issuer_url, "oauth-authorization-server");
        candidates.extend(well_known_candidates(&issuer_url, "openid-configuration"));
        let mut last_status = None;
        for metadata_url in candidates {
            let response = client
                .get(metadata_url.clone())
                .send()
                .await
                .map_err(|error| {
                    AppError::Tool(format!("fetch OAuth authorization metadata: {error}"))
                })?;
            last_status = Some(response.status());
            if response.status().is_success() {
                let metadata = response.json().await.map_err(|error| {
                    AppError::Tool(format!("parse OAuth authorization metadata: {error}"))
                })?;
                return Ok(Self {
                    protected_resource: protected,
                    authorization_server: metadata,
                    metadata_url,
                });
            }
        }
        Err(AppError::Tool(format!(
            "authorization server metadata unavailable{}",
            last_status.map_or(String::new(), |status| format!(" (last HTTP {status})"))
        )))
    }
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
}

/// Complete an Authorization Code + PKCE login. Tests pass `open_browser=false`
/// and drive the fake authorization endpoint themselves.
pub async fn authorize(
    client: &reqwest::Client,
    store: &OAuthTokenStore,
    server_name: &str,
    mcp_url: &str,
    oauth: &McpOAuthConfig,
    open_browser: bool,
) -> Result<StoredOAuthToken, AppError> {
    let stored = authorize_token(client, server_name, mcp_url, oauth, open_browser).await?;
    store.save(server_name, stored.clone())?;
    Ok(stored)
}

/// Obtain a credential without writing it. The source owner commits only after
/// rechecking its generation, login nonce, permission and credential revision.
pub(crate) async fn authorize_token(
    client: &reqwest::Client,
    server_name: &str,
    mcp_url: &str,
    oauth: &McpOAuthConfig,
    open_browser: bool,
) -> Result<StoredOAuthToken, AppError> {
    let discovery = OAuthDiscovery::discover(client, mcp_url).await?;
    let configured_redirect_uri = oauth.callback_url.clone();
    let callback = if let Some(redirect_uri) = configured_redirect_uri.as_deref() {
        crate::core::connector::mcp::oauth_callback::OAuthCallbackListener::bind_for_redirect(
            redirect_uri,
        )
        .await?
    } else {
        crate::core::connector::mcp::oauth_callback::OAuthCallbackListener::bind().await?
    };
    let redirect_uri = if let Some(redirect_uri) = configured_redirect_uri {
        let parsed = Url::parse(&redirect_uri)
            .map_err(|error| AppError::Tool(format!("invalid OAuth callback URL: {error}")))?;
        if parsed.port() == Some(0) {
            callback.redirect_uri()?
        } else {
            redirect_uri
        }
    } else {
        callback.redirect_uri()?
    };
    let (verifier, challenge) = pkce_pair()?;
    let mut resource = discovery.protected_resource.resource.clone();
    let mut state_bytes = [0_u8; 24];
    getrandom::fill(&mut state_bytes)
        .map_err(|error| AppError::Tool(format!("generate OAuth state: {error}")))?;
    let state = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(state_bytes);

    let supports_cimd = discovery
        .authorization_server
        .additional
        .get("client_id_metadata_document_supported")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let (client_id, client_secret) = if let Some(client_id) = oauth.client_id.clone() {
        (
            client_id,
            oauth
                .client_secret_env
                .as_deref()
                .and_then(|name| std::env::var(name).ok()),
        )
    } else if let Some(metadata_url) = oauth.client_metadata_url.clone() {
        let parsed = Url::parse(&metadata_url).map_err(|error| {
            AppError::Tool(format!("invalid OAuth client metadata URL: {error}"))
        })?;
        if !supports_cimd
            || parsed.scheme() != "https"
            || parsed.host_str().is_none()
            || parsed.path() == "/"
        {
            return Err(AppError::Tool(
                "OAuth client metadata URL requires advertised HTTPS CIMD support".to_string(),
            ));
        }
        (metadata_url, None)
    } else if let Some(endpoint) = discovery
        .authorization_server
        .registration_endpoint
        .as_deref()
    {
        let response = client
            .post(endpoint)
            .json(&serde_json::json!({
                "client_name": "Tomcat",
                "redirect_uris": [redirect_uri],
                "grant_types": ["authorization_code", "refresh_token"],
                "response_types": ["code"],
                "token_endpoint_auth_method": "none",
                "application_type": "native"
            }))
            .send()
            .await
            .map_err(|error| {
                AppError::Tool(format!("OAuth client registration failed: {error}"))
            })?;
        if !response.status().is_success() {
            return Err(AppError::Tool(format!(
                "OAuth client registration returned HTTP {}",
                response.status()
            )));
        }
        let response: serde_json::Value = response
            .json()
            .await
            .map_err(|error| AppError::Tool(format!("parse OAuth client registration: {error}")))?;
        let client_id = response
            .get("client_id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| AppError::Tool("OAuth registration omitted client_id".to_string()))?
            .to_string();
        (
            client_id,
            response
                .get("client_secret")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
        )
    } else {
        return Err(AppError::Tool(format!(
            "OAuth server '{server_name}' requires client_id or registration_endpoint"
        )));
    };

    let mut authorization_url = Url::parse(&discovery.authorization_server.authorization_endpoint)
        .map_err(|error| {
            AppError::Tool(format!("invalid OAuth authorization endpoint: {error}"))
        })?;
    {
        let mut query = authorization_url.query_pairs_mut();
        query.append_pair("response_type", "code");
        query.append_pair("client_id", &client_id);
        query.append_pair("redirect_uri", &redirect_uri);
        query.append_pair("state", &state);
        query.append_pair("code_challenge", &challenge);
        query.append_pair("code_challenge_method", "S256");
        if let Some(resource) = resource.as_deref() {
            query.append_pair("resource", resource);
        }
        let scopes = if oauth.scopes.is_empty() {
            &discovery.authorization_server.scopes_supported
        } else {
            &oauth.scopes
        };
        if !scopes.is_empty() {
            query.append_pair("scope", &scopes.join(" "));
        }
    }
    // Scoped futures, not a detached callback task. Dropping this authorization
    // future (logout, source replacement, cancellation) closes the listener.
    let callback = async {
        tokio::time::timeout(std::time::Duration::from_secs(240), callback.wait(&state))
            .await
            .map_err(|_| AppError::Tool("OAuth authorization timed out".into()))?
    };
    let navigate = async {
        if open_browser {
            open_url(authorization_url.as_str())?;
        } else {
            let response = client
                .get(authorization_url)
                .send()
                .await
                .map_err(|_| AppError::Tool("drive test OAuth authorization failed".into()))?;
            if !response.status().is_success() {
                return Err(AppError::Tool(format!(
                    "test OAuth authorization returned HTTP {}",
                    response.status()
                )));
            }
        }
        Ok::<_, AppError>(())
    };
    let (code, ()) = tokio::try_join!(callback, navigate)?;
    let mut form = vec![
        ("grant_type", "authorization_code".to_string()),
        ("code", code),
        ("state", state),
        ("redirect_uri", redirect_uri),
        ("client_id", client_id.clone()),
        ("code_verifier", verifier),
    ];
    if let Some(secret) = client_secret.clone() {
        form.push(("client_secret", secret));
    }
    if let Some(resource) = resource.take() {
        form.push(("resource", resource));
    }
    let response = client
        .post(&discovery.authorization_server.token_endpoint)
        .form(&form)
        .send()
        .await
        .map_err(|error| AppError::Tool(format!("OAuth token exchange failed: {error}")))?;
    if !response.status().is_success() {
        return Err(AppError::Tool(format!(
            "OAuth token exchange returned HTTP {}",
            response.status()
        )));
    }
    let token: TokenResponse = response
        .json()
        .await
        .map_err(|error| AppError::Tool(format!("parse OAuth token response: {error}")))?;
    let stored = StoredOAuthToken {
        access_token: token.access_token,
        refresh_token: token.refresh_token,
        expires_at: token.expires_in.map(|secs| now_secs().saturating_add(secs)),
        token_endpoint: discovery.authorization_server.token_endpoint,
        issuer: discovery.authorization_server.issuer,
        resource: discovery
            .protected_resource
            .resource
            .clone()
            .or_else(|| Some(mcp_url.to_string())),
        mcp_url: Some(mcp_url.to_string()),
        client_metadata_url: oauth.client_metadata_url.clone(),
        scopes: oauth.scopes.clone(),
        client_id,
        client_secret,
    };
    Ok(stored)
}
fn open_url(url: &str) -> Result<(), AppError> {
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    #[cfg(target_os = "linux")]
    let mut command = std::process::Command::new("xdg-open");
    #[cfg(target_os = "windows")]
    let mut command = std::process::Command::new("explorer.exe");
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    return Err(AppError::Tool(
        "opening the OAuth browser is unsupported on this platform".to_string(),
    ));
    command
        .arg(url)
        .spawn()
        .map_err(|error| AppError::Tool(format!("open OAuth authorization URL: {error}")))?;
    Ok(())
}

pub fn pkce_pair() -> Result<(String, String), AppError> {
    let mut verifier_bytes = [0_u8; 32];
    getrandom::fill(&mut verifier_bytes)
        .map_err(|error| AppError::Tool(format!("generate OAuth PKCE verifier: {error}")))?;
    let verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(verifier_bytes);
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(verifier.as_bytes()));
    Ok((verifier, challenge))
}

pub fn oauth_authorization_url(
    metadata: &AuthorizationServerMetadata,
    server_name: &str,
    oauth: &McpOAuthConfig,
    redirect_uri: &str,
    state: &str,
    challenge: &str,
) -> Result<Url, AppError> {
    let client_id = oauth.client_id.as_deref().ok_or_else(|| {
        AppError::Tool(format!(
            "OAuth server '{server_name}' requires a client_id or dynamic registration"
        ))
    })?;
    let mut url = Url::parse(&metadata.authorization_endpoint).map_err(|error| {
        AppError::Tool(format!("invalid OAuth authorization endpoint: {error}"))
    })?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("response_type", "code");
        query.append_pair("client_id", client_id);
        query.append_pair("redirect_uri", redirect_uri);
        query.append_pair("state", state);
        query.append_pair("code_challenge", challenge);
        query.append_pair("code_challenge_method", "S256");
        if !oauth.scopes.is_empty() {
            query.append_pair("scope", &oauth.scopes.join(" "));
        }
    }
    Ok(url)
}

fn well_known_candidates(base: &Url, resource: &str) -> Vec<Url> {
    let trimmed = base.path().trim_matches('/');
    let mut paths = Vec::new();
    if trimmed.is_empty() {
        paths.push(format!("/.well-known/{resource}"));
    } else {
        paths.push(format!("/.well-known/{resource}/{trimmed}"));
        paths.push(format!("/{trimmed}/.well-known/{resource}"));
        paths.push(format!("/.well-known/{resource}"));
    }
    paths
        .into_iter()
        .map(|path| {
            let mut url = base.clone();
            url.set_path(&path);
            url.set_query(None);
            url.set_fragment(None);
            url
        })
        .collect()
}

fn extract_parameter(header: &str, parameter: &str) -> Option<String> {
    let marker = format!("{parameter}=");
    let start = header.find(&marker)? + marker.len();
    let remainder = &header[start..];
    if let Some(value) = remainder.strip_prefix('"') {
        return value.split('"').next().map(str::to_owned);
    }
    Some(remainder.split(',').next()?.trim().to_string())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn set_private_file_permissions(path: &std::path::Path) -> Result<(), AppError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(windows)]
    {
        let username = std::env::var("USERNAME").map_err(|_| {
            AppError::Config("USERNAME is unavailable for OAuth token ACL setup".to_string())
        })?;
        let grant = format!("{username}:F");
        let status = std::process::Command::new("icacls")
            .args([
                path.as_os_str(),
                std::ffi::OsStr::new("/inheritance:r"),
                std::ffi::OsStr::new("/grant:r"),
                std::ffi::OsStr::new(&grant),
            ])
            .status()?;
        if !status.success() {
            return Err(AppError::Config(
                "icacls failed to protect OAuth token store".to_string(),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        pkce_pair, AuthorizationServerMetadata, OAuthTokenStore, ProtectedResourceMetadata,
    };
    use crate::core::connector::mcp::config::McpOAuthConfig;
    use crate::AppConfig;

    #[test]
    fn pkce_pair_has_distinct_verifier_and_challenge() {
        let (verifier, challenge) = pkce_pair().expect("pkce");
        assert!(!verifier.is_empty());
        assert!(!challenge.is_empty());
        assert_ne!(verifier, challenge);
    }

    #[test]
    fn metadata_shapes_are_deserializable() {
        let protected: ProtectedResourceMetadata = serde_json::from_value(serde_json::json!({
            "resource": "http://127.0.0.1/mcp",
            "authorization_servers": ["http://127.0.0.1:9000"],
            "scopes_supported": ["read"]
        }))
        .expect("protected metadata");
        assert_eq!(protected.authorization_servers.len(), 1);
        let metadata: AuthorizationServerMetadata = serde_json::from_value(serde_json::json!({
            "authorization_endpoint": "http://127.0.0.1/authorize",
            "token_endpoint": "http://127.0.0.1/token",
            "code_challenge_methods_supported": ["S256"]
        }))
        .expect("authorization metadata");
        assert_eq!(metadata.token_endpoint, "http://127.0.0.1/token");
    }

    #[test]
    fn credential_revision_rejects_identical_token_aba_and_empty_logout_across_stores() {
        let temp = tempfile::tempdir().unwrap();
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(temp.path().to_string_lossy().into_owned());
        let store = OAuthTokenStore::open(&cfg).unwrap();
        let other = OAuthTokenStore::open(&cfg).unwrap();
        let token: super::StoredOAuthToken = serde_json::from_value(serde_json::json!({"accessToken":"same-token", "tokenEndpoint":"http://127.0.0.1/token", "clientId":"fixture"})).unwrap();
        let empty = store.snapshot("empty").unwrap();
        assert!(!other.remove("empty").unwrap());
        assert!(!store
            .save_if_snapshot_current("empty", &empty, token.clone())
            .unwrap());
        store.save("server", token.clone()).unwrap();
        let before = store.snapshot("server").unwrap();
        other.remove("server").unwrap();
        other.save("server", token.clone()).unwrap();
        assert!(!store
            .save_if_snapshot_current("server", &before, token.clone())
            .unwrap());
        let current = store.snapshot("server").unwrap();
        assert!(store
            .save_if_snapshot_current("server", &current, token)
            .unwrap());
    }

    #[test]
    fn stale_refresh_cannot_restore_a_removed_or_replaced_token() {
        let temp = tempfile::tempdir().expect("temp");
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(temp.path().to_string_lossy().into_owned());
        let store = OAuthTokenStore::open(&cfg).expect("store");
        let original = super::StoredOAuthToken {
            access_token: "old-access".to_string(),
            refresh_token: Some("old-refresh".to_string()),
            expires_at: Some(0),
            token_endpoint: "https://example.test/token".to_string(),
            issuer: None,
            resource: None,
            mcp_url: None,
            client_metadata_url: None,
            scopes: Vec::new(),
            client_id: "test-client".to_string(),
            client_secret: None,
        };
        let updated = super::StoredOAuthToken {
            access_token: "late-access".to_string(),
            ..original.clone()
        };
        store
            .save("server", original.clone())
            .expect("save original");
        assert!(store.remove("server").expect("logout"));
        assert!(!store
            .save_if_current("server", &original, updated.clone())
            .expect("reject late refresh after removal"));
        assert!(store.load("server").expect("load after removal").is_none());

        store.save("server", updated.clone()).expect("new login");
        assert!(!store
            .save_if_current("server", &original, original.clone())
            .expect("reject late refresh after replacement"));
        assert_eq!(
            store
                .load("server")
                .expect("load replacement")
                .expect("replacement")
                .access_token,
            "late-access"
        );
    }

    #[test]
    fn token_store_round_trips_without_logging_secret() {
        let temp = tempfile::tempdir().expect("temp");
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(temp.path().to_string_lossy().into_owned());
        let store = OAuthTokenStore::open(&cfg).expect("store");
        store
            .save(
                "fake",
                super::StoredOAuthToken {
                    access_token: "access-secret".to_string(),
                    refresh_token: Some("refresh-secret".to_string()),
                    expires_at: Some(u64::MAX),
                    token_endpoint: "http://127.0.0.1/token".to_string(),
                    issuer: None,
                    resource: None,
                    mcp_url: None,
                    client_metadata_url: None,
                    scopes: Vec::new(),
                    client_id: "test-client".to_string(),
                    client_secret: None,
                },
            )
            .expect("save");
        assert_eq!(
            store
                .load("fake")
                .expect("load")
                .expect("token")
                .access_token,
            "access-secret"
        );
        assert!(store.remove("fake").expect("remove"));
        assert!(store.load("fake").expect("load after remove").is_none());
        let _ = McpOAuthConfig::default();
    }
}
