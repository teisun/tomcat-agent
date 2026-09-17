use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use parking_lot::RwLock;
use serde::Serialize;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::core::connector::mcp::config::{
    load_server_definitions, remove_global_server, remove_project_server, set_global_tool_filter,
    set_project_tool_filter, ConfiguredMcpServer, McpConfigSource, ToolFilter,
};
use crate::core::connector::mcp::naming::to_model_name;
use crate::core::connector::mcp::oauth;
use crate::core::connector::mcp::oauth::OAuthTokenStore;
use crate::core::connector::mcp::transport::{
    http_client_for, HttpTransport, McpClient, McpTransport, StdioTransport,
};
use crate::core::connector::mcp::trust::{TrustDecision, TrustStatus, TrustStore};
use crate::infra::error::AppError;
use crate::AppConfig;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerState {
    Pending,
    Connecting,
    Ready,
    Disconnected,
    NeedsConfirmation,
    NeedsAuthorization,
    Blocked,
    Failed(String),
}

impl ServerState {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Connecting => "connecting",
            Self::Ready => "connected",
            Self::Disconnected => "disconnected",
            Self::NeedsConfirmation => "needs_confirmation",
            Self::NeedsAuthorization => "needs_authorization",
            Self::Blocked => "blocked",
            Self::Failed(_) => "failed",
        }
    }

    pub fn display_label(&self) -> &'static str {
        match self {
            Self::Pending => "等待连接",
            Self::Connecting => "连接中",
            Self::Ready => "已连接",
            Self::Disconnected => "已断开",
            Self::NeedsConfirmation => "待确认",
            Self::NeedsAuthorization => "需要授权",
            Self::Blocked => "已阻止",
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
    pub trust: TrustStatus,
    pub tool_count: usize,
    pub resource_count: usize,
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

pub struct McpManager {
    entries: RwLock<BTreeMap<String, ConnectorEntry>>,
    /// Retains the newest incarnation after deletion so old async work cannot
    /// publish into a re-added configKey.
    generations: RwLock<BTreeMap<String, u64>>,
    trust: TrustStore,
    oauth_store: OAuthTokenStore,
    oauth_cancellations: DashMap<String, CancellationToken>,
    workspace_root: Option<std::path::PathBuf>,
}

#[derive(Clone)]
struct ConnectorEntry {
    server: ConfiguredMcpServer,
    status: ServerStatus,
    connection: Option<Arc<ConnectedServer>>,
    generation: u64,
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
    client: McpClient,
    all_tools: BTreeMap<String, McpToolDef>,
    title: Option<String>,
    instructions: Option<String>,
    call_timeout: Duration,
    call_lock: tokio::sync::Mutex<()>,
    resource_count: usize,
}

impl McpManager {
    pub fn new(
        cfg: &AppConfig,
        workspace_root: Option<&std::path::Path>,
    ) -> Result<Arc<Self>, AppError> {
        let servers = load_server_definitions(cfg, workspace_root)?;
        let overridden = overridden_config_keys(&servers);
        let trust = TrustStore::open(cfg)?;
        let oauth_store = OAuthTokenStore::open(cfg)?;
        let entries = servers
            .into_iter()
            .map(|server| {
                let status =
                    initial_server_status(&server, &trust, overridden.contains(&server.config_key));
                (
                    server.config_key.clone(),
                    ConnectorEntry {
                        server,
                        status,
                        connection: None,
                        generation: 0,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let generations = entries
            .keys()
            .map(|key| (key.clone(), 0))
            .collect::<BTreeMap<_, _>>();
        Ok(Arc::new(Self {
            entries: RwLock::new(entries),
            generations: RwLock::new(generations),
            trust,
            oauth_store,
            workspace_root: workspace_root.map(std::path::Path::to_path_buf),
            oauth_cancellations: DashMap::new(),
        }))
    }

    pub fn statuses(&self) -> Vec<ServerStatus> {
        self.entries
            .read()
            .values()
            .map(|entry| entry.status.clone())
            .collect()
    }

    pub fn workspace_root(&self) -> Option<&std::path::Path> {
        self.workspace_root.as_deref()
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
            self.entries.write().remove(&key);
        }
        Ok(removed)
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

    fn current_generation(&self, key: &str) -> u64 {
        *self.generations.write().entry(key.to_string()).or_insert(0)
    }

    fn invalidate_generation(&self, key: &str) -> u64 {
        let mut generations = self.generations.write();
        let generation = generations.entry(key.to_string()).or_insert(0);
        *generation = generation.saturating_add(1);
        *generation
    }

    fn is_current_generation(&self, key: &str, generation: u64) -> bool {
        self.generations.read().get(key) == Some(&generation)
            && self
                .entries
                .read()
                .get(key)
                .is_some_and(|entry| entry.generation == generation)
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

    /// L2：列出单个来源的工具卡片，不泄露 schema。
    pub fn list_tools(&self, identifier: &str) -> Result<Vec<McpToolSummary>, AppError> {
        let key = self
            .resolve_key(identifier)
            .map_err(|_| AppError::Tool(format!("unknown MCP source: {identifier}")))?;
        self.ensure_runnable(&key)?;
        let (connection, filter) = {
            let entries = self.entries.read();
            let entry = entries
                .get(&key)
                .ok_or_else(|| AppError::Tool(format!("unknown MCP source: {identifier}")))?;
            (
                entry.connection.clone().ok_or_else(|| {
                    AppError::Tool(format!(
                        "MCP source '{identifier}' is not ready; use /connector list for status"
                    ))
                })?,
                entry.server.config.tool_filter.clone(),
            )
        };
        let include = build_glob_set(&filter.include)?;
        let exclude = build_glob_set(&filter.exclude)?;
        Ok(connection
            .all_tools
            .values()
            .filter(|tool| {
                (filter.include.is_empty() || include.is_match(&tool.raw_name))
                    && !exclude.is_match(&tool.raw_name)
            })
            .map(|tool| McpToolSummary {
                name: tool.model_name.clone(),
                raw_name: tool.raw_name.clone(),
                description: tool.description.clone(),
                enabled: true,
            })
            .collect())
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
        let tool = self.lookup_tool(model_tool_name).ok_or_else(|| {
            AppError::Tool(format!(
                "unknown or not-ready deferred tool: {model_tool_name}"
            ))
        })?;
        self.call_tool(&tool.server, &tool.model_name, params).await
    }

    pub fn approve(&self, identifier: &str) -> Result<(), AppError> {
        let key = self.resolve_key(identifier)?;
        self.ensure_runnable(&key)?;
        let server = self
            .configured_server(&key)
            .ok_or_else(|| AppError::Tool(format!("unknown MCP server: {identifier}")))?;
        self.trust.approve(&server)?;
        self.refresh_trust_status(&key, &server);
        Ok(())
    }

    pub fn deny(&self, identifier: &str) -> Result<(), AppError> {
        let key = self.resolve_key(identifier)?;
        self.ensure_runnable(&key)?;
        let server = self
            .configured_server(&key)
            .ok_or_else(|| AppError::Tool(format!("unknown MCP server: {identifier}")))?;
        self.trust.deny(&server)?;
        let generation = self.invalidate_generation(&key);
        if let Some(entry) = self.entries.write().get_mut(&key) {
            entry.generation = generation;
            entry.connection = None;
            entry.status.resource_count = 0;
            entry.status.state = ServerState::Blocked;
            entry.status.tool_count = 0;
        }
        self.refresh_trust_status(&key, &server);
        Ok(())
    }

    pub fn reload_configuration(&self, cfg: &AppConfig) -> Result<Vec<String>, AppError> {
        let loaded_servers = load_server_definitions(cfg, self.workspace_root.as_deref())?;
        let overridden = overridden_config_keys(&loaded_servers);
        let next_servers = loaded_servers
            .into_iter()
            .map(|server| (server.config_key.clone(), server))
            .collect::<BTreeMap<_, _>>();
        let previous = self.entries.read().clone();
        let mut changed_keys = BTreeSet::new();
        let mut next_entries = BTreeMap::new();
        for (key, server) in next_servers {
            let is_overridden = overridden.contains(&key);
            if let Some(previous_entry) = previous.get(&key) {
                if same_transport_configuration(&previous_entry.server, &server) {
                    let mut retained = previous_entry.clone();
                    retained.server = server;
                    if let Some(connection) = retained.connection.as_ref() {
                        retained.status.tool_count =
                            visible_tools(connection, &retained.server.config.tool_filter)
                                .map_or(0, |tools| tools.len());
                    }
                    if retained.status.overridden != is_overridden {
                        retained.connection = None;
                        retained.generation = self.invalidate_generation(&key);
                        retained.status =
                            initial_server_status(&retained.server, &self.trust, is_overridden);
                        if !is_overridden {
                            changed_keys.insert(key.clone());
                        }
                    }
                    next_entries.insert(key, retained);
                    continue;
                }
            }
            if !is_overridden {
                changed_keys.insert(key.clone());
            }
            let generation = if previous.contains_key(&key) {
                self.invalidate_generation(&key)
            } else {
                self.current_generation(&key)
            };
            next_entries.insert(
                key,
                ConnectorEntry {
                    status: initial_server_status(&server, &self.trust, is_overridden),
                    server,
                    connection: None,
                    generation,
                },
            );
        }
        for key in previous
            .keys()
            .filter(|key| !next_entries.contains_key(*key))
        {
            self.invalidate_generation(key);
        }
        *self.entries.write() = next_entries;
        Ok(changed_keys.into_iter().collect())
    }

    pub async fn connect_server(&self, identifier: &str) -> Result<(), AppError> {
        let key = self.resolve_key(identifier)?;
        self.ensure_runnable(&key)?;
        let (server, generation, already_connected) = {
            let entries = self.entries.read();
            let entry = entries
                .get(&key)
                .ok_or_else(|| AppError::Tool(format!("unknown MCP server: {identifier}")))?;
            (
                entry.server.clone(),
                entry.generation,
                entry.connection.is_some(),
            )
        };
        if already_connected {
            return Ok(());
        }
        match self.trust.decide(&server)? {
            TrustDecision::Allowed => {}
            TrustDecision::NeedsConfirmation => {
                self.update_state(&key, ServerState::NeedsConfirmation, 0);
                return Ok(());
            }
            TrustDecision::Blocked => {
                self.update_state(&key, ServerState::Blocked, 0);
                return Ok(());
            }
        }
        self.refresh_trust_status(&key, &server);
        self.update_state(&key, ServerState::Connecting, 0);
        let startup_timeout = Duration::from_millis(server.config.startup_timeout_ms);
        let connected = match tokio::time::timeout(
            startup_timeout,
            ConnectedServer::connect(
                &server,
                self.workspace_root.as_deref(),
                self.oauth_store.clone(),
            ),
        )
        .await
        {
            Ok(Ok(connection)) => Arc::new(connection),
            Ok(Err(error)) => {
                let state = if error
                    .to_string()
                    .to_ascii_lowercase()
                    .contains("authorization required")
                {
                    ServerState::NeedsAuthorization
                } else {
                    ServerState::Failed(error.to_string())
                };
                self.update_state_if_generation(&key, generation, state, 0);
                return Err(error);
            }
            Err(_) => {
                let error = AppError::Tool(format!(
                    "MCP server '{identifier}' startup timed out after {} ms",
                    server.config.startup_timeout_ms
                ));
                self.update_state_if_generation(
                    &key,
                    generation,
                    ServerState::Failed(error.to_string()),
                    0,
                );
                return Err(error);
            }
        };
        let tool_count = visible_tools(&connected, &server.config.tool_filter)?.len();
        let resource_count = connected.resource_count;
        let mut entries = self.entries.write();
        let Some(entry) = entries.get_mut(&key) else {
            return Err(AppError::Tool(format!(
                "MCP server '{identifier}' was removed while connecting"
            )));
        };
        if entry.generation != generation
            || entry.server.config_key != server.config_key
            || !same_transport_configuration(&entry.server, &server)
        {
            return Err(AppError::Tool(format!(
                "MCP server '{identifier}' changed while connecting"
            )));
        }
        entry.connection = Some(connected);
        entry.status.state = ServerState::Ready;
        entry.status.tool_count = tool_count;
        entry.status.resource_count = resource_count;
        Ok(())
    }

    pub async fn call_tool(
        &self,
        identifier: &str,
        model_tool_name: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
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
        let _call_guard = connection.call_lock.lock().await;
        let result = tokio::time::timeout(
            connection.call_timeout,
            connection.client.peer().call_tool(request),
        )
        .await
        .map_err(|_| AppError::Tool(format!("MCP tool '{model_tool_name}' timed out")))
        .and_then(|result| {
            result.map_err(|error| {
                AppError::Tool(format!("MCP tool '{model_tool_name}' failed: {error}"))
            })
        });
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                self.mark_disconnected(&key, generation);
                return Err(error);
            }
        };
        serde_json::to_value(result)
            .map_err(|error| AppError::Tool(format!("serialize MCP tool result: {error}")))
    }

    pub async fn login_server(&self, identifier: &str) -> Result<(), AppError> {
        let key = self.resolve_key(identifier)?;
        self.ensure_runnable(&key)?;
        let generation = self.current_generation(&key);
        let server = self
            .configured_server(&key)
            .ok_or_else(|| AppError::Tool(format!("unknown MCP server: {identifier}")))?;
        match self.trust.decide(&server)? {
            TrustDecision::Allowed => {}
            TrustDecision::NeedsConfirmation => {
                self.update_state(&key, ServerState::NeedsConfirmation, 0);
                return Err(AppError::Tool(format!(
                    "connector '{identifier}' requires trust confirmation before OAuth login"
                )));
            }
            TrustDecision::Blocked => {
                self.update_state(&key, ServerState::Blocked, 0);
                return Err(AppError::Tool(format!(
                    "connector '{identifier}' is blocked"
                )));
            }
        }
        let url = server.config.url.clone().ok_or_else(|| {
            AppError::Tool(format!("MCP server '{identifier}' does not use HTTP OAuth"))
        })?;
        let oauth = server.config.oauth.clone().unwrap_or_default();
        let client = http_client_for(&url)?;
        let cancellation = CancellationToken::new();
        self.oauth_cancellations
            .insert(key.clone(), cancellation.clone());
        let result = tokio::select! {
            result = oauth::authorize(&client, &self.oauth_store, &key, &url, &oauth, true) => result,
            _ = cancellation.cancelled() => Err(AppError::Tool("OAuth login cancelled".to_string())),
        };
        self.oauth_cancellations.remove(&key);
        if let Err(error) = result {
            if error.to_string().contains("cancelled") {
                self.update_state(&key, ServerState::Disconnected, 0);
            } else {
                self.update_state(&key, ServerState::NeedsAuthorization, 0);
            }
            return Err(error);
        }
        if !self.is_current_generation(&key, generation) {
            // OAuth may have completed concurrently with removal or replacement.
            // The newer entry owns this configKey, so erase the late credential.
            self.oauth_store.remove(&key)?;
            return Err(AppError::Tool(
                "OAuth login cancelled because the connector changed".to_string(),
            ));
        }
        self.reconnect_server(&key).await
    }

    pub fn cancel_login(&self, identifier: &str) -> bool {
        let Ok(key) = self.resolve_key(identifier) else {
            return false;
        };
        self.oauth_cancellations
            .remove(&key)
            .map(|(_, cancellation)| cancellation.cancel())
            .is_some()
    }

    pub fn save_static_bearer(
        &self,
        identifier: &str,
        access_token: String,
        resource: Option<String>,
    ) -> Result<(), AppError> {
        let key = self.resolve_key(identifier)?;
        self.ensure_runnable(&key)?;
        self.oauth_store
            .save_static_bearer(&key, access_token, resource)
    }

    pub fn logout_server(&self, identifier: &str) -> Result<bool, AppError> {
        let key = self.resolve_key(identifier)?;
        self.ensure_runnable(&key)?;
        self.cancel_login(&key);
        let removed = self.oauth_store.remove(&key)?;
        if removed {
            if let Some(entry) = self.entries.write().get_mut(&key) {
                entry.connection = None;
                entry.status.state = ServerState::Disconnected;
                entry.status.tool_count = 0;
                entry.status.resource_count = 0;
            }
        }
        Ok(removed)
    }

    pub async fn reconnect_server(&self, identifier: &str) -> Result<(), AppError> {
        let key = self.resolve_key(identifier)?;
        self.ensure_runnable(&key)?;
        let generation = self.invalidate_generation(&key);
        if let Some(entry) = self.entries.write().get_mut(&key) {
            entry.connection = None;
            entry.status.state = ServerState::Pending;
            entry.status.tool_count = 0;
            entry.status.resource_count = 0;
            entry.generation = generation;
        }
        self.connect_server(&key).await
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

    fn refresh_trust_status(&self, key: &str, server: &ConfiguredMcpServer) {
        match self.trust.inspect(server) {
            Ok(trust) => {
                if let Some(entry) = self.entries.write().get_mut(key) {
                    entry.status.trust = trust;
                }
            }
            Err(error) => warn!(
                server = %server.name,
                error = %error,
                "failed to inspect MCP server trust status"
            ),
        }
    }

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

    fn mark_disconnected(&self, key: &str, generation: u64) {
        let mut entries = self.entries.write();
        let Some(entry) = entries.get_mut(key) else {
            return;
        };
        if entry.generation != generation {
            return;
        }
        entry.connection = None;
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
    let include = build_glob_set(&filter.include)?;
    let exclude = build_glob_set(&filter.exclude)?;
    Ok(connection
        .all_tools
        .iter()
        .filter(|(_, tool)| {
            (filter.include.is_empty() || include.is_match(&tool.raw_name))
                && !exclude.is_match(&tool.raw_name)
        })
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
    trust: &TrustStore,
    overridden: bool,
) -> ServerStatus {
    let trust = trust.inspect(server).unwrap_or_else(|error| {
        warn!(
            server = %server.name,
            error = %error,
            "failed to inspect MCP server trust status"
        );
        TrustStatus::Blocked
    });
    ServerStatus {
        config_key: server.config_key.clone(),
        name: server.name.clone(),
        source: server.source,
        overridden,
        state: ServerState::Pending,
        trust,
        tool_count: 0,
        resource_count: 0,
    }
}

impl ConnectedServer {
    async fn connect(
        server: &ConfiguredMcpServer,
        workspace_root: Option<&std::path::Path>,
        oauth_store: OAuthTokenStore,
    ) -> Result<Self, AppError> {
        let client = if server.config.url.is_some() {
            HttpTransport::new(oauth_store).connect(server).await?
        } else {
            StdioTransport::new(workspace_root).connect(server).await?
        };
        let listed_value = list_all_tools(&client, &server.name).await?;
        let mut all_tools = parse_tools(
            &server.name,
            &crate::core::connector::mcp::config::ToolFilter::default(),
            &listed_value,
        )?;
        for tool in all_tools.values_mut() {
            tool.server = server.config_key.clone();
        }
        let (title, instructions) = client
            .peer()
            .peer_info()
            .map(|info| {
                (
                    info.server_info.as_ref().map(|server| server.name.clone()),
                    info.instructions.clone(),
                )
            })
            .unwrap_or_default();
        let resource_count = list_resource_count(&client).await;
        Ok(Self {
            client,
            all_tools,
            title,
            instructions,
            call_timeout: Duration::from_millis(server.config.call_timeout_ms),
            call_lock: tokio::sync::Mutex::new(()),
            resource_count,
        })
    }
}

async fn list_resource_count(client: &McpClient) -> usize {
    let resources_supported = client
        .peer()
        .peer_info()
        .and_then(|info| serde_json::to_value(info.capabilities.clone()).ok())
        .and_then(|capabilities| capabilities.get("resources").cloned())
        .is_some();
    if !resources_supported {
        return 0;
    }
    match client.peer().list_resources(None).await {
        Ok(resources) => serde_json::to_value(resources)
            .ok()
            .and_then(|value| {
                value
                    .get("resources")
                    .and_then(serde_json::Value::as_array)
                    .cloned()
            })
            .map_or(0, |resources| resources.len()),
        Err(error) => {
            warn!(error = %error, "MCP server advertised resources but resources/list failed");
            0
        }
    }
}

async fn list_all_tools(
    client: &McpClient,
    server_name: &str,
) -> Result<serde_json::Value, AppError> {
    let mut cursor = None;
    let mut all_tools = Vec::new();
    for _ in 0..100 {
        let params = cursor
            .take()
            .map(|cursor| rmcp::model::PaginatedRequestParams::default().with_cursor(Some(cursor)));
        let listed =
            client.peer().list_tools(params).await.map_err(|error| {
                AppError::Tool(format!("list MCP tools '{server_name}': {error}"))
            })?;
        let listed_value = serde_json::to_value(listed)
            .map_err(|error| AppError::Tool(format!("serialize MCP tool list: {error}")))?;
        let tools = listed_value
            .get("tools")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| {
                AppError::Tool(format!(
                    "MCP server '{server_name}' returned invalid tools/list"
                ))
            })?;
        all_tools.extend(tools.iter().cloned());
        cursor = listed_value
            .get("nextCursor")
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned);
        if cursor.is_none() {
            return Ok(serde_json::json!({ "tools": all_tools }));
        }
    }
    Err(AppError::Tool(format!(
        "MCP server '{server_name}' returned more than 100 pages of tools"
    )))
}

fn parse_tools(
    server: &str,
    filter: &crate::core::connector::mcp::config::ToolFilter,
    listed: &serde_json::Value,
) -> Result<BTreeMap<String, McpToolDef>, AppError> {
    let include = build_glob_set(&filter.include)?;
    let exclude = build_glob_set(&filter.exclude)?;
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
        if !filter.include.is_empty() && !include.is_match(raw_name) || exclude.is_match(raw_name) {
            continue;
        }
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

fn build_glob_set(patterns: &[String]) -> Result<globset::GlobSet, AppError> {
    let mut builder = globset::GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(globset::Glob::new(pattern).map_err(|error| {
            AppError::Config(format!("invalid MCP tool filter '{pattern}': {error}"))
        })?);
    }
    builder
        .build()
        .map_err(|error| AppError::Config(format!("build MCP tool filter: {error}")))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use std::collections::{BTreeMap, BTreeSet};

    use super::{
        tokenize, tool_name_summary, tool_search_score, McpManager, McpToolDef, ServerState,
    };
    use crate::core::connector::mcp::config::{add_global_server, McpServerConfig};
    use crate::infra::config::get_work_dir;
    use crate::AppConfig;

    fn manager_with_fake_server(
        args: Vec<String>,
        call_timeout_ms: u64,
    ) -> (tempfile::TempDir, Arc<McpManager>) {
        let temp = tempfile::tempdir().expect("temporary directory");
        let workspace = temp.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
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
                        "callTimeoutMs": call_timeout_ms,
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

    fn fake_server_args(extra: &[String]) -> Vec<String> {
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
    fn deny_retires_the_generation_before_blocking_the_entry() {
        let (_temp, manager) = manager_with_fake_server(fake_server_args(&[]), 120_000);
        let key = manager
            .configured_server("fake")
            .expect("configured fake server")
            .config_key;
        let old_generation = manager.current_generation(&key);

        manager.deny(&key).expect("deny connector");
        manager.update_state_if_generation(&key, old_generation, ServerState::Ready, 2);

        let entry = manager
            .entries
            .read()
            .get(&key)
            .expect("blocked entry")
            .clone();
        assert!(entry.generation > old_generation);
        assert!(entry.connection.is_none());
        assert!(matches!(entry.status.state, ServerState::Blocked));
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
            ServerState::NeedsConfirmation
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
    async fn call_tool_timeout_returns_error_and_marks_server_disconnected() {
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
            ServerState::Disconnected
        ));
    }

    #[tokio::test]
    async fn transport_drop_marks_disconnected_without_replaying_call() {
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

        let methods = std::fs::read_to_string(&call_log).expect("read call log");
        assert_eq!(
            methods
                .lines()
                .filter(|method| *method == "tools/call")
                .count(),
            1,
            "the in-flight call must not be replayed"
        );
        assert!(matches!(
            manager.statuses().pop().expect("server status").state,
            ServerState::Disconnected
        ));
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
