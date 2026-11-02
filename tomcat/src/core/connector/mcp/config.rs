use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::path::{Component, Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::infra::config::{get_work_dir, resolve_project_resource_dir};
use crate::infra::error::AppError;
use crate::infra::i18n::tr;
use crate::AppConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpConfigSource {
    Global,
    Project,
    /// Ephemeral ACP configuration; never backed by an mcp.json file.
    Session,
}

impl McpConfigSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Global => "Global",
            Self::Project => "Workspace",
            Self::Session => "Session",
        }
    }

    pub fn wire_scope(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Project => "workspace",
            Self::Session => "session",
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolFilter {
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
}

/// Compiled once per operation so Settings, deferred discovery and actual calls
/// all use the same include-then-exclude decision for one tool name.
pub(crate) struct ToolFilterMatcher {
    include_all: bool,
    include: globset::GlobSet,
    exclude: globset::GlobSet,
}

impl ToolFilterMatcher {
    pub(crate) fn compile(filter: &ToolFilter) -> Result<Self, AppError> {
        Ok(Self {
            include_all: filter.include.is_empty(),
            include: build_glob_set(&filter.include)?,
            exclude: build_glob_set(&filter.exclude)?,
        })
    }

    pub(crate) fn allows(&self, raw_name: &str) -> bool {
        (self.include_all || self.include.is_match(raw_name)) && !self.exclude.is_match(raw_name)
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpOAuthConfig {
    /// Optional pre-registered public client identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// Name of an environment variable used only when the provider requires a confidential client.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret_env: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scopes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callback_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_metadata_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct McpServerConfig {
    /// Stdio executable. Empty when `url` selects Streamable HTTP.
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Streamable HTTP endpoint. Mutually exclusive with `command`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth: Option<McpOAuthConfig>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub tool_filter: ToolFilter,
}

impl McpServerConfig {
    pub fn validate(&self, server_name: &str) -> Result<(), AppError> {
        if server_name.trim().is_empty() {
            return Err(AppError::Config(tr("mcp.config.nameEmpty", &[])));
        }
        let has_command = !self.command.trim().is_empty();
        let has_url = self
            .url
            .as_deref()
            .is_some_and(|url| !url.trim().is_empty());
        if self.url.is_some() && !has_url {
            return Err(AppError::Config(tr(
                "mcp.config.urlEmpty",
                &[("name", server_name)],
            )));
        }
        if has_command == has_url {
            return Err(AppError::Config(tr(
                "mcp.config.transportChoice",
                &[("name", server_name)],
            )));
        }
        if let Some(auth) = self.auth.as_deref() {
            if !matches!(auth, "none" | "bearer" | "oauth") {
                return Err(AppError::Config(tr(
                    "mcp.config.authUnsupported",
                    &[("name", server_name), ("auth", auth)],
                )));
            }
        }
        let has_auth_header = self
            .headers
            .keys()
            .any(|key| key.eq_ignore_ascii_case("authorization"));
        match self.auth.as_deref() {
            Some("none") if has_auth_header || self.oauth.is_some() => {
                return Err(AppError::Config(tr(
                    "mcp.config.noAuthConflict",
                    &[("name", server_name)],
                )));
            }
            Some("bearer") if self.oauth.is_some() => {
                return Err(AppError::Config(tr(
                    "mcp.config.bearerConflict",
                    &[("name", server_name)],
                )));
            }
            Some("oauth") if has_auth_header => {
                return Err(AppError::Config(tr(
                    "mcp.config.oauthConflict",
                    &[("name", server_name)],
                )));
            }
            _ => {}
        }
        if let Some(url) = self.url.as_deref() {
            let parsed = reqwest::Url::parse(url).map_err(|error| {
                AppError::Config(tr(
                    "mcp.config.invalidUrl",
                    &[("name", server_name), ("detail", &error.to_string())],
                ))
            })?;
            if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
                return Err(AppError::Config(tr(
                    "mcp.config.httpUrl",
                    &[("name", server_name)],
                )));
            }
        }
        Ok(())
    }

    pub fn normalized_cwd(&self, workspace_root: &Path) -> PathBuf {
        match self.cwd.as_deref() {
            Some(path) if path.is_absolute() => path.to_path_buf(),
            Some(path) => workspace_root.join(path),
            None => workspace_root.to_path_buf(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConfiguredMcpServer {
    pub config_key: String,
    pub name: String,
    pub config: McpServerConfig,
    pub source: McpConfigSource,
}

/// The file envelope stays tolerant: a document copied from another tool may
/// carry keys such as `$schema`, and one unrecognized key must not make every
/// server in the file unreachable. The nested `toolFilter` and `oauth` objects
/// keep `deny_unknown_fields`, where a typo changes behavior silently.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct McpFile {
    #[serde(default)]
    mcp_servers: BTreeMap<String, McpServerConfig>,
}

pub fn global_mcp_path(cfg: &AppConfig) -> Result<PathBuf, AppError> {
    Ok(get_work_dir(cfg)?.join("mcp.json"))
}

pub fn project_mcp_path(cfg: &AppConfig, workspace_root: &Path) -> Result<PathBuf, AppError> {
    Ok(resolve_project_resource_dir(cfg, workspace_root)?.join("mcp.json"))
}

/// Opaque identity for one definition in one current-format MCP file. It is
/// derived from the normalized file path and entry name, never from a display
/// label or the effective Workspace-over-Global resolution.
pub fn connector_config_key(path: &Path, name: &str) -> Result<String, AppError> {
    if name.trim().is_empty() {
        return Err(AppError::Config(tr("mcp.config.nameEmpty", &[])));
    }
    let parent = path.parent().ok_or_else(|| {
        AppError::Config(tr(
            "mcp.config.noParent",
            &[("path", &path.display().to_string())],
        ))
    })?;
    let file_name = path.file_name().ok_or_else(|| {
        AppError::Config(tr(
            "mcp.config.noFilename",
            &[("path", &path.display().to_string())],
        ))
    })?;
    let lexical_parent = normalize_path_lexically(parent);
    let normalized_parent =
        crate::infra::platform::canonicalize(&lexical_parent).unwrap_or(lexical_parent);
    let normalized_path = normalized_parent.join(file_name);
    let mut hasher = Sha256::new();
    hasher.update(b"tomcat.mcp.config-key.v1\0");
    hasher.update(normalized_path.to_string_lossy().as_bytes());
    hasher.update(b"\0");
    hasher.update(name.as_bytes());
    Ok(format!("mcp:{:x}", hasher.finalize()))
}

fn normalize_path_lexically(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    let absolute = path.is_absolute();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() && !absolute {
                    normalized.push("..");
                }
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    normalized
}

pub fn load_servers(
    cfg: &AppConfig,
    workspace_root: Option<&Path>,
) -> Result<Vec<ConfiguredMcpServer>, AppError> {
    let mut effective = BTreeMap::new();
    for server in load_server_definitions(cfg, workspace_root)? {
        effective.insert(server.name.clone(), server);
    }
    Ok(effective.into_values().collect())
}

/// Loads every current-format definition. Project entries are included alongside
/// their Global counterparts so management surfaces can describe an override;
/// callers that execute tools must use [`load_servers`] for Workspace precedence.
pub fn load_server_definitions(
    cfg: &AppConfig,
    workspace_root: Option<&Path>,
) -> Result<Vec<ConfiguredMcpServer>, AppError> {
    let global_path = global_mcp_path(cfg)?;
    let mut servers = configured_servers_from_file(&global_path, McpConfigSource::Global)?;
    if let Some(workspace_root) = workspace_root {
        let project_path = project_mcp_path(cfg, workspace_root)?;
        servers.extend(configured_servers_from_file(
            &project_path,
            McpConfigSource::Project,
        )?);
    }
    servers.retain(|server| {
        !cfg.connector
            .disabled
            .iter()
            .any(|disabled| disabled == &server.name)
    });
    Ok(servers)
}

fn configured_servers_from_file(
    path: &Path,
    source: McpConfigSource,
) -> Result<Vec<ConfiguredMcpServer>, AppError> {
    read_mcp_file(path)?
        .mcp_servers
        .into_iter()
        .map(|(name, config)| {
            config.validate(&name)?;
            Ok(ConfiguredMcpServer {
                config_key: connector_config_key(path, &name)?,
                name,
                config,
                source,
            })
        })
        .collect()
}

pub fn add_global_server(
    cfg: &AppConfig,
    name: String,
    server: McpServerConfig,
) -> Result<(), AppError> {
    server.validate(&name)?;
    add_server_to_file(&global_mcp_path(cfg)?, name, server)
}

pub fn add_project_server(
    cfg: &AppConfig,
    workspace_root: &Path,
    name: String,
    server: McpServerConfig,
) -> Result<(), AppError> {
    server.validate(&name)?;
    add_server_to_file(&project_mcp_path(cfg, workspace_root)?, name, server)
}

fn add_server_to_file(path: &Path, name: String, server: McpServerConfig) -> Result<(), AppError> {
    mutate_mcp_file(path, move |file| {
        if file.mcp_servers.contains_key(&name) {
            return Err(AppError::Config(tr(
                "mcp.config.exists",
                &[("name", &name), ("path", &path.display().to_string())],
            )));
        }
        file.mcp_servers.insert(name, server);
        Ok(((), true))
    })
}

/// Remove a Global definition and its stored authorization, even when disabled.
/// Credential cleanup can fail after the config is saved; callers must still
/// reconcile their runtimes. Repeating removal retries that cleanup.
pub fn remove_global_server(cfg: &AppConfig, name: &str) -> Result<bool, AppError> {
    remove_server_from_file(cfg, &global_mcp_path(cfg)?, name)
}

pub fn remove_project_server(
    cfg: &AppConfig,
    workspace_root: &Path,
    name: &str,
) -> Result<bool, AppError> {
    remove_server_from_file(cfg, &project_mcp_path(cfg, workspace_root)?, name)
}

fn remove_server_from_file(cfg: &AppConfig, path: &Path, name: &str) -> Result<bool, AppError> {
    let key = connector_config_key(path, name)?;
    let removed = mutate_mcp_file(path, |file| {
        let removed = file.mcp_servers.remove(name).is_some();
        Ok((removed, removed))
    })?;
    // One deletion boundary for loaded and config-only sources. Keep the
    // existing store revision fence, including retries after partial failure.
    super::oauth::OAuthTokenStore::open(cfg)?.remove(&key)?;
    Ok(removed)
}

pub fn set_project_tool_filter(
    cfg: &AppConfig,
    workspace_root: &Path,
    name: &str,
    tool_filter: ToolFilter,
) -> Result<(), AppError> {
    set_tool_filter_in_file(&project_mcp_path(cfg, workspace_root)?, name, tool_filter)
}

pub fn set_global_tool_filter(
    cfg: &AppConfig,
    name: &str,
    tool_filter: ToolFilter,
) -> Result<(), AppError> {
    set_tool_filter_in_file(&global_mcp_path(cfg)?, name, tool_filter)
}

fn set_tool_filter_in_file(
    path: &Path,
    name: &str,
    tool_filter: ToolFilter,
) -> Result<(), AppError> {
    mutate_mcp_file(path, |file| {
        let server = file.mcp_servers.get_mut(name).ok_or_else(|| {
            AppError::Tool(tr(
                "mcp.config.unknown",
                &[("name", name), ("path", &path.display().to_string())],
            ))
        })?;
        server.tool_filter = tool_filter;
        Ok(((), true))
    })
}

/// Set one tool's desired effective state while retaining all unrelated
/// include/exclude rules. `true` means this tool must be allowed by the final
/// filter; a pre-existing broader rule is reported instead of being weakened.
pub fn set_global_tool_enabled(
    cfg: &AppConfig,
    name: &str,
    raw_name: &str,
    enabled: bool,
) -> Result<bool, AppError> {
    set_tool_enabled_in_file(&global_mcp_path(cfg)?, name, raw_name, enabled)
}

pub fn set_project_tool_enabled(
    cfg: &AppConfig,
    workspace_root: &Path,
    name: &str,
    raw_name: &str,
    enabled: bool,
) -> Result<bool, AppError> {
    set_tool_enabled_in_file(
        &project_mcp_path(cfg, workspace_root)?,
        name,
        raw_name,
        enabled,
    )
}

fn set_tool_enabled_in_file(
    path: &Path,
    name: &str,
    raw_name: &str,
    enabled: bool,
) -> Result<bool, AppError> {
    if raw_name.is_empty() {
        return Err(AppError::Config(tr("mcp.config.toolNameEmpty", &[])));
    }
    let exact_pattern = globset::escape(raw_name);
    mutate_mcp_file(path, |file| {
        let server = file.mcp_servers.get_mut(name).ok_or_else(|| {
            AppError::Tool(tr(
                "mcp.config.unknown",
                &[("name", name), ("path", &path.display().to_string())],
            ))
        })?;
        let before = server.tool_filter.clone();
        let mut next = before.clone();
        let current = ToolFilterMatcher::compile(&before)?;
        if enabled {
            // New writes use an escaped exact pattern. For historical ordinary
            // names this is the same text; a historical wildcard remains
            // untouched because it cannot safely be identified as one tool.
            next.exclude.retain(|pattern| pattern != &exact_pattern);
            let included = next.include.is_empty()
                || ToolFilterMatcher::compile(&ToolFilter {
                    include: next.include.clone(),
                    exclude: Vec::new(),
                })?
                .allows(raw_name);
            if !included && !next.include.contains(&exact_pattern) {
                next.include.push(exact_pattern.clone());
            }
        } else if current.allows(raw_name) && !next.exclude.contains(&exact_pattern) {
            next.exclude.push(exact_pattern.clone());
        }

        if !ToolFilterMatcher::compile(&next)?.allows(raw_name) && enabled {
            return Err(AppError::Tool(tr("mcp.config.batchRule", &[])));
        }
        let changed = next != before;
        if changed {
            server.tool_filter = next;
        }
        Ok((changed, changed))
    })
}

fn build_glob_set(patterns: &[String]) -> Result<globset::GlobSet, AppError> {
    let mut builder = globset::GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(globset::Glob::new(pattern).map_err(|error| {
            AppError::Config(tr(
                "mcp.config.invalidFilter",
                &[("pattern", pattern), ("detail", &error.to_string())],
            ))
        })?);
    }
    builder.build().map_err(|error| {
        AppError::Config(tr(
            "mcp.config.buildFilter",
            &[("detail", &error.to_string())],
        ))
    })
}

/// Server-level keys Tomcat no longer reads. They stay tolerated on purpose: a
/// key this version does not understand must never take the whole file down, or
/// an old `tomcat init` file would leave every MCP server unusable.
const IGNORED_SERVER_KEYS: &[(&str, &str)] = &[
    (
        "startupTimeoutMs",
        "moved to [connector.mcp].startup_timeout_ms in tomcat.config.toml",
    ),
    (
        "callTimeoutMs",
        "moved to [connector.mcp].call_timeout_ms in tomcat.config.toml",
    ),
    (
        "maxConcurrentCalls",
        "moved to [connector.mcp].max_concurrent_calls in tomcat.config.toml",
    ),
    (
        "trusted",
        "replaced by one-time project trust; approve the project when Tomcat asks",
    ),
    (
        "integrity",
        "replaced by one-time project trust; just delete this key",
    ),
];

fn warn_about_ignored_server_keys(path: &Path, raw: &serde_json::Value) {
    let Some(servers) = raw.get("mcpServers").and_then(serde_json::Value::as_object) else {
        return;
    };
    for (name, definition) in servers {
        for (key, remedy) in IGNORED_SERVER_KEYS {
            if definition.get(key).is_some() {
                tracing::warn!(
                    file = %path.display(),
                    server = %name,
                    ignored_key = %key,
                    "MCP server '{}' contains key '{}' that this version ignores ({remedy})",
                    name,
                    key
                );
            }
        }
    }
}

fn read_mcp_file(path: &Path) -> Result<McpFile, AppError> {
    if !path.exists() {
        return Ok(McpFile::default());
    }
    let content = std::fs::read_to_string(path)?;
    let raw: serde_json::Value = serde_json::from_str(&content).map_err(|error| {
        AppError::Config(tr(
            "mcp.config.parse",
            &[
                ("path", &path.display().to_string()),
                ("detail", &error.to_string()),
            ],
        ))
    })?;
    warn_about_ignored_server_keys(path, &raw);
    serde_json::from_str(&content).map_err(|error| {
        AppError::Config(tr(
            "mcp.config.parse",
            &[
                ("path", &path.display().to_string()),
                ("detail", &error.to_string()),
            ],
        ))
    })
}

/// Applies one mutation while holding a stable sidecar lock. The target itself
/// is atomically replaced, therefore it must never be used as the lock file.
fn mutate_mcp_file<T>(
    path: &Path,
    mutate: impl FnOnce(&mut McpFile) -> Result<(T, bool), AppError>,
) -> Result<T, AppError> {
    let _guard = acquire_mcp_file_lock(path)?;
    let mut file = read_mcp_file(path)?;
    let (result, changed) = mutate(&mut file)?;
    if changed {
        write_mcp_file(path, &file)?;
    }
    Ok(result)
}

fn acquire_mcp_file_lock(path: &Path) -> Result<File, AppError> {
    let parent = path.parent().ok_or_else(|| {
        AppError::Config(tr(
            "mcp.config.noParent",
            &[("path", &path.display().to_string())],
        ))
    })?;
    std::fs::create_dir_all(parent)?;
    let file_name = path
        .file_name()
        .ok_or_else(|| {
            AppError::Config(tr(
                "mcp.config.noFilename",
                &[("path", &path.display().to_string())],
            ))
        })?
        .to_string_lossy();
    let lock_path = parent.join(format!(".{file_name}.lock"));
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)?;
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match lock.try_lock_exclusive() {
            Ok(()) => return Ok(lock),
            Err(error)
                if crate::infra::platform::is_lock_contended(&error)
                    && Instant::now() < deadline =>
            {
                thread::sleep(Duration::from_millis(25));
            }
            Err(error) if crate::infra::platform::is_lock_contended(&error) => {
                return Err(AppError::Config(tr(
                    "mcp.config.lockTimeout",
                    &[("path", &path.display().to_string())],
                )));
            }
            Err(error) => return Err(error.into()),
        }
    }
}

fn write_mcp_file(path: &Path, file: &McpFile) -> Result<(), AppError> {
    let contents = serde_json::to_vec_pretty(file).map_err(|error| {
        AppError::Config(tr(
            "mcp.config.serialize",
            &[("detail", &error.to_string())],
        ))
    })?;
    crate::infra::platform::write_file_atomic(path, &contents)
}

#[cfg(test)]
#[path = "tests/config_test.rs"]
mod tests;
