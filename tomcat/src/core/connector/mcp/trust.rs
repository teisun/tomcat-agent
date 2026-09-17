use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::path::PathBuf;

use fs2::FileExt;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256, Sha512};

use crate::core::connector::mcp::config::{ConfiguredMcpServer, McpConfigSource};
use crate::infra::config::get_work_dir;
use crate::infra::error::AppError;
use crate::AppConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustDecision {
    Allowed,
    NeedsConfirmation,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustConfirmationReason {
    FirstSeen,
    LaunchChanged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SafeLaunchSnapshot {
    pub command: String,
    pub args: Vec<String>,
    #[serde(default)]
    pub url: Option<String>,
    pub cwd: Option<PathBuf>,
    pub has_redacted_arguments: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum TrustStatus {
    Trusted,
    NeedsConfirmation {
        reason: TrustConfirmationReason,
        previous: Option<SafeLaunchSnapshot>,
        current: Box<SafeLaunchSnapshot>,
        environment_changed: bool,
        hidden_argument_changed: bool,
    },
    Blocked,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TrustRecord {
    command_fingerprint: String,
    #[serde(default)]
    launch_snapshot: Option<SafeLaunchSnapshot>,
    #[serde(default)]
    command_args_cwd_fingerprint: Option<String>,
    #[serde(default)]
    denied: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct TrustFile {
    #[serde(default)]
    servers: BTreeMap<String, TrustRecord>,
}

pub struct TrustStore {
    path: PathBuf,
    lock_path: PathBuf,
    lock: Mutex<()>,
}

impl TrustStore {
    pub fn open(cfg: &AppConfig) -> Result<Self, AppError> {
        let path = get_work_dir(cfg)?.join("connector-trust.json");
        if path.exists() {
            let _ = read_trust_file(&path)?;
        }
        Ok(Self {
            lock_path: path.with_extension("lock"),
            path,
            lock: Mutex::new(()),
        })
    }

    pub fn decide(&self, server: &ConfiguredMcpServer) -> Result<TrustDecision, AppError> {
        match self.inspect(server)? {
            TrustStatus::Trusted => {
                let _guard = self.lock.lock();
                let _file_guard = self.acquire_file_lock()?;
                let mut state = self.read_file()?;
                if !state.servers.contains_key(&server.config_key) {
                    state
                        .servers
                        .insert(server.config_key.clone(), trust_record(server, false));
                    persist(&self.path, &state)?;
                }
                Ok(TrustDecision::Allowed)
            }
            TrustStatus::NeedsConfirmation { .. } => Ok(TrustDecision::NeedsConfirmation),
            TrustStatus::Blocked => Ok(TrustDecision::Blocked),
        }
    }

    pub fn inspect(&self, server: &ConfiguredMcpServer) -> Result<TrustStatus, AppError> {
        if !optional_integrity_matches(server)? {
            return Ok(TrustStatus::Blocked);
        }
        let fingerprint = command_fingerprint(server);
        let snapshot = safe_launch_snapshot(server);
        let command_args_cwd_fingerprint = command_args_cwd_fingerprint(server);
        let _guard = self.lock.lock();
        let _file_guard = self.acquire_file_lock()?;
        let state = self.read_file()?;
        match state.servers.get(&server.config_key) {
            Some(record) if record.denied => Ok(TrustStatus::Blocked),
            Some(record) if record.command_fingerprint == fingerprint => Ok(TrustStatus::Trusted),
            // A global connector is configured by this user, not by the opened
            // workspace. Keep it trusted after ordinary edits only until the user
            // explicitly denies it; that denial remains authoritative across edits.
            Some(_) if server.source == McpConfigSource::Global || server.config.trusted => {
                Ok(TrustStatus::Trusted)
            }
            Some(record) => Ok(TrustStatus::NeedsConfirmation {
                reason: TrustConfirmationReason::LaunchChanged,
                previous: record.launch_snapshot.clone(),
                environment_changed: record
                    .command_args_cwd_fingerprint
                    .as_deref()
                    .is_some_and(|previous| previous == command_args_cwd_fingerprint.as_str()),
                hidden_argument_changed: record
                    .launch_snapshot
                    .as_ref()
                    .is_some_and(|previous| previous == &snapshot)
                    && record
                        .command_args_cwd_fingerprint
                        .as_deref()
                        .is_some_and(|previous| previous != command_args_cwd_fingerprint.as_str()),
                current: Box::new(snapshot),
            }),
            None if server.source == McpConfigSource::Global || server.config.trusted => {
                Ok(TrustStatus::Trusted)
            }
            None => Ok(TrustStatus::NeedsConfirmation {
                reason: TrustConfirmationReason::FirstSeen,
                previous: None,
                current: Box::new(snapshot),
                environment_changed: false,
                hidden_argument_changed: false,
            }),
        }
    }

    pub fn approve(&self, server: &ConfiguredMcpServer) -> Result<(), AppError> {
        let _guard = self.lock.lock();
        let _file_guard = self.acquire_file_lock()?;
        let mut state = self.read_file()?;
        state
            .servers
            .insert(server.config_key.clone(), trust_record(server, false));
        persist(&self.path, &state)
    }

    pub fn deny(&self, server: &ConfiguredMcpServer) -> Result<(), AppError> {
        let _guard = self.lock.lock();
        let _file_guard = self.acquire_file_lock()?;
        let mut state = self.read_file()?;
        state
            .servers
            .insert(server.config_key.clone(), trust_record(server, true));
        persist(&self.path, &state)
    }

    fn acquire_file_lock(&self) -> Result<File, AppError> {
        let parent = self.lock_path.parent().ok_or_else(|| {
            AppError::Config("connector trust lock has no parent directory".to_string())
        })?;
        std::fs::create_dir_all(parent)?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&self.lock_path)?;
        file.lock_exclusive()?;
        Ok(file)
    }

    fn read_file(&self) -> Result<TrustFile, AppError> {
        read_trust_file(&self.path)
    }
}

pub fn command_fingerprint(server: &ConfiguredMcpServer) -> String {
    fingerprint_json(serde_json::json!({
        "command": server.config.command,
        "args": server.config.args,
        "url": server.config.url,
        "headers": server.config.headers,
        "oauthClientId": server.config.oauth.as_ref().and_then(|oauth| oauth.client_id.clone()),
        "env": server.config.env,
        "cwd": server.config.cwd,
    }))
}

fn command_args_cwd_fingerprint(server: &ConfiguredMcpServer) -> String {
    fingerprint_json(serde_json::json!({
        "command": server.config.command,
        "args": server.config.args,
        "url": server.config.url,
        "cwd": server.config.cwd,
    }))
}
fn fingerprint_json(payload: serde_json::Value) -> String {
    let encoded = serde_json::to_vec(&payload).expect("command fingerprint JSON serialization");
    let digest = Sha256::digest(encoded);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn trust_record(server: &ConfiguredMcpServer, denied: bool) -> TrustRecord {
    TrustRecord {
        command_fingerprint: command_fingerprint(server),
        launch_snapshot: Some(safe_launch_snapshot(server)),
        command_args_cwd_fingerprint: Some(command_args_cwd_fingerprint(server)),
        denied,
    }
}

fn safe_launch_snapshot(server: &ConfiguredMcpServer) -> SafeLaunchSnapshot {
    let (args, has_redacted_arguments) = redact_sensitive_arguments(&server.config.args);
    SafeLaunchSnapshot {
        command: server.config.command.clone(),
        args,
        url: server.config.url.clone(),
        cwd: server.config.cwd.clone(),
        has_redacted_arguments,
    }
}
fn redact_sensitive_arguments(args: &[String]) -> (Vec<String>, bool) {
    let mut redact_next = false;
    let mut redacted = false;
    let args = args
        .iter()
        .map(|argument| {
            if redact_next {
                redact_next = false;
                redacted = true;
                return "<redacted>".to_string();
            }
            if let Some((key, _)) = argument.split_once('=') {
                if is_sensitive_argument_key(key) {
                    redacted = true;
                    return format!("{key}=<redacted>");
                }
            }
            if is_sensitive_argument_key(argument) {
                redact_next = true;
                return argument.clone();
            }
            argument.clone()
        })
        .collect();
    (args, redacted)
}

fn is_sensitive_argument_key(argument: &str) -> bool {
    let key = argument
        .trim_start_matches('-')
        .to_ascii_lowercase()
        .replace('_', "-");
    [
        "api-key",
        "apikey",
        "authorization",
        "cookie",
        "credential",
        "password",
        "secret",
        "token",
    ]
    .iter()
    .any(|marker| key.contains(marker))
}

fn optional_integrity_matches(server: &ConfiguredMcpServer) -> Result<bool, AppError> {
    let Some(expected) = server.config.integrity.as_deref() else {
        return Ok(true);
    };
    let artifact = integrity_artifact(server).ok_or_else(|| {
        AppError::Config(format!(
            "MCP server '{}' configured integrity, but command/args do not reference a local preinstalled executable",
            server.name
        ))
    })?;
    let bytes = std::fs::read(&artifact).map_err(|error| {
        AppError::Config(format!(
            "read MCP integrity artifact '{}': {error}",
            artifact.display()
        ))
    })?;
    if let Some(expected_hex) = expected.strip_prefix("sha256:") {
        return Ok(hex_sha256(&bytes) == expected_hex.to_ascii_lowercase());
    }
    if let Some(expected_base64) = expected.strip_prefix("sha256-") {
        return Ok(base64_digest::<Sha256>(&bytes) == expected_base64);
    }
    if let Some(expected_base64) = expected.strip_prefix("sha512-") {
        return Ok(base64_digest::<Sha512>(&bytes) == expected_base64);
    }
    Err(AppError::Config(format!(
        "MCP server '{}' integrity must use sha256:<hex>, sha256-<base64>, or sha512-<base64>",
        server.name
    )))
}

fn integrity_artifact(server: &ConfiguredMcpServer) -> Option<std::path::PathBuf> {
    let cwd = server
        .config
        .cwd
        .as_deref()
        .unwrap_or_else(|| std::path::Path::new("."));
    std::iter::once(server.config.command.as_str())
        .chain(server.config.args.iter().map(String::as_str))
        .map(std::path::PathBuf::from)
        .map(|path| {
            if path.is_absolute() {
                path
            } else {
                cwd.join(path)
            }
        })
        .find(|path| path.is_file())
}

fn hex_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn base64_digest<D: Digest>(bytes: &[u8]) -> String {
    use base64::Engine;

    base64::engine::general_purpose::STANDARD.encode(D::digest(bytes))
}

fn read_trust_file(path: &std::path::Path) -> Result<TrustFile, AppError> {
    if !path.exists() {
        return Ok(TrustFile::default());
    }
    let content = std::fs::read_to_string(path)?;
    serde_json::from_str(&content).map_err(|error| {
        AppError::Config(format!(
            "parse connector trust store '{}': {error}",
            path.display()
        ))
    })
}

fn persist(path: &std::path::Path, state: &TrustFile) -> Result<(), AppError> {
    let contents = serde_json::to_vec_pretty(state)
        .map_err(|error| AppError::Config(format!("serialize connector trust store: {error}")))?;
    crate::infra::platform::write_file_atomic(path, &contents)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{TrustConfirmationReason, TrustDecision, TrustStatus, TrustStore};
    use crate::core::connector::mcp::config::{
        is_floating_npm_version, ConfiguredMcpServer, McpConfigSource, McpServerConfig, ToolFilter,
    };
    use crate::AppConfig;

    fn server(source: McpConfigSource, command: &str) -> ConfiguredMcpServer {
        ConfiguredMcpServer {
            config_key: format!("mcp:test:{}", source.wire_scope()),
            name: "browser".to_string(),
            config: McpServerConfig {
                command: command.to_string(),
                args: vec!["-y".to_string(), "browser-mcp".to_string()],
                env: Default::default(),
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
            source,
        }
    }

    #[test]
    fn deny_is_isolated_by_config_key_for_same_named_connectors() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
        let store = TrustStore::open(&cfg).expect("open trust store");
        let global = server(McpConfigSource::Global, "npx");
        let project = server(McpConfigSource::Project, "npx");

        store.deny(&global).expect("deny global connector");
        assert_eq!(
            store.inspect(&global).expect("inspect global"),
            TrustStatus::Blocked
        );
        assert!(matches!(
            store.inspect(&project).expect("inspect project"),
            TrustStatus::NeedsConfirmation { .. }
        ));
    }

    #[test]
    fn global_config_stays_trusted_after_edits_unless_explicitly_denied() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
        let store = TrustStore::open(&cfg).expect("open trust store");
        let configured = server(McpConfigSource::Global, "npx");

        assert_eq!(
            store.decide(&configured).expect("auto trust"),
            TrustDecision::Allowed
        );
        let changed = server(McpConfigSource::Global, "node");
        assert_eq!(
            store.decide(&changed).expect("global edits remain trusted"),
            TrustDecision::Allowed
        );

        store.deny(&changed).expect("explicit deny");
        assert_eq!(
            store.decide(&changed).expect("deny remains authoritative"),
            TrustDecision::Blocked
        );
        let changed_again = server(McpConfigSource::Global, "bun");
        assert_eq!(
            store
                .decide(&changed_again)
                .expect("deny survives config edits"),
            TrustDecision::Blocked
        );
    }

    #[test]
    fn configured_curated_server_is_trusted_by_default() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
        let store = TrustStore::open(&cfg).expect("open trust store");
        let mut configured = server(McpConfigSource::Project, "npx");
        configured.config.trusted = true;

        assert_eq!(
            store.decide(&configured).expect("configured trust"),
            TrustDecision::Allowed
        );
    }

    #[test]
    fn user_floating_version_warns_not_blocks() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
        let store = TrustStore::open(&cfg).expect("open trust store");
        let mut configured = server(McpConfigSource::Global, "npx");
        configured.config.args[1] = "browser-mcp@latest".to_string();

        assert!(is_floating_npm_version(&configured.config.args));
        assert_eq!(
            store
                .decide(&configured)
                .expect("floating versions remain allowed"),
            TrustDecision::Allowed
        );
    }

    #[test]
    fn command_change_surfaces_safe_diff_without_environment_values() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
        let store = TrustStore::open(&cfg).expect("open trust store");
        let configured = server(McpConfigSource::Project, "npx");
        store.approve(&configured).expect("record project trust");
        let mut changed = configured.clone();
        changed.config.command = "node".to_string();
        changed
            .config
            .env
            .insert("MCP_TOKEN".to_string(), "super-secret".to_string());

        let status = store.inspect(&changed).expect("inspect changed command");
        assert!(matches!(
            status,
            TrustStatus::NeedsConfirmation {
                reason: TrustConfirmationReason::LaunchChanged,
                environment_changed: false,
                ..
            }
        ));
        let serialized = serde_json::to_string(&status).expect("serialize trust status");
        assert!(serialized.contains("\"command\":\"node\""));
        assert!(!serialized.contains("super-secret"));
        assert!(!serialized.contains("MCP_TOKEN"));
    }

    #[test]
    fn environment_change_never_leaks_value() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
        let store = TrustStore::open(&cfg).expect("open trust store");
        let mut configured = server(McpConfigSource::Project, "npx");
        configured
            .config
            .env
            .insert("MCP_TOKEN".to_string(), "old-secret".to_string());
        store.approve(&configured).expect("record project trust");
        configured
            .config
            .env
            .insert("MCP_TOKEN".to_string(), "new-secret".to_string());

        let status = store
            .inspect(&configured)
            .expect("inspect changed environment");
        assert!(matches!(
            status,
            TrustStatus::NeedsConfirmation {
                environment_changed: true,
                hidden_argument_changed: false,
                ..
            }
        ));
        let serialized = serde_json::to_string(&status).expect("serialize trust status");
        assert!(!serialized.contains("old-secret"));
        assert!(!serialized.contains("new-secret"));
        assert!(!serialized.contains("MCP_TOKEN"));
    }

    #[test]
    fn sensitive_argument_values_are_redacted_in_trust_status() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
        let store = TrustStore::open(&cfg).expect("open trust store");
        let mut configured = server(McpConfigSource::Project, "npx");
        configured
            .config
            .args
            .extend(["--api-key".to_string(), "secret-value".to_string()]);

        let status = store.inspect(&configured).expect("inspect project server");
        let serialized = serde_json::to_string(&status).expect("serialize trust status");
        assert!(serialized.contains("<redacted>"));
        assert!(!serialized.contains("secret-value"));
    }

    #[test]
    fn project_config_requires_one_explicit_approval() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
        let store = TrustStore::open(&cfg).expect("open trust store");
        let configured = server(McpConfigSource::Project, "npx");

        assert_eq!(
            store.decide(&configured).expect("evaluate project config"),
            TrustDecision::NeedsConfirmation
        );
        store.approve(&configured).expect("approve project config");
        assert_eq!(
            store
                .decide(&configured)
                .expect("evaluate approved project config"),
            TrustDecision::Allowed
        );
    }

    #[test]
    fn configured_integrity_blocks_a_tampered_local_launcher() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let launcher = temp.path().join("launcher");
        std::fs::write(&launcher, "expected launcher bytes").expect("write launcher");
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
        let store = TrustStore::open(&cfg).expect("open trust store");
        let mut configured = server(McpConfigSource::Global, launcher.to_str().expect("UTF-8"));
        configured.config.integrity = Some("sha256:deadbeef".to_string());

        assert_eq!(
            store
                .decide(&configured)
                .expect("evaluate tampered launcher"),
            TrustDecision::Blocked
        );
    }
}
