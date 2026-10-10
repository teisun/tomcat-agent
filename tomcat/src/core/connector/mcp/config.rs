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
                if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
            {
                thread::sleep(Duration::from_millis(25));
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
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
mod tests {
    use super::{
        add_global_server, connector_config_key, global_mcp_path, load_server_definitions,
        load_servers, project_mcp_path, set_global_tool_enabled, McpConfigSource, McpServerConfig,
        ToolFilterMatcher, IGNORED_SERVER_KEYS,
    };
    use crate::infra::config::get_work_dir;
    use crate::AppConfig;

    #[derive(Clone)]
    struct SharedLogWriter(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for SharedLogWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .expect("lock test log")
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

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
        assert!(servers[0].config.tool_filter.include.is_empty());
        assert!(servers[0].config.tool_filter.exclude.is_empty());
    }
    #[test]
    fn legacy_server_keys_are_tolerated_without_rewriting_the_file() {
        let temp = tempfile::tempdir().unwrap();
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
        let path = global_mcp_path(&cfg).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let contents = serde_json::json!({
            "mcpServers": {
                "legacy": {
                    "command": "node",
                    "startupTimeoutMs": 30_000,
                    "callTimeoutMs": 90_000,
                    "maxConcurrentCalls": 16,
                    "trusted": true,
                    "integrity": "obsolete"
                }
            }
        })
        .to_string();
        std::fs::write(&path, &contents).unwrap();

        let logs = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .without_time()
            .with_writer({
                let logs = std::sync::Arc::clone(&logs);
                move || SharedLogWriter(std::sync::Arc::clone(&logs))
            })
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        let servers = load_servers(&cfg, None).expect("legacy keys must not block loading");
        assert_eq!(servers.len(), 1);
        let serialized = serde_json::to_string(&servers[0].config).unwrap();
        let rendered = String::from_utf8(logs.lock().expect("lock test log").clone())
            .expect("warnings must be UTF-8");
        for key in [
            "startupTimeoutMs",
            "callTimeoutMs",
            "maxConcurrentCalls",
            "trusted",
            "integrity",
        ] {
            assert!(
                !serialized.contains(key),
                "ignored legacy key leaked into output: {key}"
            );
            assert!(
                IGNORED_SERVER_KEYS
                    .iter()
                    .any(|(ignored, _)| ignored == &key),
                "legacy key must retain its ignored-with-WARN classification: {key}"
            );
            assert!(
                rendered.contains(&format!("ignored_key={key}")),
                "missing ignored-key warning for {key}: {rendered}"
            );
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), contents);
    }

    #[test]
    fn truly_unknown_fields_are_silently_ignored() {
        let temp = tempfile::tempdir().unwrap();
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
        let path = global_mcp_path(&cfg).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{"mcpServers":{"future":{"command":"node","futureField":{"version":2}}}}"#,
        )
        .unwrap();

        let servers = load_servers(&cfg, None).expect("unknown server fields must be ignored");
        assert_eq!(servers[0].name, "future");
        assert_eq!(servers[0].config.command, "node");
    }

    #[test]
    fn single_tool_mutation_uses_exact_patterns_and_preserves_batch_rules() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
        let server: McpServerConfig = serde_json::from_value(serde_json::json!({
            "command": "node", "args": []
        }))
        .expect("server configuration");
        add_global_server(&cfg, "fake".to_string(), server).expect("add server");

        assert!(set_global_tool_enabled(&cfg, "fake", "capture", false).expect("disable capture"));
        let disabled = load_servers(&cfg, None).expect("load disabled server");
        assert_eq!(disabled[0].config.tool_filter.exclude, ["capture"]);
        assert!(!ToolFilterMatcher::compile(&disabled[0].config.tool_filter)
            .expect("compile disabled filter")
            .allows("capture"));

        assert!(set_global_tool_enabled(&cfg, "fake", "capture", true).expect("re-enable capture"));
        let enabled = load_servers(&cfg, None).expect("load enabled server");
        assert!(enabled[0].config.tool_filter.exclude.is_empty());
        assert!(ToolFilterMatcher::compile(&enabled[0].config.tool_filter)
            .expect("compile enabled filter")
            .allows("capture"));

        let mixed_include = r#"{"mcpServers":{"fake":{"command":"node","args":[],"toolFilter":{"include":["status*"]}}}}"#;
        std::fs::write(global_mcp_path(&cfg).expect("global config"), mixed_include)
            .expect("write mixed include rule");
        assert!(set_global_tool_enabled(&cfg, "fake", "capture", true)
            .expect("add exact capture include beside a glob"));
        let mixed_enabled = load_servers(&cfg, None).expect("load mixed include rule");
        assert_eq!(
            mixed_enabled[0].config.tool_filter.include,
            ["status*", "capture"]
        );
        assert!(!set_global_tool_enabled(&cfg, "fake", "capture", true)
            .expect("same desired state is a no-op"));

        let special_name = "capture[1]";
        std::fs::write(
            global_mcp_path(&cfg).expect("global config"),
            r#"{"mcpServers":{"fake":{"command":"node","args":[],"toolFilter":{"include":["status*"]}}}}"#,
        )
        .expect("reset mixed include rule");
        assert!(set_global_tool_enabled(&cfg, "fake", special_name, true)
            .expect("escape special-character tool name"));
        let special_enabled = load_servers(&cfg, None).expect("load special tool rule");
        assert!(
            ToolFilterMatcher::compile(&special_enabled[0].config.tool_filter)
                .expect("compile special tool rule")
                .allows(special_name)
        );
        let path = global_mcp_path(&cfg).expect("global config");
        let batch_rule = r#"{"mcpServers":{"fake":{"command":"node","args":[],"toolFilter":{"exclude":["capture*"]}}}}"#;
        std::fs::write(&path, batch_rule).expect("write batch rule");
        let error = set_global_tool_enabled(&cfg, "fake", "capture", true)
            .expect_err("a single tool must not weaken a batch rule");
        assert!(error.to_string().contains(&crate::infra::i18n::tr_in(
            crate::infra::i18n::Locale::En,
            "mcp.config.batchRule",
            &[]
        )));
        assert_eq!(
            std::fs::read_to_string(path).expect("read unchanged batch rule"),
            batch_rule
        );
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
    fn unknown_fields_are_ignored_without_rewriting_the_file() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
        let path = global_mcp_path(&cfg).expect("global config path");
        std::fs::create_dir_all(path.parent().expect("config parent")).expect("config parent");
        let raw = r#"{"$schema":"https://example.invalid/mcp.schema.json","comment":"kept until a save","mcpServers":{"browser":{"command":"node","args":[],"removedField":true}}}"#;
        std::fs::write(&path, raw).expect("write config with unknown fields");

        let servers = load_servers(&cfg, None).expect("unknown fields must be ignored");
        assert_eq!(servers[0].config.command, "node");
        assert_eq!(std::fs::read_to_string(path).expect("read source"), raw);
    }

    #[test]
    fn unknown_fields_inside_tool_filter_remain_errors() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
        let path = global_mcp_path(&cfg).expect("global config path");
        std::fs::create_dir_all(path.parent().expect("config parent")).expect("config parent");
        let raw =
            r#"{"mcpServers":{"browser":{"command":"node","toolFilter":{"unknownRule":true}}}}"#;
        std::fs::write(&path, raw).expect("write invalid nested configuration");

        let error = load_servers(&cfg, None)
            .expect_err("toolFilter must keep rejecting unknown fields")
            .to_string();
        assert!(error.contains("unknownRule"), "{error}");
        assert_eq!(std::fs::read_to_string(path).expect("read source"), raw);
    }

    #[test]
    fn saving_drops_unrecognized_keys_without_touching_known_ones() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
        let path = global_mcp_path(&cfg).expect("global config path");
        std::fs::create_dir_all(path.parent().expect("config parent")).expect("config parent");
        std::fs::write(
            &path,
            r#"{"$schema":"https://example.invalid/mcp.schema.json","mcpServers":{"browser":{"command":"node","args":["bridge"],"futureField":true}}}"#,
        )
        .expect("write config with unknown fields");

        assert!(set_global_tool_enabled(&cfg, "browser", "capture", false)
            .expect("save a known configuration mutation"));
        let saved: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(path).expect("read rewritten configuration"),
        )
        .expect("parse rewritten configuration");
        assert!(saved.get("$schema").is_none());
        assert!(saved["mcpServers"]["browser"].get("futureField").is_none());
        assert_eq!(saved["mcpServers"]["browser"]["command"], "node");
        assert_eq!(
            saved["mcpServers"]["browser"]["args"],
            serde_json::json!(["bridge"])
        );
        assert_eq!(
            saved["mcpServers"]["browser"]["toolFilter"]["exclude"],
            serde_json::json!(["capture"])
        );
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
        assert!(error.to_string().contains(&crate::infra::i18n::tr_in(
            crate::infra::i18n::Locale::En,
            "mcp.config.exists",
            &[("name", "same"), ("path", "")]
        )));
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
}
