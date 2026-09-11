use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use parking_lot::RwLock;
use serde::Serialize;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::core::connector::mcp::config::{
    load_servers, remove_global_server, remove_project_server, set_global_tool_filter,
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
    pub name: String,
    pub source: McpConfigSource,
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
    servers: RwLock<BTreeMap<String, ConfiguredMcpServer>>,
    states: RwLock<BTreeMap<String, ServerStatus>>,
    connections: RwLock<BTreeMap<String, Arc<ConnectedServer>>>,
    trust: TrustStore,
    oauth_store: OAuthTokenStore,
    oauth_cancellations: DashMap<String, CancellationToken>,
    workspace_root: std::path::PathBuf,
}

struct ConnectedServer {
    client: McpClient,
    tools: BTreeMap<String, McpToolDef>,
    all_tools: BTreeMap<String, McpToolDef>,
    title: Option<String>,
    instructions: Option<String>,
    call_timeout: Duration,
    call_lock: tokio::sync::Mutex<()>,
    resource_count: usize,
}

impl McpManager {
    pub fn new(cfg: &AppConfig, workspace_root: &std::path::Path) -> Result<Arc<Self>, AppError> {
        let servers = match load_servers(cfg, workspace_root) {
            Ok(servers) => servers,
            Err(error) => {
                warn!(
                    error = %error,
                    "MCP configuration is invalid; continuing without configured MCP servers"
                );
                Vec::new()
            }
        };
        let trust = TrustStore::open(cfg)?;
        let oauth_store = OAuthTokenStore::open(cfg)?;
        let states = servers
            .iter()
            .map(|server| (server.name.clone(), initial_server_status(server, &trust)))
            .collect();
        Ok(Arc::new(Self {
            servers: RwLock::new(
                servers
                    .into_iter()
                    .map(|server| (server.name.clone(), server))
                    .collect(),
            ),
            states: RwLock::new(states),
            connections: RwLock::new(BTreeMap::new()),
            trust,
            oauth_store,
            workspace_root: workspace_root.to_path_buf(),
            oauth_cancellations: DashMap::new(),
        }))
    }

    pub fn statuses(&self) -> Vec<ServerStatus> {
        self.states.read().values().cloned().collect()
    }

    pub fn workspace_root(&self) -> &std::path::Path {
        &self.workspace_root
    }

    pub fn remove_configured_server(
        &self,
        server_name: &str,
        cfg: &AppConfig,
    ) -> Result<bool, AppError> {
        let server = self
            .configured_server(server_name)
            .ok_or_else(|| AppError::Tool(format!("unknown MCP server: {server_name}")))?;
        let removed = match server.source {
            McpConfigSource::Global => remove_global_server(cfg, server_name)?,
            McpConfigSource::Project => {
                remove_project_server(cfg, &self.workspace_root, server_name)?
            }
        };
        if removed {
            self.connections.write().remove(server_name);
            self.servers.write().remove(server_name);
            self.states.write().remove(server_name);
        }
        Ok(removed)
    }

    pub fn set_configured_tool_filter(
        &self,
        server_name: &str,
        filter: ToolFilter,
        cfg: &AppConfig,
    ) -> Result<(), AppError> {
        let server = self
            .configured_server(server_name)
            .ok_or_else(|| AppError::Tool(format!("unknown MCP server: {server_name}")))?;
        match server.source {
            McpConfigSource::Global => set_global_tool_filter(cfg, server_name, filter)?,
            McpConfigSource::Project => {
                set_project_tool_filter(cfg, &self.workspace_root, server_name, filter)?
            }
        }
        Ok(())
    }

    pub fn configured_server(&self, server_name: &str) -> Option<ConfiguredMcpServer> {
        self.servers.read().get(server_name).cloned()
    }

    /// `tool_*` 元工具的 surface gating 只依赖配置、绝不能依赖异步连接状态。
    pub fn has_configured_servers(&self) -> bool {
        !self.servers.read().is_empty()
    }

    pub fn tool_defs(&self, server_name: &str) -> Vec<McpToolDef> {
        self.connections
            .read()
            .get(server_name)
            .map(|connection| connection.tools.values().cloned().collect())
            .unwrap_or_default()
    }

    /// L1：列出当前可调用的 MCP 来源。连接目录是唯一的运行时事实源，因此只返回
    /// Ready 的 server；BTreeMap 保证顺序稳定。
    pub fn list_servers(&self) -> Vec<McpToolSource> {
        let servers = self.servers.read();
        self.connections
            .read()
            .iter()
            .map(|(name, connection)| {
                let configured = servers.get(name);
                let config_origin = configured
                    .map(|server| server.source.as_str())
                    .unwrap_or("Unknown");
                McpToolSource {
                    name: name.clone(),
                    source_type: "mcp",
                    title: connection.title.clone().unwrap_or_else(|| name.clone()),
                    description: connection
                        .instructions
                        .clone()
                        .or_else(|| tool_name_summary(&connection.tools))
                        .unwrap_or_else(|| {
                            format!("MCP connector configured from {config_origin} mcp.json.")
                        }),
                    tool_count: connection.tools.len(),
                }
            })
            .collect()
    }

    /// L2：列出单个来源的工具卡片，不泄露 schema。
    pub fn list_tools(&self, source: &str) -> Result<Vec<McpToolSummary>, AppError> {
        let connection = self.connected_source(source)?;
        let filter = self
            .configured_server(source)
            .map(|server| server.config.tool_filter)
            .unwrap_or_default();
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

    /// 确定性、零依赖的 keyword search。排序规则：名称子串 > 任意字段子串 >
    /// 名称 token 重叠 > 描述 token 重叠 > model_name 字典序。
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
        if let Some(source) = source {
            // 区分拼错 source 与「source 已配置但尚未 Ready」，给模型可行动的错误。
            let _ = self.connected_source(source)?;
        }

        let normalized_query = query.to_lowercase();
        let query_tokens = tokenize(query);
        let connections = self.connections.read();
        let mut matches = connections
            .iter()
            .filter(|(name, _)| source.is_none_or(|requested| requested == name.as_str()))
            .flat_map(|(source_name, connection)| {
                connection.tools.values().filter_map(|tool| {
                    let score = tool_search_score(tool, &normalized_query, &query_tokens);
                    (score > 0).then(|| {
                        (
                            score,
                            McpToolSearchMatch {
                                name: tool.model_name.clone(),
                                source: source_name.clone(),
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

    /// 批量按调用方输入顺序返回 schema；单个未知工具不得使已知工具失效。
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

    /// 由模型可见的 canonical name 反查工具，不依赖也不污染 ToolRegistry。
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

    pub fn approve(&self, server_name: &str) -> Result<(), AppError> {
        let server = self
            .configured_server(server_name)
            .ok_or_else(|| AppError::Tool(format!("unknown MCP server: {server_name}")))?;
        self.trust.approve(&server)?;
        self.refresh_trust_status(server_name, &server);
        Ok(())
    }

    pub fn deny(&self, server_name: &str) -> Result<(), AppError> {
        let server = self
            .configured_server(server_name)
            .ok_or_else(|| AppError::Tool(format!("unknown MCP server: {server_name}")))?;
        self.trust.deny(&server)?;
        self.refresh_trust_status(server_name, &server);
        self.connections.write().remove(server_name);
        self.update_state(server_name, ServerState::Blocked, 0);
        Ok(())
    }

    pub fn reload_configuration(&self, cfg: &AppConfig) -> Result<Vec<String>, AppError> {
        let servers = load_servers(cfg, &self.workspace_root)?;
        self.connections.write().clear();
        *self.servers.write() = servers
            .iter()
            .cloned()
            .map(|server| (server.name.clone(), server))
            .collect();
        *self.states.write() = servers
            .iter()
            .map(|server| {
                (
                    server.name.clone(),
                    initial_server_status(server, &self.trust),
                )
            })
            .collect();
        Ok(servers.into_iter().map(|server| server.name).collect())
    }

    pub async fn connect_server(&self, server_name: &str) -> Result<(), AppError> {
        let server = self
            .configured_server(server_name)
            .ok_or_else(|| AppError::Tool(format!("unknown MCP server: {server_name}")))?;
        if self.connections.read().contains_key(server_name) {
            return Ok(());
        }
        match self.trust.decide(&server)? {
            TrustDecision::Allowed => {}
            TrustDecision::NeedsConfirmation => {
                self.update_state(server_name, ServerState::NeedsConfirmation, 0);
                return Ok(());
            }
            TrustDecision::Blocked => {
                self.update_state(server_name, ServerState::Blocked, 0);
                return Ok(());
            }
        }
        self.refresh_trust_status(server_name, &server);

        self.update_state(server_name, ServerState::Connecting, 0);
        let startup_timeout = Duration::from_millis(server.config.startup_timeout_ms);
        let connected = match tokio::time::timeout(
            startup_timeout,
            ConnectedServer::connect(&server, &self.workspace_root, self.oauth_store.clone()),
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
                self.update_state(server_name, state, 0);
                return Err(error);
            }
            Err(_) => {
                let error = AppError::Tool(format!(
                    "MCP server '{server_name}' startup timed out after {} ms",
                    server.config.startup_timeout_ms
                ));
                self.update_state(server_name, ServerState::Failed(error.to_string()), 0);
                return Err(error);
            }
        };
        let tool_count = connected.tools.len();
        let resource_count = connected.resource_count;
        self.connections
            .write()
            .insert(server_name.to_string(), connected);
        self.update_state(server_name, ServerState::Ready, tool_count);
        if let Some(status) = self.states.write().get_mut(server_name) {
            status.resource_count = resource_count;
        }
        Ok(())
    }

    pub async fn call_tool(
        &self,
        server_name: &str,
        model_tool_name: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        let connection = self
            .connections
            .read()
            .get(server_name)
            .cloned()
            .ok_or_else(|| {
                AppError::Tool(format!(
                    "MCP server '{server_name}' is not ready; use /connector list for status"
                ))
            })?;
        let tool = connection
            .tools
            .get(model_tool_name)
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
                self.mark_disconnected(server_name);
                return Err(error);
            }
        };
        serde_json::to_value(result)
            .map_err(|error| AppError::Tool(format!("serialize MCP tool result: {error}")))
    }

    pub async fn login_server(&self, server_name: &str) -> Result<(), AppError> {
        let server = self
            .configured_server(server_name)
            .ok_or_else(|| AppError::Tool(format!("unknown MCP server: {server_name}")))?;
        match self.trust.decide(&server)? {
            TrustDecision::Allowed => {}
            TrustDecision::NeedsConfirmation => {
                self.update_state(server_name, ServerState::NeedsConfirmation, 0);
                return Err(AppError::Tool(format!(
                    "connector '{server_name}' requires trust confirmation before OAuth login"
                )));
            }
            TrustDecision::Blocked => {
                self.update_state(server_name, ServerState::Blocked, 0);
                return Err(AppError::Tool(format!(
                    "connector '{server_name}' is blocked"
                )));
            }
        }
        let url = server.config.url.clone().ok_or_else(|| {
            AppError::Tool(format!(
                "MCP server '{server_name}' does not use HTTP OAuth"
            ))
        })?;
        let oauth = server.config.oauth.clone().unwrap_or_default();
        let client = http_client_for(&url)?;
        let cancellation = CancellationToken::new();
        self.oauth_cancellations
            .insert(server_name.to_string(), cancellation.clone());
        let result = tokio::select! {
            result = oauth::authorize(&client, &self.oauth_store, server_name, &url, &oauth, true) => result,
            _ = cancellation.cancelled() => Err(AppError::Tool("OAuth login cancelled".to_string())),
        };
        self.oauth_cancellations.remove(server_name);
        if let Err(error) = result {
            if error.to_string().contains("cancelled") {
                self.update_state(server_name, ServerState::Disconnected, 0);
            } else {
                self.update_state(server_name, ServerState::NeedsAuthorization, 0);
            }
            return Err(error);
        }
        self.reconnect_server(server_name).await
    }

    pub fn cancel_login(&self, server_name: &str) -> bool {
        self.oauth_cancellations
            .remove(server_name)
            .map(|(_, cancellation)| {
                cancellation.cancel();
            })
            .is_some()
    }

    pub fn save_static_bearer(
        &self,
        server_name: &str,
        access_token: String,
        resource: Option<String>,
    ) -> Result<(), AppError> {
        self.oauth_store
            .save_static_bearer(server_name, access_token, resource)
    }

    pub fn logout_server(&self, server_name: &str) -> Result<bool, AppError> {
        self.cancel_login(server_name);
        let removed = self.oauth_store.remove(server_name)?;
        if removed {
            self.connections.write().remove(server_name);
            self.update_state(server_name, ServerState::Disconnected, 0);
        }
        Ok(removed)
    }

    pub async fn reconnect_server(&self, server_name: &str) -> Result<(), AppError> {
        self.connections.write().remove(server_name);
        self.update_state(server_name, ServerState::Pending, 0);
        self.connect_server(server_name).await
    }

    pub async fn connect_all(&self) {
        for status in self.statuses() {
            if let Err(error) = self.connect_server(&status.name).await {
                warn!(server = %status.name, error = %error, "MCP server did not become ready");
            }
        }
    }

    fn update_state(&self, server_name: &str, state: ServerState, tool_count: usize) {
        if let Some(status) = self.states.write().get_mut(server_name) {
            if !matches!(&state, ServerState::Ready) {
                status.resource_count = 0;
            }
            status.state = state;
            status.tool_count = tool_count;
        }
    }

    fn refresh_trust_status(&self, server_name: &str, server: &ConfiguredMcpServer) {
        match self.trust.inspect(server) {
            Ok(trust) => {
                if let Some(status) = self.states.write().get_mut(server_name) {
                    status.trust = trust;
                }
            }
            Err(error) => warn!(
                server = %server_name,
                error = %error,
                "failed to inspect MCP server trust status"
            ),
        }
    }

    fn mark_disconnected(&self, server_name: &str) {
        self.connections.write().remove(server_name);
        self.update_state(server_name, ServerState::Disconnected, 0);
    }

    fn connected_source(&self, source: &str) -> Result<Arc<ConnectedServer>, AppError> {
        if let Some(connection) = self.connections.read().get(source).cloned() {
            return Ok(connection);
        }
        if self.servers.read().contains_key(source) {
            return Err(AppError::Tool(format!(
                "MCP source '{source}' is not ready; use /connector list for status"
            )));
        }
        Err(AppError::Tool(format!("unknown MCP source: {source}")))
    }

    fn lookup_tool(&self, model_tool_name: &str) -> Option<McpToolDef> {
        self.connections
            .read()
            .values()
            .find_map(|connection| connection.tools.get(model_tool_name).cloned())
    }
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

fn initial_server_status(server: &ConfiguredMcpServer, trust: &TrustStore) -> ServerStatus {
    let trust = trust.inspect(server).unwrap_or_else(|error| {
        warn!(
            server = %server.name,
            error = %error,
            "failed to inspect MCP server trust status"
        );
        TrustStatus::Blocked
    });
    ServerStatus {
        name: server.name.clone(),
        source: server.source,
        state: ServerState::Pending,
        trust,
        tool_count: 0,
        resource_count: 0,
    }
}

impl ConnectedServer {
    async fn connect(
        server: &ConfiguredMcpServer,
        workspace_root: &std::path::Path,
        oauth_store: OAuthTokenStore,
    ) -> Result<Self, AppError> {
        let client = if server.config.url.is_some() {
            HttpTransport::new(oauth_store).connect(server).await?
        } else {
            StdioTransport::new(workspace_root).connect(server).await?
        };
        let listed_value = list_all_tools(&client, &server.name).await?;
        let all_tools = parse_tools(
            &server.name,
            &crate::core::connector::mcp::config::ToolFilter::default(),
            &listed_value,
        )?;
        let tools = parse_tools(&server.name, &server.config.tool_filter, &listed_value)?;
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
            tools,
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
        let manager = McpManager::new(&cfg, &workspace).expect("construct MCP manager");
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
        let manager = McpManager::new(&cfg, &workspace).expect("construct MCP manager");
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
