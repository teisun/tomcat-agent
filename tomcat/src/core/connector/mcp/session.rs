//! Session-private MCP configuration and a read-only composition of shared and
//! private tool catalogs. This is a manager submodule; it owns no global cache.
use super::*;
use crate::core::connector::mcp::config::McpServerConfig;
use crate::infra::platform::EnvironmentSnapshot;
use std::path::Path;

#[derive(Clone, Default)]
pub(crate) struct SessionMcpConfig(pub BTreeMap<String, McpServerConfig>);

impl std::fmt::Debug for SessionMcpConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Configs may contain credentials in headers, arguments, or environment.
        f.debug_struct("SessionMcpConfig")
            .field("servers", &self.0.len())
            .finish_non_exhaustive()
    }
}

impl SessionMcpConfig {
    pub(crate) fn prepare(
        &self,
        cfg: &AppConfig,
        cwd: &Path,
        session_id: &str,
        shared: Option<&Arc<McpManager>>,
        environment: Option<Arc<EnvironmentSnapshot>>,
    ) -> Result<Option<Arc<McpManager>>, AppError> {
        if self.0.is_empty() {
            return Ok(None);
        }
        if !cfg.connector.enabled {
            return Err(AppError::Config("connector support is disabled".into()));
        }
        let environment = environment.ok_or_else(|| {
            AppError::Config("session MCP requires a controlling connection environment".into())
        })?;
        let shared = shared.ok_or_else(|| {
            AppError::Config("session MCP requires an available connector runtime".into())
        })?;
        let shared_names = shared
            .statuses()
            .into_iter()
            .map(|s| s.name)
            .collect::<BTreeSet<_>>();
        let mut servers = Vec::new();
        for (name, config) in &self.0 {
            config.validate(name)?;
            if shared_names.contains(name) {
                return Err(AppError::Config(format!(
                    "session MCP source conflicts with a configured source: {name}"
                )));
            }
            let mut config = config.clone();
            config.cwd = Some(cwd.to_path_buf());
            servers.push(ConfiguredMcpServer {
                config_key: format!("session:{session_id}:{name}"),
                name: name.clone(),
                config,
                source: McpConfigSource::Session,
            });
        }
        McpManager::from_servers(cfg, Some(cwd), Some(environment), servers).map(Some)
    }
}

impl McpManager {
    /// Begin existing recovery workers without blocking the service's admission
    /// lock. A prompt joins these workers before asking the model to use tools.
    pub(crate) fn start_session_servers(self: &Arc<Self>) {
        for status in self.statuses() {
            let manager = self.clone();
            tokio::spawn(async move {
                if let Err(error) = manager.connect_server(&status.config_key).await {
                    warn!(server = %status.name, error = %error, "session MCP startup failed");
                }
            });
        }
    }

    pub(crate) async fn ready_session_servers(&self) -> Result<(), AppError> {
        let results =
            futures_util::future::join_all(self.statuses().into_iter().map(|status| async move {
                // Cancellation closes private transports. A subsequent prompt may
                // start them again using this same controller's configuration.
                if status.state == ServerState::Disconnected {
                    self.reconnect_server(&status.config_key).await.map(|_| ())
                } else {
                    self.connect_server(&status.config_key).await
                }
            }))
            .await;
        let errors = results
            .into_iter()
            .filter_map(Result::err)
            .map(|e| e.to_string())
            .collect::<Vec<_>>();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(AppError::Tool(errors.join("; ")))
        }
    }

    pub(crate) fn suspend_session_servers(&self) {
        for entry in self.entries.write().values_mut() {
            entry.suspended = true;
            entry.retire_connection();
            entry.status.state = ServerState::Disconnected;
        }
    }

    pub(crate) async fn stop_session_servers(&self) {
        self.suspend_session_servers();
        let mut retiring = self
            .entries
            .read()
            .values()
            .filter_map(|e| e.retiring.clone())
            .collect::<Vec<_>>();
        retiring.extend(self.retirements.read().values().cloned());
        futures_util::future::join_all(retiring.iter().map(TaskTracker::wait)).await;
    }
}

/// Only these two managers are visible to a turn. Management operations still
/// use ConnectorRegistry and cannot save, reload, or reveal private configuration.
pub(crate) struct McpToolView {
    shared: Arc<McpManager>,
    private: Option<Arc<McpManager>>,
}

impl McpToolView {
    pub(crate) fn new(shared: Arc<McpManager>, private: Option<Arc<McpManager>>) -> Arc<Self> {
        Arc::new(Self { shared, private })
    }

    fn managers(&self) -> impl Iterator<Item = &Arc<McpManager>> {
        std::iter::once(&self.shared).chain(self.private.as_ref())
    }

    pub(crate) fn has_configured_servers(&self) -> bool {
        self.managers().any(|m| m.has_configured_servers())
    }

    fn collision(&self, name: &str) -> bool {
        self.managers()
            .filter(|m| m.entries.read().values().any(|e| e.server.name == name))
            .count()
            > 1
    }

    fn source_manager(&self, source: &str) -> Result<&Arc<McpManager>, AppError> {
        let mut managers = self.managers().filter(|m| {
            m.entries
                .read()
                .iter()
                .any(|(key, e)| key == source || e.server.name == source)
        });
        let Some(manager) = managers.next() else {
            return Err(AppError::Tool(tr("mcp.unknownSource", &[("id", source)])));
        };
        if managers.next().is_some()
            || manager
                .configured_server(source)
                .is_some_and(|s| self.collision(&s.name))
        {
            return Err(AppError::Tool(tr("mcp.ambiguous", &[("id", source)])));
        }
        Ok(manager)
    }

    fn lookup(&self, name: &str) -> Option<(&Arc<McpManager>, McpToolDef)> {
        let mut found = self
            .managers()
            .filter_map(|m| m.lookup_tool(name).map(|tool| (m, tool)));
        let (manager, tool) = found.next()?;
        if found.next().is_some() {
            return None;
        }
        let source = manager.configured_server(&tool.server)?;
        // A later shared-config reload must not silently redirect a private name,
        // including while the private server is disconnected or has failed.
        (!self.collision(&source.name)).then_some((manager, tool))
    }

    pub(crate) fn list_servers(&self) -> Vec<McpToolSource> {
        let mut sources = self
            .managers()
            .flat_map(|m| m.list_servers())
            .filter(|s| !self.collision(&s.name))
            .collect::<Vec<_>>();
        if self.private.is_some() {
            sources.sort_by(|a, b| a.name.cmp(&b.name));
        }
        sources
    }

    pub(crate) fn list_tools(&self, source: &str) -> Result<Vec<McpToolSummary>, AppError> {
        self.source_manager(source)?.list_tools(source)
    }

    pub(crate) fn search(
        &self,
        query: &str,
        source: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<McpToolSearchMatch>, AppError> {
        if let Some(source) = source {
            return self
                .source_manager(source)?
                .search(query, Some(source), limit, offset);
        }
        let mut matches = Vec::new();
        for manager in self.managers() {
            matches.extend(manager.search_scored(query, None)?);
        }
        matches.retain(|(_, tool)| !self.collision(&tool.source));
        matches.sort_by(|(a_score, a), (b_score, b)| {
            b_score.cmp(a_score).then_with(|| a.name.cmp(&b.name))
        });
        Ok(matches
            .into_iter()
            .skip(offset)
            .take(limit)
            .map(|(_, item)| item)
            .collect())
    }

    pub(crate) fn describe_many(&self, names: &[String]) -> McpToolDescribeMany {
        let mut tools = Vec::new();
        let mut errors = Vec::new();
        for name in names {
            match self.lookup(name) {
                Some((_, tool)) => tools.push(tool),
                None => errors.push(McpToolLookupError {
                    name: name.clone(),
                    message: tr("mcp.deferredUnknown", &[("name", name)]),
                }),
            }
        }
        McpToolDescribeMany { tools, errors }
    }

    pub(crate) async fn call_model_tool_with_context(
        &self,
        name: &str,
        params: serde_json::Value,
        context: McpCallContext,
    ) -> Result<serde_json::Value, AppError> {
        let (manager, _) = self
            .lookup(name)
            .ok_or_else(|| AppError::Tool(tr("mcp.deferredUnknown", &[("name", name)])))?;
        manager
            .call_model_tool_with_context(name, params, context)
            .await
    }
}
