use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::path::{Component, Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::warn;

use crate::infra::config::{get_work_dir, resolve_project_resource_dir};
use crate::infra::error::AppError;
use crate::AppConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpConfigSource {
    Global,
    Project,
}

impl McpConfigSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Global => "Global",
            Self::Project => "Workspace",
        }
    }

    pub fn wire_scope(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Project => "workspace",
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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
    pub trusted: bool,
    #[serde(default)]
    pub integrity: Option<String>,
    #[serde(default = "default_startup_timeout_ms")]
    pub startup_timeout_ms: u64,
    #[serde(default = "default_call_timeout_ms")]
    pub call_timeout_ms: u64,
    #[serde(default)]
    pub tool_filter: ToolFilter,
}

impl McpServerConfig {
    pub fn validate(&self, server_name: &str) -> Result<(), AppError> {
        if server_name.trim().is_empty() {
            return Err(AppError::Config(
                "MCP server name cannot be empty".to_string(),
            ));
        }
        let has_command = !self.command.trim().is_empty();
        let has_url = self
            .url
            .as_deref()
            .is_some_and(|url| !url.trim().is_empty());
        if self.url.is_some() && !has_url {
            return Err(AppError::Config(format!(
                "MCP server '{server_name}' url cannot be empty"
            )));
        }
        if has_command == has_url {
            return Err(AppError::Config(format!(
                "MCP server '{server_name}' must define exactly one of command or url"
            )));
        }
        if let Some(auth) = self.auth.as_deref() {
            if !matches!(auth, "none" | "bearer" | "oauth") {
                return Err(AppError::Config(format!(
                    "MCP server '{server_name}' has unsupported auth mode '{auth}'"
                )));
            }
        }
        let has_auth_header = self
            .headers
            .keys()
            .any(|key| key.eq_ignore_ascii_case("authorization"));
        match self.auth.as_deref() {
            Some("none") if has_auth_header || self.oauth.is_some() => {
                return Err(AppError::Config(format!(
                    "MCP server '{server_name}' auth=none cannot include OAuth or Authorization"
                )));
            }
            Some("bearer") if self.oauth.is_some() => {
                return Err(AppError::Config(format!(
                    "MCP server '{server_name}' bearer auth cannot include OAuth config"
                )));
            }
            Some("oauth") if has_auth_header => {
                return Err(AppError::Config(format!(
                    "MCP server '{server_name}' OAuth auth cannot include Authorization"
                )));
            }
            _ => {}
        }
        if let Some(url) = self.url.as_deref() {
            let parsed = reqwest::Url::parse(url).map_err(|error| {
                AppError::Config(format!(
                    "MCP server '{server_name}' has invalid url: {error}"
                ))
            })?;
            if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
                return Err(AppError::Config(format!(
                    "MCP server '{server_name}' url must be an http(s) URL"
                )));
            }
        }
        if self.startup_timeout_ms == 0 || self.call_timeout_ms == 0 {
            return Err(AppError::Config(format!(
                "MCP server '{server_name}' timeouts must be positive"
            )));
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

pub fn is_floating_npm_version(args: &[String]) -> bool {
    let mut args = args.iter().peekable();
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "-y" | "--yes" | "--quiet" => continue,
            "-p" | "--package" => {
                let _ = args.next();
                continue;
            }
            value if value.starts_with('-') => continue,
            package => return !has_exact_npm_version(package),
        }
    }
    false
}

fn has_exact_npm_version(package: &str) -> bool {
    let Some((_, version)) = package.rsplit_once('@') else {
        return false;
    };
    let mut components = version
        .split(['-', '+'])
        .next()
        .unwrap_or_default()
        .split('.');
    matches!(
        (
            components.next(),
            components.next(),
            components.next(),
            components.next(),
        ),
        (Some(major), Some(minor), Some(patch), None)
            if [major, minor, patch]
                .into_iter()
                .all(|component| !component.is_empty()
                    && component.chars().all(|character| character.is_ascii_digit()))
    )
}

fn is_npx_command(command: &str) -> bool {
    Path::new(command)
        .file_name()
        .is_some_and(|name| name == "npx")
}

#[derive(Debug, Clone)]
pub struct ConfiguredMcpServer {
    pub config_key: String,
    pub name: String,
    pub config: McpServerConfig,
    pub source: McpConfigSource,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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
        return Err(AppError::Config(
            "MCP server name cannot be empty".to_string(),
        ));
    }
    let parent = path.parent().ok_or_else(|| {
        AppError::Config(format!(
            "MCP configuration '{}' has no parent",
            path.display()
        ))
    })?;
    let file_name = path.file_name().ok_or_else(|| {
        AppError::Config(format!(
            "MCP configuration '{}' has no file name",
            path.display()
        ))
    })?;
    let lexical_parent = normalize_path_lexically(parent);
    let normalized_parent = std::fs::canonicalize(&lexical_parent).unwrap_or(lexical_parent);
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
            if is_npx_command(&config.command) && is_floating_npm_version(&config.args) {
                warn!(
                    server = %name,
                    "MCP server uses a floating npx package version; pin an exact @x.y.z version"
                );
            }
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
            return Err(AppError::Config(format!(
                "MCP server '{name}' already exists in {}",
                path.display()
            )));
        }
        file.mcp_servers.insert(name, server);
        Ok(((), true))
    })
}

pub fn remove_global_server(cfg: &AppConfig, name: &str) -> Result<bool, AppError> {
    remove_server_from_file(&global_mcp_path(cfg)?, name)
}

pub fn remove_project_server(
    cfg: &AppConfig,
    workspace_root: &Path,
    name: &str,
) -> Result<bool, AppError> {
    remove_server_from_file(&project_mcp_path(cfg, workspace_root)?, name)
}

fn remove_server_from_file(path: &Path, name: &str) -> Result<bool, AppError> {
    mutate_mcp_file(path, |file| {
        let removed = file.mcp_servers.remove(name).is_some();
        Ok((removed, removed))
    })
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
            AppError::Tool(format!("unknown MCP server '{name}' in {}", path.display()))
        })?;
        server.tool_filter = tool_filter;
        Ok(((), true))
    })
}

fn read_mcp_file(path: &Path) -> Result<McpFile, AppError> {
    if !path.exists() {
        return Ok(McpFile::default());
    }
    let content = std::fs::read_to_string(path)?;
    serde_json::from_str(&content).map_err(|error| {
        AppError::Config(format!(
            "parse MCP configuration '{}': {error}",
            path.display()
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
        AppError::Config(format!(
            "MCP configuration '{}' has no parent",
            path.display()
        ))
    })?;
    std::fs::create_dir_all(parent)?;
    let file_name = path
        .file_name()
        .ok_or_else(|| {
            AppError::Config(format!(
                "MCP configuration '{}' has no file name",
                path.display()
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
                if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
            {
                thread::sleep(Duration::from_millis(25));
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                return Err(AppError::Config(format!(
                    "timed out waiting for MCP configuration lock '{}': 2 seconds",
                    path.display()
                )));
            }
            Err(error) => return Err(error.into()),
        }
    }
}

fn write_mcp_file(path: &Path, file: &McpFile) -> Result<(), AppError> {
    let contents = serde_json::to_vec_pretty(file)
        .map_err(|error| AppError::Config(format!("serialize MCP configuration: {error}")))?;
    crate::infra::platform::write_file_atomic(path, &contents)
}

const fn default_startup_timeout_ms() -> u64 {
    30_000
}

const fn default_call_timeout_ms() -> u64 {
    120_000
}

#[cfg(test)]
mod tests {
    use super::{
        add_global_server, connector_config_key, global_mcp_path, is_floating_npm_version,
        load_server_definitions, load_servers, project_mcp_path, McpConfigSource, McpServerConfig,
    };
    use crate::infra::config::get_work_dir;
    use crate::AppConfig;

    #[test]
    fn minimal_cursor_style_server_uses_optional_field_defaults() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let workspace = temp.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
        let global = get_work_dir(&cfg).expect("work dir").join("mcp.json");
        std::fs::create_dir_all(global.parent().expect("global parent")).expect("global parent");
        std::fs::write(
            global,
            r#"{"mcpServers":{"browser":{"command":"npx","args":["-y","browser-mcp@1.2.3"]}}}"#,
        )
        .expect("write config");

        let servers = load_servers(&cfg, Some(&workspace)).expect("parse minimal MCP config");
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].name, "browser");
        assert_eq!(servers[0].config.command, "npx");
        assert_eq!(servers[0].config.args, ["-y", "browser-mcp@1.2.3"]);
        assert_eq!(servers[0].source, McpConfigSource::Global);
        assert!(servers[0].config.env.is_empty());
        assert!(servers[0].config.cwd.is_none());
        assert!(!servers[0].config.trusted);
        assert!(servers[0].config.integrity.is_none());
        assert_eq!(servers[0].config.startup_timeout_ms, 30_000);
        assert_eq!(servers[0].config.call_timeout_ms, 120_000);
        assert!(servers[0].config.tool_filter.include.is_empty());
        assert!(servers[0].config.tool_filter.exclude.is_empty());
    }
    #[test]
    fn no_project_root_loads_global_mcp_without_touching_a_project_path() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
        let global = get_work_dir(&cfg).expect("work dir").join("mcp.json");
        std::fs::create_dir_all(global.parent().expect("global parent")).unwrap();
        std::fs::write(
            global,
            r#"{"mcpServers":{"global-only":{"command":"node","args":[]}}}"#,
        )
        .unwrap();

        let servers = load_servers(&cfg, None).expect("global MCP must work without a project");
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].name, "global-only");
        assert_eq!(servers[0].source, McpConfigSource::Global);
    }

    #[test]
    fn project_mcp_path_uses_configured_resource_directory_and_rejects_invalid_values() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let workspace = temp.path().join("workspace");
        let mut cfg = AppConfig::default();

        assert_eq!(
            project_mcp_path(&cfg, &workspace).expect("default project config path"),
            workspace.join(".agents").join("mcp.json")
        );

        cfg.workspace.project_resource_dir = ".workspace-data".to_string();
        assert_eq!(
            project_mcp_path(&cfg, &workspace).expect("custom project config path"),
            workspace.join(".workspace-data").join("mcp.json")
        );

        cfg.workspace.project_resource_dir = "../outside".to_string();
        assert!(project_mcp_path(&cfg, &workspace).is_err());
    }

    #[test]
    fn project_server_overrides_global_server_with_same_name() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let workspace = temp.path().join("workspace");
        let mut cfg = AppConfig::default();
        cfg.workspace.project_resource_dir = ".workspace-data".to_string();
        cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
        let project = project_mcp_path(&cfg, &workspace).expect("project config path");
        std::fs::create_dir_all(project.parent().expect("project config directory"))
            .expect("project config directory");
        let global = get_work_dir(&cfg).expect("work dir").join("mcp.json");
        std::fs::create_dir_all(global.parent().expect("global parent")).expect("global parent");
        std::fs::write(
            global,
            r#"{"mcpServers":{"same":{"command":"global","args":[]}}}"#,
        )
        .expect("write global config");
        std::fs::write(
            project_mcp_path(&cfg, &workspace).expect("project config path"),
            r#"{"mcpServers":{"same":{"command":"project","args":[]}}}"#,
        )
        .expect("write project config");

        let servers = load_servers(&cfg, Some(&workspace)).expect("load merged config");
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].config.command, "project");
        assert_eq!(servers[0].source, McpConfigSource::Project);
        assert!(
            !workspace.join(".agents").join("mcp.json").exists(),
            "loading the custom path must not fall back to the default project path"
        );
    }

    #[test]
    fn http_server_requires_exactly_one_transport_selector() {
        let both: McpServerConfig = serde_json::from_value(serde_json::json!({
            "command": "node",
            "args": [],
            "url": "https://example.test/mcp"
        }))
        .expect("config");
        assert!(both.validate("both").is_err());

        let http: McpServerConfig = serde_json::from_value(serde_json::json!({
            "url": "https://example.test/mcp"
        }))
        .expect("HTTP config");
        assert!(http.validate("http").is_ok());

        let invalid: McpServerConfig = serde_json::from_value(serde_json::json!({
            "url": "file:///tmp/mcp"
        }))
        .expect("invalid URL config parses before validation");
        assert!(invalid.validate("invalid").is_err());
    }

    #[test]
    fn config_key_is_stable_for_path_aliases_and_separates_sources() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let config_dir = temp.path().join("configs");
        std::fs::create_dir_all(&config_dir).expect("config directory");
        let direct = config_dir.join("mcp.json");
        let alias = config_dir.join("subdir").join("..").join("mcp.json");
        let same = connector_config_key(&direct, "server").expect("direct key");
        assert_eq!(
            same,
            connector_config_key(&alias, "server").expect("alias key")
        );
        assert_ne!(
            same,
            connector_config_key(&direct, "other").expect("name key")
        );
        assert_ne!(
            same,
            connector_config_key(&temp.path().join("other.json"), "server").expect("path key")
        );
    }

    #[test]
    fn current_schema_rejects_unknown_fields_without_rewriting_the_file() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
        let path = global_mcp_path(&cfg).expect("global config path");
        std::fs::create_dir_all(path.parent().expect("config parent")).expect("config parent");
        let raw = r#"{"mcpServers":{"browser":{"command":"node","args":[],"removedField":true}}}"#;
        std::fs::write(&path, raw).expect("write malformed current config");

        let error = load_servers(&cfg, None).expect_err("unknown fields must be rejected");
        assert!(error.to_string().contains("removedField"));
        assert_eq!(std::fs::read_to_string(path).expect("read source"), raw);
    }

    #[test]
    fn add_rejects_duplicate_without_overwriting_existing_server() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
        let initial: McpServerConfig = serde_json::from_value(serde_json::json!({
            "command": "node", "args": ["first"]
        }))
        .expect("initial config");
        add_global_server(&cfg, "same".to_string(), initial).expect("first add");
        let replacement: McpServerConfig = serde_json::from_value(serde_json::json!({
            "command": "node", "args": ["replacement"]
        }))
        .expect("replacement config");

        let error = add_global_server(&cfg, "same".to_string(), replacement)
            .expect_err("duplicate add must fail");
        assert!(error.to_string().contains("already exists"));
        let servers = load_servers(&cfg, None).expect("read existing server");
        assert_eq!(servers[0].config.args, ["first"]);
    }

    #[test]
    fn concurrent_adds_keep_both_current_format_servers() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        std::thread::scope(|scope| {
            for name in ["left", "right"] {
                let cfg = cfg.clone();
                let barrier = barrier.clone();
                scope.spawn(move || {
                    barrier.wait();
                    let server: McpServerConfig = serde_json::from_value(serde_json::json!({
                        "command": "node", "args": [name]
                    }))
                    .expect("server config");
                    add_global_server(&cfg, name.to_string(), server).expect("concurrent add");
                });
            }
        });
        let names = load_server_definitions(&cfg, None)
            .expect("read both adds")
            .into_iter()
            .map(|server| server.name)
            .collect::<Vec<_>>();
        assert_eq!(names, ["left", "right"]);
    }

    #[test]
    fn identifies_floating_npx_package_versions() {
        assert!(is_floating_npm_version(&[
            "-y".to_string(),
            "browser-mcp".to_string()
        ]));
        assert!(is_floating_npm_version(&[
            "--yes".to_string(),
            "@scope/browser-mcp@latest".to_string(),
        ]));
        assert!(is_floating_npm_version(&[
            "-y".to_string(),
            "browser-mcp@next".to_string(),
        ]));
        assert!(!is_floating_npm_version(&[
            "-y".to_string(),
            "@playwright/mcp@0.0.79".to_string(),
            "--headless".to_string(),
        ]));
    }
}
