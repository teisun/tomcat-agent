use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use parking_lot::RwLock;
use tracing::warn;

use crate::core::connector::mcp::config::{global_mcp_path, project_mcp_path, McpConfigSource};
use crate::core::connector::mcp::executor::McpToolExecutor;
use crate::core::connector::mcp::manager::McpManager;
use crate::core::tools::contract::registry::{Tool, ToolExecutor};
use crate::infra::error::AppError;
use crate::AppConfig;

pub mod mcp;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectorType {
    Mcp,
    Cli,
    A2a,
}

pub trait Connector: Send + Sync {
    fn connector_type(&self) -> ConnectorType;
}

impl Connector for McpManager {
    fn connector_type(&self) -> ConnectorType {
        ConnectorType::Mcp
    }
}

/// Routes registry calls by their owner namespace without changing the existing
/// JSON-only ToolExecutor contract.
pub struct CompositeToolExecutor {
    plugin_executor: Arc<dyn ToolExecutor>,
    mcp_executor: Arc<dyn ToolExecutor>,
}

impl CompositeToolExecutor {
    pub fn new(
        plugin_executor: Arc<dyn ToolExecutor>,
        mcp_executor: Arc<dyn ToolExecutor>,
    ) -> Arc<Self> {
        Arc::new(Self {
            plugin_executor,
            mcp_executor,
        })
    }
}

#[async_trait]
impl ToolExecutor for CompositeToolExecutor {
    async fn execute(
        &self,
        tool: &Tool,
        params: serde_json::Value,
        caller_plugin_id: &str,
        session_id: Option<&str>,
    ) -> Result<serde_json::Value, AppError> {
        let executor = if tool.plugin_id.starts_with("mcp:") {
            &self.mcp_executor
        } else {
            &self.plugin_executor
        };
        executor
            .execute(tool, params, caller_plugin_id, session_id)
            .await
    }
}

/// Owns connector configuration and lifecycle orchestration. MCP catalogs live
/// exclusively in `McpManager`; they are queried through stable `tool_*`
/// builtins and never registered into the prompt-facing ToolRegistry.
pub struct ConnectorRegistry {
    enabled: bool,
    config: AppConfig,
    mcp: Arc<McpManager>,
    started: AtomicBool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ConnectorRegistryCacheKey {
    global_config_path: PathBuf,
    workspace_config_path: Option<PathBuf>,
    enabled: bool,
    disabled: Vec<String>,
    mcp: crate::infra::config::McpRuntimeConfig,
}

fn connector_registry_cache(
) -> &'static RwLock<HashMap<ConnectorRegistryCacheKey, Arc<ConnectorRegistry>>> {
    static CACHE: OnceLock<RwLock<HashMap<ConnectorRegistryCacheKey, Arc<ConnectorRegistry>>>> =
        OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

fn normalized_config_path(path: &Path) -> PathBuf {
    let Some(parent) = path.parent() else {
        return path.to_path_buf();
    };
    let Some(file_name) = path.file_name() else {
        return path.to_path_buf();
    };
    std::fs::canonicalize(parent)
        .unwrap_or_else(|_| parent.to_path_buf())
        .join(file_name)
}

fn connector_registry_cache_key(
    cfg: &AppConfig,
    workspace_root: Option<&Path>,
) -> Result<ConnectorRegistryCacheKey, AppError> {
    let mut disabled = cfg.connector.disabled.clone();
    disabled.sort();
    Ok(ConnectorRegistryCacheKey {
        global_config_path: normalized_config_path(&global_mcp_path(cfg)?),
        workspace_config_path: workspace_root
            .map(|root| project_mcp_path(cfg, root).map(|path| normalized_config_path(&path)))
            .transpose()?,
        enabled: cfg.connector.enabled,
        disabled,
        mcp: cfg.connector.mcp,
    })
}

impl ConnectorRegistry {
    /// Creates a connector registry and schedules startup for all configured MCP servers.
    /// Startup is fire-and-forget: connection attempts run in background tasks.
    pub fn create_and_start(
        cfg: &AppConfig,
        workspace_root: Option<&std::path::Path>,
    ) -> Result<Arc<Self>, AppError> {
        let registry = Self::new(cfg, workspace_root)?;
        registry.spawn_connect_all_now();
        Ok(registry)
    }

    /// Synchronously creates a connector registry without starting connections.
    /// Intended for tests that need precise control over connection timing.
    /// Most callers should use `create_and_start` instead.
    #[doc(hidden)]
    pub fn new(
        cfg: &AppConfig,
        workspace_root: Option<&std::path::Path>,
    ) -> Result<Arc<Self>, AppError> {
        let key = connector_registry_cache_key(cfg, workspace_root)?;
        if let Some(existing) = connector_registry_cache().read().get(&key).cloned() {
            return Ok(existing);
        }

        tracing::debug!(global_config_path = ?key.global_config_path, workspace_config_path = ?key.workspace_config_path, enabled = key.enabled, disabled = ?key.disabled, "creating MCP connector registry");
        let created = Arc::new(Self {
            enabled: cfg.connector.enabled,
            config: cfg.clone(),
            mcp: McpManager::new(cfg, workspace_root)?,
            started: AtomicBool::new(false),
        });
        let mut cache = connector_registry_cache().write();
        Ok(cache
            .entry(key)
            .or_insert_with(|| Arc::clone(&created))
            .clone())
    }

    /// Revoke this source in every cached workspace sharing its credential store.
    /// This is local retirement, not remote token revocation or rollback.
    pub fn logout_server(&self, identifier: &str) -> Result<bool, AppError> {
        let scope = normalized_config_path(&global_mcp_path(&self.config)?);
        // Keep cache membership stable until retirement finishes: a newly
        // published registry must not start with the credential we are removing.
        let cache = connector_registry_cache().read();
        let peers = cache
            .iter()
            .filter(|(key, _)| key.global_config_path == scope)
            .map(|(_, registry)| registry.mcp.clone())
            .collect();
        let result = self.mcp.logout_server_with_peers(identifier, peers);
        drop(cache);
        result
    }

    pub fn mcp_manager(&self) -> Arc<McpManager> {
        self.mcp.clone()
    }

    pub fn mcp_executor(&self) -> Arc<dyn ToolExecutor> {
        McpToolExecutor::new(self.mcp.clone())
    }

    /// Tool-surface gating must derive from config rather than asynchronous Ready state.
    pub fn has_configured_mcp_servers(&self) -> bool {
        self.enabled && self.mcp.has_configured_servers()
    }

    /// Starts trusted configured MCP servers concurrently. This method only
    /// spawns tasks; it never waits for a server and therefore cannot delay a
    /// chat's first request or a serve handshake.
    pub async fn spawn_connect_all(self: &Arc<Self>) {
        self.spawn_connect_all_now();
    }

    fn spawn_connect_all_now(self: &Arc<Self>) {
        if !self.enabled || self.started.swap(true, Ordering::AcqRel) {
            return;
        }
        for status in self.mcp.statuses() {
            if status.overridden {
                continue;
            }
            let manager = self.mcp.clone();
            tokio::spawn(async move {
                connect_in_background(manager, status.config_key).await;
            });
        }
    }

    /// Notify every cached manager for this project after the durable decision.
    pub fn project_trusted(cfg: &AppConfig, root: &Path) -> Result<(), AppError> {
        let config_root = normalized_config_path(&global_mcp_path(cfg)?);
        let affected: Vec<_> = connector_registry_cache()
            .read()
            .values()
            .filter(|registry| {
                registry.mcp.project_root() == Some(root)
                    && global_mcp_path(&registry.config)
                        .is_ok_and(|path| normalized_config_path(&path) == config_root)
            })
            .cloned()
            .collect();
        for registry in affected {
            let waiting = registry.mcp.wake_project();
            if registry.started.load(Ordering::Acquire) {
                for key in waiting {
                    let manager = registry.mcp.clone();
                    tokio::spawn(async move { connect_in_background(manager, key).await });
                }
            }
        }
        Ok(())
    }

    /// Explicit user reload also retries stopped/failed sources with a fresh
    /// recovery budget. Keep healthy connections, unlike a forced restart.
    pub async fn reload_and_reconnect(&self) -> Result<(), AppError> {
        self.mcp.reload_configuration(&self.config)?;
        let mut errors = Vec::new();
        for status in self.mcp.statuses() {
            if status.overridden || status.state == mcp::manager::ServerState::Ready {
                continue;
            }
            if let Err(error) = self.mcp.reconnect_server(&status.config_key).await {
                warn!(server = %status.config_key, error = %error, "MCP server did not become ready after explicit reload");
                errors.push(error.to_string());
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(AppError::Tool(errors.join("; ")))
        }
    }

    /// Reconcile a saved configuration without resuming intentionally stopped
    /// sources. User-triggered reload uses `reload_and_reconnect` instead.
    pub async fn reload(&self) -> Result<(), AppError> {
        let server_names = self.mcp.reload_configuration(&self.config)?;
        let mut errors = Vec::new();
        for server_name in server_names {
            if let Err(error) = self.mcp.connect_server(&server_name).await {
                warn!(server = %server_name, error = %error, "MCP server did not become ready after reload");
                errors.push(error.to_string());
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(AppError::Tool(errors.join("; ")))
        }
    }

    /// Reconcile every cached registry touched by a config-file mutation without
    /// starting network work. A Global file is shared by all workspaces; a
    /// Project file belongs only to its explicitly selected workspace.
    pub fn synchronize_cached_config_change(
        cfg: &AppConfig,
        source: McpConfigSource,
        workspace_root: Option<&Path>,
    ) -> Result<(), AppError> {
        let changed_global_path = normalized_config_path(&global_mcp_path(cfg)?);
        let changed_project_path = match source {
            McpConfigSource::Global => None,
            McpConfigSource::Project => {
                let workspace_root = workspace_root.ok_or_else(|| {
                    AppError::Config(
                        "project MCP configuration requires an explicit session project root"
                            .into(),
                    )
                })?;
                Some(normalized_config_path(&project_mcp_path(
                    cfg,
                    workspace_root,
                )?))
            }
        };
        let cache = connector_registry_cache().read();
        let mut registries = Vec::new();
        for registry in cache.values() {
            // A Tomcat process can host independent AppConfig roots in tests and
            // embedded callers. A Global update crosses workspaces, not roots.
            if normalized_config_path(&global_mcp_path(&registry.config)?) != changed_global_path {
                continue;
            }
            if let Some(changed_project_path) = &changed_project_path {
                let Some(registry_workspace_root) = registry.mcp.workspace_root() else {
                    continue;
                };
                if normalized_config_path(&project_mcp_path(
                    &registry.config,
                    registry_workspace_root,
                )?) != *changed_project_path
                {
                    continue;
                }
            }
            registries.push(Arc::clone(registry));
        }
        drop(cache);
        for registry in registries {
            // The receiver's configuration chooses its resource directory and
            // disabled servers; never reload it with the mutating session's cfg.
            registry.mcp.reload_configuration(&registry.config)?;
        }
        Ok(())
    }
}

async fn connect_in_background(manager: Arc<McpManager>, server_name: String) {
    // The manager owns the only attempt counter/backoff/deadline. This caller
    // only waits for its terminal result and never replenishes that budget.
    if let Err(error) = manager.connect_server(&server_name).await {
        warn!(server = %server_name, error = %error, "MCP server did not become ready during background startup");
    }
}

#[cfg(test)]
#[path = "tests/connector_test.rs"]
mod tests;
