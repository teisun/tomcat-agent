use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use dashmap::DashMap;
use parking_lot::RwLock;
use serde::Serialize;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tracing::warn;

pub use super::call::McpCallContext;
use super::call::{CallFailure, CallRuntime};
use super::recovery::{RecoveryBudget, RecoveryRun};
#[path = "connection.rs"]
mod connection;
#[path = "lifecycle.rs"]
mod lifecycle;
#[path = "login.rs"]
mod login;
use connection::{ConnectionId, ServiceOwner};
use lifecycle::ConnectionEvent;
pub use lifecycle::{RecoveryStatus, ReloadReceipt};

use crate::core::connector::mcp::config::{
    load_server_definitions, remove_global_server, remove_project_server, set_global_tool_enabled,
    set_global_tool_filter, set_project_tool_enabled, set_project_tool_filter, ConfiguredMcpServer,
    McpConfigSource, ToolFilter, ToolFilterMatcher,
};
use crate::core::connector::mcp::naming::to_model_name;
use crate::core::connector::mcp::oauth::OAuthTokenStore;
use crate::core::security::project_trust::ProjectTrustStore;
use crate::infra::error::AppError;
use crate::AppConfig;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerState {
    Pending,
    Connecting,
    Ready,
    Disconnected,
    AwaitingProjectTrust,
    NeedsAuthorization,
    Failed(String),
}

impl ServerState {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Connecting => "connecting",
            Self::Ready => "connected",
            Self::Disconnected => "disconnected",
            Self::AwaitingProjectTrust => "awaiting_project_trust",
            Self::NeedsAuthorization => "needs_authorization",
            Self::Failed(_) => "failed",
        }
    }

    pub fn display_label(&self) -> &'static str {
        match self {
            Self::Pending => "等待连接",
            Self::Connecting => "连接中",
            Self::Ready => "已连接",
            Self::Disconnected => "已断开",
            Self::AwaitingProjectTrust => "等待项目信任",
            Self::NeedsAuthorization => "需要授权",
            Self::Failed(_) => "失败",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ServerStatus {
    pub config_key: String,
    pub name: String,
    pub source: McpConfigSource,
    /// A same-named workspace definition is the effective connector. The
    /// Global definition remains visible for management but cannot run tools.
    pub overridden: bool,
    pub state: ServerState,
    pub tool_count: usize,
    pub resource_count: usize,
    pub generation: String,
    pub attempt: u8,
    pub recovery: Option<RecoveryStatus>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct McpToolDef {
    pub server: String,
    pub raw_name: String,
    pub model_name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

/// 对外的「工具来源」摘要。`source` 是元工具的通用词；本期它等于 MCP server 名，
/// 将来 CLI/A2A 等来源接入时不必改动 LLM 侧的 `tool_search` 参数。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct McpToolSource {
    pub name: String,
    #[serde(rename = "type")]
    pub source_type: &'static str,
    pub title: String,
    pub description: String,
    pub tool_count: usize,
}

/// schema 延迟披露前给模型看的最小工具卡片。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct McpToolSummary {
    pub name: String,
    pub description: String,
    #[serde(rename = "rawName")]
    pub raw_name: String,
    pub enabled: bool,
}

/// A catalog and the exact connection that produced it, captured atomically.
pub struct McpToolCatalog {
    pub config_key: String,
    pub generation: String,
    pub attempt: u8,
    pub tools: Vec<McpToolSummary>,
}

/// 关键词检索的命中项；`source` 保持元工具的通用术语。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct McpToolSearchMatch {
    pub name: String,
    pub source: String,
    pub description: String,
}

/// `describe_many` 不因一个未知名字丢弃其余结果。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct McpToolDescribeMany {
    pub tools: Vec<McpToolDef>,
    pub errors: Vec<McpToolLookupError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct McpToolLookupError {
    pub name: String,
    pub message: String,
}

#[derive(Debug)]
pub struct ToolEnableApplyError {
    pub config_saved: bool,
    pub error: AppError,
}

impl ToolEnableApplyError {
    fn not_saved(error: AppError) -> Self {
        Self {
            config_saved: false,
            error,
        }
    }

    fn saved(error: AppError) -> Self {
        Self {
            config_saved: true,
            error,
        }
    }
}

pub struct McpManager {
    entries: RwLock<BTreeMap<String, ConnectorEntry>>,
    /// Retains the newest incarnation after deletion so old async work cannot
    /// publish into a re-added configKey.
    generations: RwLock<BTreeMap<String, u64>>,
    runtime: crate::infra::config::McpRuntimeConfig,
    project_root: Option<std::path::PathBuf>,
    project_trusted: AtomicBool,
    project_trust: ProjectTrustStore,
    oauth_store: OAuthTokenStore,
    oauth_cancellations: DashMap<String, Arc<CancellationToken>>,
    workspace_root: Option<std::path::PathBuf>,
    self_weak: Weak<Self>,
    retirements: RwLock<BTreeMap<String, TaskTracker>>,
}

#[derive(Clone)]
struct ConnectorEntry {
    server: ConfiguredMcpServer,
    status: ServerStatus,
    connection: Option<Arc<ConnectedServer>>,
    generation: u64,
    budget: Option<RecoveryBudget>,
    run: Option<Arc<RecoveryRun>>,
    ready_since: Option<Instant>,
    recovery_phase: &'static str,
    suspended: bool,
    retiring: Option<TaskTracker>,
}

impl ConnectorEntry {
    fn new(server: ConfiguredMcpServer, status: ServerStatus, generation: u64) -> Self {
        Self {
            server,
            status,
            generation,
            connection: None,
            budget: None,
            run: None,
            ready_since: None,
            recovery_phase: "idle",
            suspended: false,
            retiring: None,
        }
    }
    fn retire_connection(&mut self) {
        self.status.tool_count = 0;
        self.status.resource_count = 0;
        self.ready_since = None;
        let connection = self.connection.take();
        let run = self.run.take();
        if connection.is_none() && run.is_none() {
            return;
        }
        if let Some(connection) = &connection {
            connection.retire();
        }
        if let Some(run) = &run {
            run.cancel.cancel();
        }
        let previous = self.retiring.take();
        let tracker = TaskTracker::new();
        let receipt = tracker.token();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _receipt = receipt;
                // Wait for the startup owner FIRST: it can register transport
                // close receipts while unwinding an interrupted initialization.
                if let Some(run) = run {
                    run.done.cancelled().await;
                    run.cleanup.close();
                    run.cleanup.wait().await;
                }
                if let Some(connection) = connection {
                    connection.wait_retired().await;
                }
                if let Some(previous) = previous {
                    previous.wait().await;
                }
            });
        }
        tracker.close();
        self.retiring = Some(tracker);
    }
}

impl Drop for McpManager {
    fn drop(&mut self) {
        for entry in self.entries.get_mut().values_mut() {
            entry.retire_connection();
        }
        for cancel in self.oauth_cancellations.iter() {
            cancel.value().cancel();
        }
    }
}

fn overridden_config_keys(servers: &[ConfiguredMcpServer]) -> BTreeSet<String> {
    let workspace_names = servers
        .iter()
        .filter(|server| server.source == McpConfigSource::Project)
        .map(|server| server.name.as_str())
        .collect::<BTreeSet<_>>();
    servers
        .iter()
        .filter(|server| {
            server.source == McpConfigSource::Global
                && workspace_names.contains(server.name.as_str())
        })
        .map(|server| server.config_key.clone())
        .collect()
}

struct ConnectedServer {
    id: ConnectionId,
    owner: ServiceOwner,
    all_tools: BTreeMap<String, McpToolDef>,
    title: Option<String>,
    instructions: Option<String>,
    calls: CallRuntime,
    resource_count: usize,
}

impl McpManager {
    pub fn new(
        cfg: &AppConfig,
        workspace_root: Option<&std::path::Path>,
    ) -> Result<Arc<Self>, AppError> {
        let runtime = cfg.connector.mcp;
        runtime.validate()?;
        let servers = load_server_definitions(cfg, workspace_root)?;
        let overridden = overridden_config_keys(&servers);
        let project_trust = ProjectTrustStore::open(cfg)?;
        let project_root = workspace_root
            .map(ProjectTrustStore::root_for)
            .transpose()?;
        let trusted = project_root.as_deref().is_some_and(|root| {
            project_trust.is_trusted(root).unwrap_or_else(|error| {
                warn!(error = %error, "project trust unreadable; denying project MCP");
                false
            })
        });
        let oauth_store = OAuthTokenStore::open(cfg)?;
        let entries = servers
            .into_iter()
            .map(|server| {
                let status = initial_server_status(
                    &server,
                    trusted,
                    overridden.contains(&server.config_key),
                );
                (
                    server.config_key.clone(),
                    ConnectorEntry::new(server, status, 0),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let generations = entries
            .keys()
            .map(|key| (key.clone(), 0))
            .collect::<BTreeMap<_, _>>();
        Ok(Arc::new_cyclic(|weak| Self {
            self_weak: weak.clone(),
            retirements: RwLock::new(BTreeMap::new()),
            entries: RwLock::new(entries),
            generations: RwLock::new(generations),
            runtime,
            project_root,
            project_trusted: AtomicBool::new(trusted),
            project_trust,
            oauth_store,
            workspace_root: workspace_root.map(std::path::Path::to_path_buf),
            oauth_cancellations: DashMap::new(),
        }))
    }

    pub fn statuses(&self) -> Vec<ServerStatus> {
        self.entries
            .read()
            .values()
            .map(|entry| {
                let mut status = entry.status.clone();
                status.generation = entry.generation.to_string();
                status.attempt = entry.budget.as_ref().map_or(0, |budget| budget.used);
                status.recovery = entry
                    .run
                    .as_ref()
                    .filter(|run| run.is_active())
                    .and(entry.budget.as_ref())
                    .map(|budget| RecoveryStatus {
                        phase: entry.recovery_phase,
                        max_attempts: budget.limit,
                        remaining_ms: budget.remaining_ms(),
                    });
                status
            })
            .collect()
    }

    /// Test-only lifecycle receipt: task count, free execution slots, free admissions.
    #[cfg(any(test, feature = "test-streamable-http-server"))]
    pub fn call_debug_counts(&self, identifier: &str) -> Option<(usize, usize, usize)> {
        let key = self.resolve_key(identifier).ok()?;
        let entries = self.entries.read();
        let calls = &entries.get(&key)?.connection.as_ref()?.calls;
        Some((
            calls.tasks.len(),
            calls.slots.available_permits(),
            calls.admitted.available_permits(),
        ))
    }

    #[cfg(any(test, feature = "test-streamable-http-server"))]
    pub fn retirement_debug_count(&self, identifier: &str) -> Option<usize> {
        let key = self.resolve_key(identifier).ok()?;
        let entries = self.entries.read();
        Some(
            entries
                .get(&key)?
                .retiring
                .as_ref()
                .map_or(0, TaskTracker::len),
        )
    }

    pub fn workspace_root(&self) -> Option<&std::path::Path> {
        self.workspace_root.as_deref()
    }

    pub fn project_root(&self) -> Option<&std::path::Path> {
        self.project_root.as_deref()
    }

    pub fn project_trusted(&self) -> bool {
        self.project_trusted.load(Ordering::Acquire)
    }

    pub fn project_trusted_for(&self, server: &ConfiguredMcpServer) -> bool {
        server.source == McpConfigSource::Global || self.project_trusted()
    }

    /// A user decision applies to all sources in this project; never revokes live work.
    pub fn wake_project(&self) -> Vec<String> {
        self.project_trusted.store(true, Ordering::Release);
        let mut entries = self.entries.write();
        entries
            .iter_mut()
            .filter_map(|(key, entry)| {
                if entry.server.source != McpConfigSource::Project
                    || entry.suspended
                    || entry.status.overridden
                    || entry.status.state != ServerState::AwaitingProjectTrust
                {
                    return None;
                }
                entry.status.state = ServerState::Pending;
                Some(key.clone())
            })
            .collect()
    }

    pub fn remove_configured_server(
        &self,
        identifier: &str,
        cfg: &AppConfig,
    ) -> Result<bool, AppError> {
        let key = self.resolve_key(identifier)?;
        self.cancel_login(&key);
        let server = self
            .configured_server(&key)
            .ok_or_else(|| AppError::Tool(format!("unknown MCP server: {identifier}")))?;
        let removed = match server.source {
            McpConfigSource::Global => remove_global_server(cfg, &server.name)?,
            McpConfigSource::Project => {
                let workspace_root = self.workspace_root.as_deref().ok_or_else(|| {
                    AppError::Config(
                        "project MCP configuration requires an explicit session project root"
                            .into(),
                    )
                })?;
                remove_project_server(cfg, workspace_root, &server.name)?
            }
        };
        if removed {
            // Deletion is a revocation boundary: cancel OAuth, erase stored
            // credentials, then retire this entry's generation.
            self.oauth_store.remove(&key)?;
            self.invalidate_generation(&key);
            if let Some(mut removed_entry) = self.entries.write().remove(&key) {
                removed_entry.retire_connection();
                if let Some(retiring) = removed_entry.retiring {
                    self.retirements.write().insert(key.clone(), retiring);
                }
            }
        }
        Ok(removed)
    }

    /// Changes one tool's desired effective state. A config write is a durable
    /// success even if this process cannot subsequently reload its cache; the
    /// caller needs that distinction to offer an idempotent retry.
    pub fn set_configured_tool_enabled(
        &self,
        identifier: &str,
        raw_name: &str,
        enabled: bool,
        cfg: &AppConfig,
    ) -> Result<(), ToolEnableApplyError> {
        let key = self
            .resolve_key(identifier)
            .map_err(ToolEnableApplyError::not_saved)?;
        self.ensure_runnable(&key)
            .map_err(ToolEnableApplyError::not_saved)?;
        let server = self
            .configured_server(&key)
            .ok_or_else(|| AppError::Tool(format!("unknown MCP server: {identifier}")))
            .map_err(ToolEnableApplyError::not_saved)?;
        let known_tool = self
            .entries
            .read()
            .get(&key)
            .and_then(|entry| entry.connection.as_ref())
            .is_some_and(|connection| {
                connection
                    .all_tools
                    .values()
                    .any(|tool| tool.raw_name == raw_name)
            });
        if !known_tool {
            return Err(ToolEnableApplyError::not_saved(AppError::Tool(format!(
                "unknown MCP tool '{raw_name}' for source '{identifier}'"
            ))));
        }
        match server.source {
            McpConfigSource::Global => {
                set_global_tool_enabled(cfg, &server.name, raw_name, enabled)
            }
            McpConfigSource::Project => {
                let workspace_root = self.workspace_root.as_deref().ok_or_else(|| {
                    AppError::Config(
                        "project MCP configuration requires an explicit session project root"
                            .into(),
                    )
                });
                workspace_root.and_then(|workspace_root| {
                    set_project_tool_enabled(cfg, workspace_root, &server.name, raw_name, enabled)
                })
            }
        }
        .map_err(ToolEnableApplyError::not_saved)?;
        self.reload_configuration(cfg)
            .map_err(ToolEnableApplyError::saved)?;
        Ok(())
    }

    pub fn set_configured_tool_filter(
        &self,
        identifier: &str,
        filter: ToolFilter,
        cfg: &AppConfig,
    ) -> Result<(), AppError> {
        let key = self.resolve_key(identifier)?;
        self.ensure_runnable(&key)?;
        let server = self
            .configured_server(&key)
            .ok_or_else(|| AppError::Tool(format!("unknown MCP server: {identifier}")))?;
        match server.source {
            McpConfigSource::Global => set_global_tool_filter(cfg, &server.name, filter)?,
            McpConfigSource::Project => {
                let workspace_root = self.workspace_root.as_deref().ok_or_else(|| {
                    AppError::Config(
                        "project MCP configuration requires an explicit session project root"
                            .into(),
                    )
                })?;
                set_project_tool_filter(cfg, workspace_root, &server.name, filter)?
            }
        }
        // A successful mutation must also update this manager before queued
        // calls can observe completion. Serve still synchronizes other cached
        // registries according to the existing Global/Workspace scope rules.
        self.reload_configuration(cfg)?;
        Ok(())
    }

    pub fn configured_server(&self, identifier: &str) -> Option<ConfiguredMcpServer> {
        let entries = self.entries.read();
        if let Some(entry) = entries.get(identifier) {
            return Some(entry.server.clone());
        }
        let mut matches = entries
            .values()
            .filter(|entry| entry.server.name == identifier);
        let entry = matches.next()?;
        matches.next().is_none().then(|| entry.server.clone())
    }

    fn resolve_key(&self, identifier: &str) -> Result<String, AppError> {
        let entries = self.entries.read();
        if entries.contains_key(identifier) {
            return Ok(identifier.to_string());
        }
        let matches = entries
            .iter()
            .filter(|(_, entry)| entry.server.name == identifier)
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [key] => Ok(key.clone()),
            [] => Err(AppError::Tool(format!("unknown MCP server: {identifier}"))),
            _ => Err(AppError::Tool(format!(
                "MCP server name '{identifier}' is ambiguous; use configKey"
            ))),
        }
    }

    pub fn configured_server_source(&self, identifier: &str) -> Result<McpConfigSource, AppError> {
        let key = self.resolve_key(identifier)?;
        self.configured_server(&key)
            .map(|server| server.source)
            .ok_or_else(|| AppError::Tool(format!("unknown MCP server: {identifier}")))
    }

    #[cfg(test)]
    fn current_generation(&self, key: &str) -> u64 {
        *self.generations.write().entry(key.to_string()).or_insert(0)
    }

    fn invalidate_generation(&self, key: &str) -> u64 {
        let mut generations = self.generations.write();
        let generation = generations.entry(key.to_string()).or_insert(0);
        *generation = generation.saturating_add(1);
        *generation
    }

    pub fn has_configured_servers(&self) -> bool {
        !self.entries.read().is_empty()
    }

    fn ensure_runnable(&self, key: &str) -> Result<(), AppError> {
        let entries = self.entries.read();
        let entry = entries
            .get(key)
            .ok_or_else(|| AppError::Tool(format!("unknown MCP server: {key}")))?;
        if entry.status.overridden {
            return Err(AppError::Tool(format!(
                "MCP server '{}' is overridden by the workspace connector with the same name",
                entry.server.name
            )));
        }
        Ok(())
    }

    pub fn tool_defs(&self, identifier: &str) -> Vec<McpToolDef> {
        let Ok(key) = self.resolve_key(identifier) else {
            return Vec::new();
        };
        let entries = self.entries.read();
        let Some(entry) = entries.get(&key).filter(|entry| !entry.status.overridden) else {
            return Vec::new();
        };
        let Some(connection) = entry.connection.as_ref() else {
            return Vec::new();
        };
        visible_tools(connection, &entry.server.config.tool_filter)
            .unwrap_or_default()
            .into_values()
            .collect()
    }

    /// L1：列出当前可调用的 MCP 来源。连接目录是唯一的运行时事实源，因此只返回
    /// Ready 的 server；BTreeMap 保证顺序稳定。
    pub fn list_servers(&self) -> Vec<McpToolSource> {
        self.entries
            .read()
            .values()
            .filter_map(|entry| {
                if entry.status.overridden {
                    return None;
                }
                let connection = entry.connection.as_ref()?;
                let visible = visible_tools(connection, &entry.server.config.tool_filter).ok()?;
                let config_origin = entry.server.source.as_str();
                Some(McpToolSource {
                    name: entry.server.name.clone(),
                    source_type: "mcp",
                    title: connection
                        .title
                        .clone()
                        .unwrap_or_else(|| entry.server.name.clone()),
                    description: connection
                        .instructions
                        .clone()
                        .or_else(|| tool_name_summary(&visible))
                        .unwrap_or_else(|| {
                            format!("MCP connector configured from {config_origin} mcp.json.")
                        }),
                    tool_count: visible.len(),
                })
            })
            .collect()
    }

    /// L2：列出允许 AI 发现和调用的工具卡片，不泄露 schema。
    ///
    /// Settings must use [`Self::tool_catalog_snapshot`] instead: the management
    /// catalog deliberately retains disabled tools so a user can re-enable them.
    pub fn list_tools(&self, identifier: &str) -> Result<Vec<McpToolSummary>, AppError> {
        Ok(self
            .tool_catalog_snapshot(identifier)?
            .tools
            .into_iter()
            .filter(|tool| tool.enabled)
            .collect())
    }

    /// Complete user-management catalog for one ready source. The same
    /// `ToolFilterMatcher` decides both the `enabled` flags here and every
    /// AI-callable projection below; this is not a second persisted catalog.
    pub fn tool_catalog_snapshot(&self, identifier: &str) -> Result<McpToolCatalog, AppError> {
        let key = self
            .resolve_key(identifier)
            .map_err(|_| AppError::Tool(format!("unknown MCP source: {identifier}")))?;
        self.ensure_runnable(&key)?;
        let (connection, filter) = {
            let entries = self.entries.read();
            let entry = entries
                .get(&key)
                .ok_or_else(|| AppError::Tool(format!("unknown MCP source: {identifier}")))?;
            if entry.status.overridden {
                return Err(AppError::Tool(
                    "MCP source is overridden by workspace configuration".into(),
                ));
            }
            (
                entry.connection.clone().ok_or_else(|| {
                    AppError::Tool(format!(
                        "MCP source '{identifier}' is not ready; use /connector list for status"
                    ))
                })?,
                entry.server.config.tool_filter.clone(),
            )
        };
        let matcher = ToolFilterMatcher::compile(&filter)?;
        let tools = connection
            .all_tools
            .values()
            .map(|tool| McpToolSummary {
                name: tool.model_name.clone(),
                raw_name: tool.raw_name.clone(),
                description: tool.description.clone(),
                enabled: matcher.allows(&tool.raw_name),
            })
            .collect();
        Ok(McpToolCatalog {
            config_key: key,
            generation: connection.id.generation.to_string(),
            attempt: connection.id.attempt,
            tools,
        })
    }

    pub fn search(
        &self,
        query: &str,
        source: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<McpToolSearchMatch>, AppError> {
        let query = query.trim();
        if query.is_empty() {
            return Err(AppError::Tool(
                "tool search query cannot be empty".to_string(),
            ));
        }
        let source_key = source
            .map(|identifier| {
                self.resolve_key(identifier)
                    .map_err(|_| AppError::Tool(format!("unknown MCP source: {identifier}")))
            })
            .transpose()?;
        let normalized_query = query.to_lowercase();
        let query_tokens = tokenize(query);
        let entries = self.entries.read();
        let mut matches = entries
            .iter()
            .filter(|(key, entry)| {
                source_key
                    .as_ref()
                    .is_none_or(|requested| requested == *key)
                    && !entry.status.overridden
                    && entry.connection.is_some()
            })
            .flat_map(|(_, entry)| {
                let connection = entry.connection.as_ref().expect("filtered connection");
                visible_tools(connection, &entry.server.config.tool_filter)
                    .unwrap_or_default()
                    .into_values()
                    .filter_map(|tool| {
                        let score = tool_search_score(&tool, &normalized_query, &query_tokens);
                        (score > 0).then(|| {
                            (
                                score,
                                McpToolSearchMatch {
                                    name: tool.model_name.clone(),
                                    source: entry.server.name.clone(),
                                    description: tool.description.clone(),
                                },
                            )
                        })
                    })
            })
            .collect::<Vec<_>>();
        matches.sort_by(|(left_score, left), (right_score, right)| {
            right_score
                .cmp(left_score)
                .then_with(|| left.name.cmp(&right.name))
        });
        Ok(matches
            .into_iter()
            .skip(offset)
            .take(limit)
            .map(|(_, item)| item)
            .collect())
    }

    pub fn describe_many(&self, names: &[String]) -> McpToolDescribeMany {
        let mut tools = Vec::new();
        let mut errors = Vec::new();
        for name in names {
            match self.lookup_tool(name) {
                Some(tool) => tools.push(tool),
                None => errors.push(McpToolLookupError {
                    name: name.clone(),
                    message: format!("unknown or not-ready deferred tool: {name}"),
                }),
            }
        }
        McpToolDescribeMany { tools, errors }
    }

    pub async fn call_model_tool(
        &self,
        model_tool_name: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        self.call_model_tool_with_context(model_tool_name, params, McpCallContext::default())
            .await
    }

    pub async fn call_model_tool_with_context(
        &self,
        model_tool_name: &str,
        params: serde_json::Value,
        context: McpCallContext,
    ) -> Result<serde_json::Value, AppError> {
        let tool = self.lookup_tool(model_tool_name).ok_or_else(|| {
            AppError::Tool(format!(
                "unknown or not-ready deferred tool: {model_tool_name}"
            ))
        })?;
        self.call_tool_with_context(&tool.server, &tool.model_name, params, context)
            .await
    }

    pub fn reload_configuration(&self, cfg: &AppConfig) -> Result<Vec<String>, AppError> {
        // Serialize the file snapshot with reconciliation. Never overwrite live
        // recovery/permission changes using an earlier clone of the entries map.
        let servers = load_server_definitions(cfg, self.workspace_root.as_deref())?;
        let disk_trusted = self.project_root.as_deref().is_some_and(|root| {
            self.project_trust.is_trusted(root).unwrap_or_else(|error| {
                warn!(error = %error, "project trust unreadable on reload; denying new project MCP");
                false
            })
        });
        if disk_trusted {
            self.wake_project();
        }
        let trusted = self.project_trusted();
        let mut entries = self.entries.write();
        let overridden = overridden_config_keys(&servers);
        let mut previous = std::mem::take(&mut *entries);
        let mut changed = Vec::new();
        for server in servers {
            let key = server.config_key.clone();
            let is_overridden = overridden.contains(&key);
            let mut old = previous.remove(&key);
            if old
                .as_ref()
                .is_some_and(|entry| same_transport_configuration(&entry.server, &server))
            {
                let mut retained = old.take().expect("retained entry checked");
                retained.server = server;
                if retained.status.overridden != is_overridden {
                    retained.retire_connection();
                    retained.generation = self.invalidate_generation(&key);
                    retained.budget = None;
                    retained.suspended = false;
                    retained.status =
                        initial_server_status(&retained.server, trusted, is_overridden);
                    if let Some(cancel) = self.oauth_cancellations.get(&key) {
                        cancel.cancel();
                    }
                    if !is_overridden {
                        changed.push(key.clone());
                    }
                } else if let Some(connection) = &retained.connection {
                    retained.status.tool_count =
                        visible_tools(connection, &retained.server.config.tool_filter)
                            .map_or(0, |tools| tools.len());
                }
                entries.insert(key, retained);
                continue;
            }
            // Replaced sources retain their cleanup fence just like a deleted
            // and immediately re-added source with the same configKey.
            let generation = self.invalidate_generation(&key);
            let status = initial_server_status(&server, trusted, is_overridden);
            let mut entry = ConnectorEntry::new(server, status, generation);
            entry.retiring = if let Some(mut replaced) = old {
                replaced.retire_connection();
                replaced.retiring.take()
            } else {
                self.retirements.write().remove(&key)
            };
            if let Some(cancel) = self.oauth_cancellations.get(&key) {
                cancel.cancel();
            }
            if !is_overridden {
                changed.push(key.clone());
            }
            entries.insert(key, entry);
        }
        for (key, mut removed) in previous {
            self.invalidate_generation(&key);
            if let Some(cancel) = self.oauth_cancellations.get(&key) {
                cancel.cancel();
            }
            removed.retire_connection();
            if let Some(retiring) = removed.retiring {
                self.retirements.write().insert(key, retiring);
            }
        }
        Ok(changed)
    }

    pub async fn call_tool(
        &self,
        identifier: &str,
        model_tool_name: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        self.call_tool_with_context(
            identifier,
            model_tool_name,
            params,
            McpCallContext::default(),
        )
        .await
    }

    pub async fn call_tool_with_context(
        &self,
        identifier: &str,
        model_tool_name: &str,
        params: serde_json::Value,
        mut context: McpCallContext,
    ) -> Result<serde_json::Value, AppError> {
        let entered = tokio::time::Instant::now();
        // Even callers reusing a context cannot let A's Drop cancel sibling B.
        context.cancel = context.cancel.child_token();
        let _cancel_on_drop = context.cancel.clone().drop_guard();
        let key = self.resolve_key(identifier)?;
        self.ensure_runnable(&key)?;
        let (connection, filter, generation) = {
            let entries = self.entries.read();
            let entry = entries.get(&key).ok_or_else(|| {
                AppError::Tool(format!(
                    "MCP server '{identifier}' is not ready; use /connector list for status"
                ))
            })?;
            (
                entry.connection.clone().ok_or_else(|| {
                    AppError::Tool(format!(
                        "MCP server '{identifier}' is not ready; use /connector list for status"
                    ))
                })?,
                entry.server.config.tool_filter.clone(),
                entry.generation,
            )
        };
        let tool = visible_tools(&connection, &filter)?
            .get(model_tool_name)
            .cloned()
            .ok_or_else(|| AppError::Tool(format!("unknown MCP tool: {model_tool_name}")))?;
        let arguments = params.as_object().cloned().ok_or_else(|| {
            AppError::Tool(format!(
                "MCP tool '{model_tool_name}' arguments must be a JSON object"
            ))
        })?;
        let request = rmcp::model::CallToolRequestParams::new(tool.raw_name.clone())
            .with_arguments(arguments);
        if connection.owner.peer.is_transport_closed() {
            self.connection_event(&key, connection.id, ConnectionEvent::SubmissionClosed);
            return Err(AppError::Tool(
                "MCP submission channel closed; request not sent".into(),
            ));
        }
        let deadline = entered
            .checked_add(connection.calls.timeout)
            .ok_or_else(|| AppError::Config("MCP admission timeout is too large".into()))?;
        let admission = connection
            .calls
            .admitted
            .clone()
            .try_acquire_owned()
            .map_err(|_| CallFailure::Busy.to_app_error(model_tool_name))?;
        let slot = tokio::select! {
            biased;
            _ = context.cancel.cancelled() => return Err(CallFailure::Cancelled { possibly_sent: false }.to_app_error(model_tool_name)),
            _ = connection.calls.retired.cancelled() => return Err(CallFailure::Retired { possibly_sent: false }.to_app_error(model_tool_name)),
            _ = tokio::time::sleep_until(deadline) => return Err(CallFailure::QueueTimeout.to_app_error(model_tool_name)),
            permit = connection.calls.slots.clone().acquire_owned() => permit
                .map_err(|_| CallFailure::Retired { possibly_sent: false }.to_app_error(model_tool_name))?,
        };
        let result = {
            // This short boundary grants permission to submit. Revocation and
            // replacement take the same entries lock; no network await here.
            let entries = self.entries.read();
            let current = entries.get(&key).ok_or_else(|| {
                CallFailure::Retired {
                    possibly_sent: false,
                }
                .to_app_error(model_tool_name)
            })?;
            if current.generation != generation
                || current.status.overridden
                || !current
                    .connection
                    .as_ref()
                    .is_some_and(|live| Arc::ptr_eq(live, &connection))
                || context.cancel.is_cancelled()
                || connection.calls.retired.is_cancelled()
            {
                return Err(CallFailure::Retired {
                    possibly_sent: false,
                }
                .to_app_error(model_tool_name));
            }
            if !visible_tools(&connection, &current.server.config.tool_filter)?
                .contains_key(model_tool_name)
            {
                return Err(AppError::Tool(
                    "MCP request no longer permitted; request not sent".into(),
                ));
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(CallFailure::QueueTimeout.to_app_error(model_tool_name));
            }
            connection.calls.spawn(
                connection.owner.peer.clone(),
                rmcp::model::ClientRequest::CallToolRequest(rmcp::model::Request::new(request)),
                context,
                deadline,
                slot,
                admission,
            )
        };
        let result = result.await.map_err(|_| {
            AppError::Tool(
                "MCP request task ended without a result; execution result unknown".into(),
            )
        })?;
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                // TransportClosed by itself can describe only one response
                // stream. Require an explicit shared-connection fact.
                if connection.owner.peer.is_transport_closed() {
                    self.connection_event(&key, connection.id, ConnectionEvent::SubmissionClosed);
                } else if error.session_expired() {
                    self.connection_event(&key, connection.id, ConnectionEvent::SessionExpired);
                } else if let Some(facts) = error
                    .facts()
                    .filter(|facts| facts.kind == super::failure::FailureKind::Authorization)
                {
                    self.authorization_failure(&key, connection.id, facts);
                }
                return Err(error.to_app_error(model_tool_name));
            }
        };
        serde_json::to_value(result)
            .map_err(|_| AppError::Tool("serialize MCP tool result failed".into()))
    }

    pub async fn connect_all(&self) {
        for status in self
            .statuses()
            .into_iter()
            .filter(|status| !status.overridden)
        {
            if let Err(error) = self.connect_server(&status.config_key).await {
                warn!(server = %status.name, error = %error, "MCP server did not become ready");
            }
        }
    }

    fn update_state(&self, key: &str, state: ServerState, tool_count: usize) {
        if let Some(entry) = self.entries.write().get_mut(key) {
            if !matches!(&state, ServerState::Ready) {
                entry.status.resource_count = 0;
            }
            entry.status.state = state;
            entry.status.tool_count = tool_count;
        }
    }

    #[cfg(test)]
    fn update_state_if_generation(
        &self,
        key: &str,
        generation: u64,
        state: ServerState,
        tool_count: usize,
    ) {
        let mut entries = self.entries.write();
        let Some(entry) = entries.get_mut(key) else {
            return;
        };
        if entry.generation != generation {
            return;
        }
        if !matches!(&state, ServerState::Ready) {
            entry.status.resource_count = 0;
        }
        entry.status.state = state;
        entry.status.tool_count = tool_count;
    }

    #[cfg(test)]
    fn mark_disconnected(&self, key: &str, generation: u64) {
        let mut entries = self.entries.write();
        let Some(entry) = entries.get_mut(key) else {
            return;
        };
        if entry.generation != generation {
            return;
        }
        entry.retire_connection();
        entry.status.resource_count = 0;
        entry.status.state = ServerState::Disconnected;
        entry.status.tool_count = 0;
    }

    fn lookup_tool(&self, model_tool_name: &str) -> Option<McpToolDef> {
        self.entries
            .read()
            .values()
            .filter(|entry| !entry.status.overridden)
            .find_map(|entry| {
                let connection = entry.connection.as_ref()?;
                visible_tools(connection, &entry.server.config.tool_filter)
                    .ok()?
                    .get(model_tool_name)
                    .cloned()
            })
    }
}

fn same_transport_configuration(
    previous: &ConfiguredMcpServer,
    next: &ConfiguredMcpServer,
) -> bool {
    if previous.source != next.source {
        return false;
    }
    let mut previous_config = previous.config.clone();
    previous_config.tool_filter = next.config.tool_filter.clone();
    previous_config == next.config
}

fn visible_tools(
    connection: &ConnectedServer,
    filter: &ToolFilter,
) -> Result<BTreeMap<String, McpToolDef>, AppError> {
    let matcher = ToolFilterMatcher::compile(filter)?;
    Ok(connection
        .all_tools
        .iter()
        .filter(|(_, tool)| matcher.allows(&tool.raw_name))
        .map(|(name, tool)| (name.clone(), tool.clone()))
        .collect())
}
fn tokenize(value: &str) -> BTreeSet<String> {
    let mut tokens = BTreeSet::new();
    let mut current = String::new();
    let mut previous_was_lower_or_digit = false;
    for character in value.chars() {
        if character.is_alphanumeric() {
            if character.is_uppercase() && previous_was_lower_or_digit && !current.is_empty() {
                tokens.insert(std::mem::take(&mut current));
            }
            current.extend(character.to_lowercase());
            previous_was_lower_or_digit = character.is_lowercase() || character.is_numeric();
        } else {
            if !current.is_empty() {
                tokens.insert(std::mem::take(&mut current));
            }
            previous_was_lower_or_digit = false;
        }
    }
    if !current.is_empty() {
        tokens.insert(current);
    }
    tokens
}

fn tool_search_score(
    tool: &McpToolDef,
    normalized_query: &str,
    query_tokens: &BTreeSet<String>,
) -> usize {
    let normalized_name = tool.model_name.to_lowercase();
    let normalized_description = tool.description.to_lowercase();
    let name_tokens = tokenize(&tool.model_name);
    let description_tokens = tokenize(&tool.description);
    let name_overlap = query_tokens.intersection(&name_tokens).count();
    let description_overlap = query_tokens.intersection(&description_tokens).count();

    let mut score = 0;
    if normalized_name.contains(normalized_query) {
        score += 16;
    }
    if normalized_description.contains(normalized_query) {
        score += 8;
    }
    score + name_overlap * 4 + description_overlap * 2
}

fn initial_server_status(
    server: &ConfiguredMcpServer,
    trusted: bool,
    overridden: bool,
) -> ServerStatus {
    ServerStatus {
        config_key: server.config_key.clone(),
        name: server.name.clone(),
        source: server.source,
        overridden,
        state: if server.source == McpConfigSource::Project && !trusted && !overridden {
            ServerState::AwaitingProjectTrust
        } else {
            ServerState::Pending
        },
        tool_count: 0,
        resource_count: 0,
        generation: "0".into(),
        attempt: 0,
        recovery: None,
    }
}

fn parse_tools(
    server: &str,
    listed: &serde_json::Value,
) -> Result<BTreeMap<String, McpToolDef>, AppError> {
    let tools = listed
        .get("tools")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            AppError::Tool(format!("MCP server '{server}' returned invalid tools/list"))
        })?;
    let mut definitions = BTreeMap::new();
    for tool in tools {
        let raw_name = tool
            .get("name")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                AppError::Tool(format!(
                    "MCP server '{server}' returned a tool without name"
                ))
            })?;
        let model_name = to_model_name(server, raw_name);
        definitions.insert(
            model_name.clone(),
            McpToolDef {
                server: server.to_string(),
                raw_name: raw_name.to_string(),
                model_name,
                description: model_description(
                    server,
                    raw_name,
                    tool.get("description")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default(),
                ),
                input_schema: tool
                    .get("inputSchema")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({"type": "object"})),
            },
        );
    }
    Ok(definitions)
}

fn model_description(server: &str, raw_name: &str, description: &str) -> String {
    if server == "playwright" && raw_name == "browser_take_screenshot" {
        format!(
            "{description}\n\nFor visual inspection by the model: omit filename and omit fullPage=true. The current Playwright MCP server saves named/full-page screenshots to disk and returns only a file link, not an image content block."
        )
    } else {
        description.to_string()
    }
}

fn tool_name_summary(tools: &BTreeMap<String, McpToolDef>) -> Option<String> {
    let names = tools
        .values()
        .map(|tool| tool.raw_name.as_str())
        .take(3)
        .collect::<Vec<_>>();
    if names.is_empty() {
        return None;
    }
    let remaining = tools.len().saturating_sub(names.len());
    let suffix = (remaining > 0).then(|| format!(", and {remaining} more"));
    Some(format!(
        "Provides tools: {}{}.",
        names.join(", "),
        suffix.unwrap_or_default()
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use std::collections::{BTreeMap, BTreeSet};

    use super::{
        tokenize, tool_name_summary, tool_search_score, McpManager, McpToolDef, ServerState,
    };
    use crate::core::connector::mcp::config::{add_global_server, McpServerConfig};
    use crate::infra::config::get_work_dir;
    use crate::AppConfig;

    #[test]
    fn manager_uses_one_runtime_for_global_and_workspace_sources() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("project");
        std::fs::create_dir_all(workspace.join(".agents")).unwrap();
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
        cfg.connector.mcp.startup_timeout_ms = 31_000;
        cfg.connector.mcp.call_timeout_ms = 91_000;
        cfg.connector.mcp.max_concurrent_calls = 3;
        add_global_server(
            &cfg,
            "global".into(),
            serde_json::from_value(serde_json::json!({"command":"node"})).unwrap(),
        )
        .unwrap();
        std::fs::write(
            workspace.join(".agents/mcp.json"),
            r#"{"mcpServers":{"project":{"command":"node"}}}"#,
        )
        .unwrap();
        let manager = McpManager::new(&cfg, Some(&workspace)).unwrap();
        assert_eq!(manager.statuses().len(), 2);
        assert_eq!(manager.runtime, cfg.connector.mcp);
        let mut invalid = cfg.clone();
        invalid.connector.mcp.max_concurrent_calls = 0;
        assert!(McpManager::new(&invalid, Some(&workspace)).is_err());
    }

    pub(super) fn manager_with_fake_server(
        args: Vec<String>,
        call_timeout_ms: u64,
    ) -> (tempfile::TempDir, Arc<McpManager>) {
        let temp = tempfile::tempdir().expect("temporary directory");
        let workspace = temp.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
        cfg.connector.mcp.call_timeout_ms = call_timeout_ms;
        let config_path = get_work_dir(&cfg).expect("work dir").join("mcp.json");
        std::fs::create_dir_all(config_path.parent().expect("config parent"))
            .expect("config directory");
        std::fs::write(
            config_path,
            serde_json::json!({
                "mcpServers": {
                    "fake": {
                        "command": "node",
                        "args": args,
                    }
                }
            })
            .to_string(),
        )
        .expect("write MCP config");
        let manager = McpManager::new(&cfg, Some(&workspace)).expect("construct MCP manager");
        (temp, manager)
    }

    fn manager_with_untrusted_project_server() -> (tempfile::TempDir, Arc<McpManager>) {
        let temp = tempfile::tempdir().expect("temporary directory");
        let workspace = temp.path().join("workspace");
        let config_path = workspace.join(".agents/mcp.json");
        std::fs::create_dir_all(config_path.parent().expect("project config parent"))
            .expect("project config directory");
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/mcp/fake_stdio_server.mjs");
        std::fs::write(
            config_path,
            serde_json::json!({
                "mcpServers": {
                    "project-fake": {
                        "command": "node",
                        "args": [fixture],
                    }
                }
            })
            .to_string(),
        )
        .expect("write untrusted project MCP config");

        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
        let manager = McpManager::new(&cfg, Some(&workspace)).expect("construct MCP manager");
        (temp, manager)
    }

    pub(super) fn fake_server_args(extra: &[String]) -> Vec<String> {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/mcp/fake_stdio_server.mjs");
        std::iter::once(fixture.to_string_lossy().into_owned())
            .chain(extra.iter().cloned())
            .collect()
    }

    #[tokio::test]
    async fn same_name_global_and_workspace_connectors_have_distinct_config_keys() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let workspace = temp.path().join("workspace");
        let global_path = temp.path().join("work/mcp.json");
        let project_path = workspace.join(".agents/mcp.json");
        std::fs::create_dir_all(global_path.parent().expect("global parent"))
            .expect("global parent");
        std::fs::create_dir_all(project_path.parent().expect("project parent"))
            .expect("project parent");
        std::fs::write(
            &global_path,
            serde_json::json!({ "mcpServers": { "shared": { "command": "node" } } }).to_string(),
        )
        .expect("global config");
        std::fs::write(
            &project_path,
            serde_json::json!({ "mcpServers": { "shared": { "command": "bun" } } }).to_string(),
        )
        .expect("project config");
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
        let manager = McpManager::new(&cfg, Some(&workspace)).expect("manager");
        let statuses = manager.statuses();
        assert_eq!(statuses.len(), 2);
        let global = statuses
            .iter()
            .find(|status| {
                status.source == crate::core::connector::mcp::config::McpConfigSource::Global
            })
            .expect("global status");
        let workspace_status = statuses
            .iter()
            .find(|status| {
                status.source == crate::core::connector::mcp::config::McpConfigSource::Project
            })
            .expect("workspace status");
        let global_key = global.config_key.clone();
        assert!(global.overridden);
        assert!(!workspace_status.overridden);
        assert!(manager.configured_server("shared").is_none());
        assert!(manager.configured_server(&global_key).is_some());
        assert!(manager
            .configured_server(&workspace_status.config_key)
            .is_some());
        assert!(manager.connect_server(&global_key).await.is_err());
        assert!(manager.list_tools(&global_key).is_err());

        assert!(manager
            .remove_configured_server(&workspace_status.config_key, &cfg)
            .expect("remove workspace override"));
        assert_eq!(
            manager
                .reload_configuration(&cfg)
                .expect("reload after removal"),
            vec![global_key.clone()],
        );
        assert!(manager
            .statuses()
            .into_iter()
            .find(|status| status.config_key == global_key)
            .is_some_and(|status| !status.overridden));
    }

    #[test]
    fn removal_revokes_credentials_and_readd_uses_a_new_generation() {
        let (temp, manager) = manager_with_fake_server(fake_server_args(&[]), 120_000);
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
        let configured = manager.configured_server("fake").expect("configured fake");
        let key = configured.config_key.clone();
        let old_generation = manager
            .entries
            .read()
            .get(&key)
            .expect("original entry")
            .generation;
        manager
            .save_static_bearer(&key, "secret".to_string(), None)
            .expect("save token");
        assert!(manager
            .oauth_store
            .load(&key)
            .expect("load token")
            .is_some());

        assert!(manager
            .remove_configured_server(&key, &cfg)
            .expect("remove connector"));
        assert!(manager
            .oauth_store
            .load(&key)
            .expect("revoked token")
            .is_none());
        assert!(manager.entries.read().get(&key).is_none());

        add_global_server(&cfg, "fake".to_string(), configured.config).expect("re-add connector");
        manager
            .reload_configuration(&cfg)
            .expect("reload re-added connector");
        assert!(
            manager
                .entries
                .read()
                .get(&key)
                .is_some_and(|entry| entry.generation > old_generation),
            "a re-added configKey must reject any pre-removal connection result",
        );
    }

    #[test]
    fn removed_source_cannot_publish_old_generation() {
        let (temp, manager) = manager_with_fake_server(fake_server_args(&[]), 120_000);
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
        let key = manager.configured_server("fake").unwrap().config_key;
        let old_generation = manager.current_generation(&key);
        assert!(manager.remove_configured_server(&key, &cfg).unwrap());
        manager.update_state_if_generation(&key, old_generation, ServerState::Ready, 2);
        assert!(manager.entries.read().get(&key).is_none());
        assert!(manager.current_generation(&key) > old_generation);
    }

    #[test]
    fn configuration_reload_retirement_prevents_removed_key_reuse() {
        let (temp, manager) = manager_with_fake_server(fake_server_args(&[]), 120_000);
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
        let configured = manager.configured_server("fake").expect("configured fake");
        let key = configured.config_key.clone();
        let old_generation = manager.current_generation(&key);

        std::fs::write(
            get_work_dir(&cfg).expect("work directory").join("mcp.json"),
            r#"{"mcpServers":{}}"#,
        )
        .expect("remove the config entry externally");
        manager
            .reload_configuration(&cfg)
            .expect("reload removed configuration");
        assert!(manager.entries.read().get(&key).is_none());

        add_global_server(&cfg, "fake".to_string(), configured.config).expect("re-add connector");
        manager
            .reload_configuration(&cfg)
            .expect("reload re-added configuration");
        assert!(manager
            .entries
            .read()
            .get(&key)
            .is_some_and(|entry| entry.generation > old_generation));
    }

    #[tokio::test]
    async fn fake_stdio_server_lists_and_calls_tools() {
        let (_temp, manager) = manager_with_fake_server(fake_server_args(&[]), 120_000);
        manager
            .connect_server("fake")
            .await
            .expect("connect fake server");
        let status = manager.statuses().pop().expect("server status");
        assert_eq!(status.state, ServerState::Ready);
        assert_eq!(status.tool_count, 2);
        let tool = manager
            .tool_defs("fake")
            .into_iter()
            .find(|tool| tool.raw_name == "capture")
            .expect("capture tool");
        assert_eq!(tool.model_name, "mcp__fake__capture");
        let result = manager
            .call_tool("fake", &tool.model_name, serde_json::json!({}))
            .await
            .expect("call fake tool");
        assert_eq!(result["content"][0]["text"], "fake capture complete");
        assert_eq!(result["content"][1]["type"], "image");
    }

    #[tokio::test]
    async fn reload_keeps_an_unchanged_ready_connection_when_another_server_is_added() {
        let (temp, manager) = manager_with_fake_server(fake_server_args(&[]), 120_000);
        manager
            .connect_server("fake")
            .await
            .expect("connect original server");
        let fake_key = manager
            .configured_server("fake")
            .expect("configured fake server")
            .config_key;
        let original = manager
            .entries
            .read()
            .get(&fake_key)
            .and_then(|entry| entry.connection.clone())
            .expect("ready original connection");
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
        let added: McpServerConfig = serde_json::from_value(serde_json::json!({
            "command": "node", "args": ["does-not-start"]
        }))
        .expect("added server configuration");
        add_global_server(&cfg, "new-server".to_string(), added).expect("add another server");

        let changed = manager
            .reload_configuration(&cfg)
            .expect("incremental reload");
        assert_eq!(
            changed,
            vec![
                manager
                    .configured_server("new-server")
                    .expect("new server")
                    .config_key,
            ]
        );
        let retained = manager
            .entries
            .read()
            .get(&fake_key)
            .and_then(|entry| entry.connection.clone())
            .expect("unchanged connection stays ready");
        assert!(Arc::ptr_eq(&original, &retained));
        assert_eq!(manager.tool_defs("fake").len(), 2);
    }

    #[tokio::test]
    async fn changing_tool_filter_keeps_the_transport_and_updates_visible_tools() {
        let (temp, manager) = manager_with_fake_server(fake_server_args(&[]), 120_000);
        manager
            .connect_server("fake")
            .await
            .expect("connect fake server");
        let key = manager
            .configured_server("fake")
            .expect("configured fake server")
            .config_key;
        let original = manager
            .entries
            .read()
            .get(&key)
            .and_then(|entry| entry.connection.clone())
            .expect("ready connection");
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());

        manager
            .set_configured_tool_filter(
                &key,
                crate::core::connector::mcp::config::ToolFilter {
                    include: vec!["capture".to_string()],
                    exclude: vec![],
                },
                &cfg,
            )
            .expect("save tool filter");
        assert!(manager
            .reload_configuration(&cfg)
            .expect("reload tool filter")
            .is_empty());

        let retained = manager
            .entries
            .read()
            .get(&key)
            .and_then(|entry| entry.connection.clone())
            .expect("filter-only update retains connection");
        assert!(Arc::ptr_eq(&original, &retained));
        assert_eq!(manager.list_tools(&key).expect("filtered tools").len(), 1);
        assert_eq!(manager.tool_defs(&key).len(), 1);
        assert_eq!(manager.list_servers()[0].tool_count, 1);
        let management = manager
            .tool_catalog_snapshot(&key)
            .expect("complete management catalog");
        assert_eq!(
            management
                .tools
                .into_iter()
                .map(|tool| (tool.raw_name, tool.enabled))
                .collect::<Vec<_>>(),
            vec![("capture".to_string(), true), ("status".to_string(), false)],
            "Settings keeps the disabled row while every AI-facing projection remains filtered"
        );
    }

    #[tokio::test]
    async fn single_tool_toggle_updates_the_catalog_and_blocks_model_calls() {
        let (temp, manager) = manager_with_fake_server(fake_server_args(&[]), 120_000);
        manager
            .connect_server("fake")
            .await
            .expect("connect fake server");
        let key = manager
            .configured_server("fake")
            .expect("configured fake server")
            .config_key;
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());

        manager
            .set_configured_tool_enabled(&key, "capture", false, &cfg)
            .expect("disable one tool");
        let catalog = manager
            .tool_catalog_snapshot(&key)
            .expect("complete management catalog");
        assert_eq!(
            catalog
                .tools
                .into_iter()
                .map(|tool| (tool.raw_name, tool.enabled))
                .collect::<Vec<_>>(),
            vec![("capture".to_string(), false), ("status".to_string(), true)]
        );
        manager
            .call_model_tool("mcp__fake__capture", serde_json::json!({}))
            .await
            .expect_err("disabled tool must not be callable by the model");

        manager
            .set_configured_tool_enabled(&key, "capture", true, &cfg)
            .expect("re-enable one tool");
        assert_eq!(
            manager
                .call_model_tool("mcp__fake__capture", serde_json::json!({}))
                .await
                .expect("re-enabled tool must be callable")["content"][0]["text"],
            "fake capture complete"
        );
    }

    #[tokio::test]
    async fn deferred_catalog_lists_searches_describes_and_calls_without_registry() {
        let (_temp, manager) = manager_with_fake_server(fake_server_args(&[]), 120_000);
        manager
            .connect_server("fake")
            .await
            .expect("connect fake server");

        assert_eq!(
            manager.list_servers(),
            vec![super::McpToolSource {
                name: "fake".to_string(),
                source_type: "mcp",
                title: "tomcat-fake-mcp".to_string(),
                description: "Use fake tools for connector tests.".to_string(),
                tool_count: 2,
            }]
        );
        assert_eq!(
            manager
                .list_tools("fake")
                .expect("list ready source")
                .into_iter()
                .map(|tool| tool.name)
                .collect::<Vec<_>>(),
            vec![
                "mcp__fake__capture".to_string(),
                "mcp__fake__status".to_string()
            ]
        );

        let matches = manager
            .search("capture", Some("fake"), 20, 0)
            .expect("search ready source");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].name, "mcp__fake__capture");
        assert_eq!(matches[0].source, "fake");
        let second_page = manager
            .search("fake", Some("fake"), 1, 1)
            .expect("page through deterministic results");
        assert_eq!(second_page.len(), 1);
        assert_eq!(second_page[0].name, "mcp__fake__capture");

        let names = vec![
            "mcp__fake__capture".to_string(),
            "mcp__fake__missing".to_string(),
            "mcp__fake__status".to_string(),
        ];
        let described = manager.describe_many(&names);
        assert_eq!(
            described
                .tools
                .iter()
                .map(|tool| tool.model_name.as_str())
                .collect::<Vec<_>>(),
            vec!["mcp__fake__capture", "mcp__fake__status"],
            "known schemas preserve caller order while unknown names do not poison the batch"
        );
        assert_eq!(described.errors.len(), 1);
        assert_eq!(described.errors[0].name, "mcp__fake__missing");

        let result = manager
            .call_model_tool("mcp__fake__capture", serde_json::json!({}))
            .await
            .expect("call canonical deferred name");
        assert_eq!(result["content"][0]["text"], "fake capture complete");
    }

    #[tokio::test]
    async fn deferred_catalog_reports_unknown_and_not_ready_sources() {
        let (_temp, manager) = manager_with_fake_server(fake_server_args(&[]), 120_000);
        assert!(manager.list_servers().is_empty());
        assert!(manager
            .list_tools("fake")
            .expect_err("configured but not-ready source must be actionable")
            .to_string()
            .contains("not ready"));
        assert!(manager
            .list_tools("missing")
            .expect_err("unknown source must not look like an empty source")
            .to_string()
            .contains("unknown MCP source"));
        assert!(manager
            .search("capture", Some("missing"), 20, 0)
            .expect_err("scoped search validates source")
            .to_string()
            .contains("unknown MCP source"));
    }

    #[tokio::test]
    async fn deferred_call_cannot_bypass_untrusted_project_connector() {
        let (_temp, manager) = manager_with_untrusted_project_server();

        manager
            .connect_server("project-fake")
            .await
            .expect("untrusted source is recorded as awaiting confirmation, not connected");
        assert!(matches!(
            manager.statuses().pop().expect("source status").state,
            ServerState::AwaitingProjectTrust
        ));
        assert!(manager.list_servers().is_empty());

        let error = manager
            .call_model_tool("mcp__project-fake__capture", serde_json::json!({}))
            .await
            .expect_err("tool_call must not bypass project connector confirmation");
        assert!(
            error
                .to_string()
                .contains("unknown or not-ready deferred tool"),
            "unapproved sources must not expose a callable deferred tool: {error}"
        );
    }

    #[tokio::test]
    async fn project_trust_gates_execution_once_and_survives_configuration_edits() {
        let (temp, manager) = manager_with_untrusted_project_server();
        let root = manager.project_root().unwrap().to_path_buf();
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
        let store = crate::core::security::project_trust::ProjectTrustStore::open(&cfg).unwrap();
        assert!(!manager.project_trusted());
        assert_eq!(
            manager.statuses()[0].state,
            ServerState::AwaitingProjectTrust
        );
        manager.connect_server("project-fake").await.unwrap();
        assert!(manager.test_server("project-fake").await.is_err());
        assert!(manager.login_server("project-fake").await.is_err());
        assert!(manager.list_servers().is_empty());

        store.trust(&root).unwrap();
        assert_eq!(manager.wake_project().len(), 1);
        assert!(manager.wake_project().is_empty(), "idempotent wake");
        assert_eq!(manager.statuses()[0].state, ServerState::Pending);
        manager.connect_server("project-fake").await.unwrap();
        assert_eq!(manager.statuses()[0].state, ServerState::Ready);

        let project_path = root.join(".agents/mcp.json");
        let mut file: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&project_path).unwrap()).unwrap();
        file["mcpServers"]["project-fake"]["env"] = serde_json::json!({"RECONFIGURED":"yes"});
        std::fs::write(&project_path, file.to_string()).unwrap();
        manager.reload_configuration(&cfg).unwrap();
        manager.connect_server("project-fake").await.unwrap();
        assert_eq!(manager.statuses()[0].state, ServerState::Ready);

        let other = temp.path().join("other");
        std::fs::create_dir_all(&other).unwrap();
        let unrelated = McpManager::new(&cfg, Some(&other)).unwrap();
        assert!(!unrelated.project_trusted());
    }

    #[test]
    fn corrupt_project_trust_file_cannot_approve_workspace_sources() {
        let (temp, manager) = manager_with_untrusted_project_server();
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
        let store = crate::core::security::project_trust::ProjectTrustStore::open(&cfg).unwrap();
        std::fs::create_dir_all(store.path().parent().unwrap()).unwrap();
        std::fs::write(store.path(), b"not json").unwrap();
        let second = McpManager::new(&cfg, manager.workspace_root()).unwrap();
        assert!(!second.project_trusted());
        assert_eq!(
            second.statuses()[0].state,
            ServerState::AwaitingProjectTrust
        );
        assert!(second.reload_configuration(&cfg).is_ok());
        assert!(store.trust(second.project_root().unwrap()).is_err());
    }

    #[test]
    fn search_scoring_is_deterministic_and_prefers_name_matches() {
        let click = McpToolDef {
            server: "fake".to_string(),
            raw_name: "browser_click".to_string(),
            model_name: "mcp__fake__browser_click".to_string(),
            description: "Interact with page elements.".to_string(),
            input_schema: serde_json::json!({}),
        };
        let narrative = McpToolDef {
            server: "fake".to_string(),
            raw_name: "inspect".to_string(),
            model_name: "mcp__fake__inspect".to_string(),
            description: "Return a narrative that says click many times.".to_string(),
            input_schema: serde_json::json!({}),
        };
        let query = tokenize("click");
        assert!(
            tool_search_score(&click, "click", &query)
                > tool_search_score(&narrative, "click", &query),
            "canonical name matches rank ahead of description-only matches"
        );
        assert_eq!(
            tokenize("BrowserClick browser_click"),
            BTreeSet::from(["browser".to_string(), "click".to_string()])
        );
    }

    #[test]
    fn tool_name_summary_is_stable_and_bounded() {
        let tool = |raw_name: &str| McpToolDef {
            server: "fake".to_string(),
            raw_name: raw_name.to_string(),
            model_name: format!("mcp__fake__{raw_name}"),
            description: String::new(),
            input_schema: serde_json::json!({}),
        };
        let tools = BTreeMap::from([
            ("mcp__fake__alpha".to_string(), tool("alpha")),
            ("mcp__fake__beta".to_string(), tool("beta")),
            ("mcp__fake__gamma".to_string(), tool("gamma")),
            ("mcp__fake__omega".to_string(), tool("omega")),
        ]);

        assert_eq!(
            tool_name_summary(&tools).as_deref(),
            Some("Provides tools: alpha, beta, gamma, and 1 more.")
        );
        assert_eq!(tool_name_summary(&BTreeMap::new()), None);
    }

    #[tokio::test]
    async fn call_tool_timeout_preserves_shared_connection() {
        let (_temp, manager) =
            manager_with_fake_server(fake_server_args(&["--hang".to_string()]), 25);
        manager
            .connect_server("fake")
            .await
            .expect("connect fake server");
        let tool = manager.tool_defs("fake").pop().expect("discovered tool");

        let error = manager
            .call_tool("fake", &tool.model_name, serde_json::json!({}))
            .await
            .expect_err("hanging MCP tool should time out");

        assert!(error.to_string().contains("timed out"));
        assert!(matches!(
            manager.statuses().pop().expect("server status").state,
            ServerState::Ready
        ));
        assert!(!manager.tool_defs("fake").is_empty());
    }

    #[tokio::test]
    async fn idle_stdio_service_exit_recovers_without_a_call_and_stops_at_three() {
        let temp = tempfile::tempdir().unwrap();
        let log = temp.path().join("methods");
        let (_manager_temp, manager) = manager_with_fake_server(
            fake_server_args(&[
                "--record".into(),
                log.to_string_lossy().into_owned(),
                "--exit-after-catalog-ms".into(),
                "1000".into(),
            ]),
            1000,
        );
        manager.connect_server("fake").await.unwrap();
        assert_eq!(manager.statuses()[0].state, ServerState::Ready);
        tokio::time::timeout(Duration::from_secs(8), async {
            while !matches!(manager.statuses()[0].state, ServerState::Failed(_)) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("idle service exit is observable without another tools/call");
        for _ in 0..3 {
            assert!(manager.connect_server("fake").await.is_err());
        }
        let log = std::fs::read_to_string(log).unwrap();
        assert_eq!(log.lines().filter(|line| *line == "initialize").count(), 3);
        assert_eq!(log.lines().filter(|line| *line == "tools/call").count(), 0);
    }

    #[tokio::test]
    async fn transport_drop_recovers_without_replaying_call() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let call_log = temp.path().join("calls.log");
        let (_manager_temp, manager) = manager_with_fake_server(
            fake_server_args(&[
                "--die-midcall".to_string(),
                "--record".to_string(),
                call_log.to_string_lossy().into_owned(),
            ]),
            1_000,
        );
        manager
            .connect_server("fake")
            .await
            .expect("connect fake server");
        let tool = manager.tool_defs("fake").pop().expect("discovered tool");

        manager
            .call_tool("fake", &tool.model_name, serde_json::json!({}))
            .await
            .expect_err("connection drop should fail the in-flight call");

        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let status = manager.statuses().pop().expect("status");
                if status.state == ServerState::Ready && status.attempt == 2 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("concrete stdio service exit must recover");
        let methods = std::fs::read_to_string(&call_log).expect("read call log");
        assert_eq!(
            methods
                .lines()
                .filter(|method| *method == "tools/call")
                .count(),
            1,
            "the in-flight call must not be replayed"
        );
        assert_eq!(
            methods
                .lines()
                .filter(|method| *method == "initialize")
                .count(),
            2
        );
        assert_eq!(
            manager.statuses().pop().expect("server status").state,
            ServerState::Ready
        );
    }

    #[tokio::test]
    async fn stale_failure_cannot_mutate_a_reconnected_entry() {
        let (_temp, manager) = manager_with_fake_server(fake_server_args(&[]), 1_000);
        manager
            .connect_server("fake")
            .await
            .expect("initial connection");
        let key = manager
            .configured_server("fake")
            .expect("configured fake server")
            .config_key;
        let stale_generation = manager.current_generation(&key);

        manager.reconnect_server(&key).await.expect("reconnection");
        manager.update_state_if_generation(
            &key,
            stale_generation,
            ServerState::Failed("late failure".to_string()),
            0,
        );
        manager.mark_disconnected(&key, stale_generation);

        assert!(matches!(
            manager.statuses().pop().expect("server status").state,
            ServerState::Ready
        ));
        assert!(!manager.tool_defs(&key).is_empty());
    }

    #[tokio::test]
    async fn reconnect_refetches_tools_without_a_persistent_cache() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let request_log = temp.path().join("requests.log");
        let (_manager_temp, manager) = manager_with_fake_server(
            fake_server_args(&[
                "--record".to_string(),
                request_log.to_string_lossy().into_owned(),
            ]),
            1_000,
        );
        manager
            .connect_server("fake")
            .await
            .expect("initial connection");
        manager
            .reconnect_server("fake")
            .await
            .expect("reconnection");

        let methods = std::fs::read_to_string(&request_log).expect("read request log");
        assert_eq!(
            methods
                .lines()
                .filter(|method| *method == "tools/list")
                .count(),
            2,
            "each connection must refresh its tool catalog"
        );
    }
}
