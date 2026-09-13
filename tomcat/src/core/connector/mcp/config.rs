use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
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
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolFilter {
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
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

#[derive(Debug, Clone, Deserialize, Serialize)]
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
    pub name: String,
    pub config: McpServerConfig,
    pub source: McpConfigSource,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
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

pub fn load_servers(
    cfg: &AppConfig,
    workspace_root: Option<&Path>,
) -> Result<Vec<ConfiguredMcpServer>, AppError> {
    let mut merged = read_mcp_file(&global_mcp_path(cfg)?)?
        .mcp_servers
        .into_iter()
        .map(|(name, config)| {
            (
                name,
                ConfiguredMcpServer {
                    name: String::new(),
                    config,
                    source: McpConfigSource::Global,
                },
            )
        })
        .collect::<BTreeMap<_, _>>();

    if let Some(workspace_root) = workspace_root {
        for (name, config) in read_mcp_file(&project_mcp_path(cfg, workspace_root)?)?.mcp_servers {
            merged.insert(
                name,
                ConfiguredMcpServer {
                    name: String::new(),
                    config,
                    source: McpConfigSource::Project,
                },
            );
        }
    }

    merged
        .into_iter()
        .filter(|(name, _)| {
            !cfg.connector
                .disabled
                .iter()
                .any(|disabled| disabled == name)
        })
        .map(|(name, mut server)| {
            server.config.validate(&name)?;
            server.name = name;
            if is_npx_command(&server.config.command)
                && is_floating_npm_version(&server.config.args)
            {
                warn!(
                    server = %server.name,
                    "MCP server uses a floating npx package version; pin an exact @x.y.z version"
                );
            }
            Ok(server)
        })
        .collect()
}

pub fn upsert_global_server(
    cfg: &AppConfig,
    name: String,
    server: McpServerConfig,
) -> Result<(), AppError> {
    server.validate(&name)?;
    let path = global_mcp_path(cfg)?;
    let mut file = read_mcp_file(&path)?;
    file.mcp_servers.insert(name, server);
    write_mcp_file(&path, &file)
}

pub fn remove_global_server(cfg: &AppConfig, name: &str) -> Result<bool, AppError> {
    let path = global_mcp_path(cfg)?;
    let mut file = read_mcp_file(&path)?;
    let removed = file.mcp_servers.remove(name).is_some();
    if removed {
        write_mcp_file(&path, &file)?;
    }
    Ok(removed)
}

pub fn upsert_project_server(
    cfg: &AppConfig,
    workspace_root: &Path,
    name: String,
    server: McpServerConfig,
) -> Result<(), AppError> {
    server.validate(&name)?;
    let path = project_mcp_path(cfg, workspace_root)?;
    let mut file = read_mcp_file(&path)?;
    file.mcp_servers.insert(name, server);
    write_mcp_file(&path, &file)
}

pub fn remove_project_server(
    cfg: &AppConfig,
    workspace_root: &Path,
    name: &str,
) -> Result<bool, AppError> {
    let path = project_mcp_path(cfg, workspace_root)?;
    let mut file = read_mcp_file(&path)?;
    let removed = file.mcp_servers.remove(name).is_some();
    if removed {
        write_mcp_file(&path, &file)?;
    }
    Ok(removed)
}

pub fn set_project_tool_filter(
    cfg: &AppConfig,
    workspace_root: &Path,
    name: &str,
    tool_filter: ToolFilter,
) -> Result<(), AppError> {
    let path = project_mcp_path(cfg, workspace_root)?;
    let mut file = read_mcp_file(&path)?;
    let server = file
        .mcp_servers
        .get_mut(name)
        .ok_or_else(|| AppError::Tool(format!("unknown project MCP server: {name}")))?;
    server.tool_filter = tool_filter;
    write_mcp_file(&path, &file)
}

pub fn set_global_tool_filter(
    cfg: &AppConfig,
    name: &str,
    tool_filter: ToolFilter,
) -> Result<(), AppError> {
    let path = global_mcp_path(cfg)?;
    let mut file = read_mcp_file(&path)?;
    let server = file
        .mcp_servers
        .get_mut(name)
        .ok_or_else(|| AppError::Tool(format!("unknown global MCP server: {name}")))?;
    server.tool_filter = tool_filter;
    write_mcp_file(&path, &file)
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
        is_floating_npm_version, load_servers, project_mcp_path, McpConfigSource, McpServerConfig,
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
