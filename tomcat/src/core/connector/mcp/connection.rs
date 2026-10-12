//! SDK service ownership and one complete startup attempt (manager submodule).
use std::path::PathBuf;
use std::sync::{atomic::AtomicBool, Arc, Weak};

use rmcp::{Peer, RoleClient};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

use super::lifecycle::ConnectionEvent;
use super::{
    parse_tools, CallRuntime, ConfiguredMcpServer, ConnectedServer, Duration, McpManager,
    OAuthTokenStore,
};
use crate::core::connector::mcp::failure::{FailureKind, McpFailure};
use crate::core::connector::mcp::transport::{
    HttpTransport, McpClient, McpTransport, StdioTransport,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ConnectionId {
    pub generation: u64,
    pub attempt: u8,
}

pub(super) struct ConnectAttempt {
    pub server: ConfiguredMcpServer,
    pub runtime: crate::infra::config::McpRuntimeConfig,
    pub id: ConnectionId,
    pub manager: Weak<McpManager>,
    pub workspace_root: Option<PathBuf>,
    pub oauth_store: OAuthTokenStore,
    pub cancel: CancellationToken,
    pub cleanup: TaskTracker,
    pub refresh: Option<McpFailure>,
    pub refresh_used: Arc<AtomicBool>,
}

impl ConnectAttempt {
    fn token_commit(&self) -> crate::core::connector::mcp::oauth::TokenCommit {
        let manager = self.manager.clone();
        let server = self.server.clone();
        let id = self.id;
        let cancel = self.cancel.clone();
        let store = self.oauth_store.clone();
        Arc::new(move |expected, updated| {
            let Some(manager) = manager.upgrade() else {
                return Ok(false);
            };
            let entries = manager.entries.read();
            let Some(entry) = entries.get(&server.config_key) else {
                return Ok(false);
            };
            if cancel.is_cancelled()
                || entry.generation != id.generation
                || entry.status.overridden
                || entry.suspended
                || entry
                    .budget
                    .as_ref()
                    .is_none_or(|budget| budget.used != id.attempt)
                || entry
                    .run
                    .as_ref()
                    .is_none_or(|run| run.cancel.is_cancelled())
                || !super::same_transport_configuration(&entry.server, &server)
            {
                return Ok(false);
            }
            // No await while holding this boundary: revoke/config replacement
            // and token CAS linearize against the same current source identity.
            store.save_if_snapshot_current(&server.config_key, expected, updated)
        })
    }
}

/// Callers clone only Peer. Exactly one observer owns and joins RunningService.
pub(super) struct ServiceOwner {
    pub peer: Peer<RoleClient>,
    stop: CancellationToken,
    pub tasks: TaskTracker,
}

impl ServiceOwner {
    fn new(client: McpClient, attempt: &ConnectAttempt) -> Self {
        let owner = Self {
            peer: client.peer().clone(),
            // Retain the token supplied to serve_with_ct for this attempt.
            stop: attempt.cancel.clone(),
            tasks: attempt.cleanup.clone(),
        };
        let manager = attempt.manager.clone();
        let key = attempt.server.config_key.clone();
        let id = attempt.id;
        owner.tasks.spawn(async move {
            // A JoinError is also a concrete service-exit event. Do not format
            // arbitrary panic payloads or remote data in a lifecycle diagnostic.
            let _ = client.waiting().await;
            if let Some(manager) = manager.upgrade() {
                manager.connection_event(&key, id, ConnectionEvent::ServiceEnded);
            }
        });
        owner
    }
    pub fn retire(&self) {
        self.stop.cancel();
        self.tasks.close();
    }
}
impl Drop for ServiceOwner {
    fn drop(&mut self) {
        self.retire();
    }
}

impl ConnectedServer {
    pub(super) async fn connect(attempt: &ConnectAttempt) -> Result<Self, McpFailure> {
        let server = &attempt.server;
        let client = if server.config.url.is_some() {
            HttpTransport::new(attempt.oauth_store.clone())
                .with_max_concurrent_requests(attempt.runtime.max_concurrent_calls)
                .with_refresh(attempt.refresh.clone(), attempt.refresh_used.clone())
                .with_token_commit(attempt.token_commit())
                .connect_cancellable(server, attempt.cancel.clone(), attempt.cleanup.clone())
                .await?
        } else {
            StdioTransport::new(attempt.workspace_root.as_deref())
                .connect_cancellable(server, attempt.cancel.clone(), attempt.cleanup.clone())
                .await?
        };
        let owner = ServiceOwner::new(client, attempt);
        let catalog = async {
            let listed = list_all_tools(&owner.peer).await?;
            let mut all_tools = parse_tools(&server.name, &listed)
                .map_err(|_| McpFailure::new(FailureKind::Protocol, "tools/list schema"))?;
            for tool in all_tools.values_mut() {
                tool.server = server.config_key.clone();
            }
            let (title, instructions) = owner
                .peer
                .peer_info()
                .map(|info| {
                    (
                        info.server_info.as_ref().map(|server| server.name.clone()),
                        info.instructions.clone(),
                    )
                })
                .unwrap_or_default();
            let resource_count = list_resource_count(&owner.peer).await;
            if owner.peer.is_transport_closed() {
                return Err(McpFailure::new(
                    FailureKind::Transient,
                    "catalog connection closed",
                ));
            }
            Ok((all_tools, title, instructions, resource_count))
        };
        let (all_tools, title, instructions, resource_count) = tokio::select! {
            biased;
            _ = attempt.cancel.cancelled() => return Err(McpFailure::new(FailureKind::Cancelled, "catalog")),
            result = catalog => result?,
        };
        Ok(Self {
            id: attempt.id,
            owner,
            all_tools,
            title,
            instructions,
            resource_count,
            calls: CallRuntime::new(
                attempt.runtime.max_concurrent_calls,
                Duration::from_millis(attempt.runtime.call_timeout_ms),
            ),
        })
    }
    pub(super) fn retire(&self) {
        self.calls.retire();
        self.owner.retire();
    }
    pub(super) async fn wait_retired(&self) {
        self.retire();
        tokio::join!(self.calls.tasks.wait(), self.owner.tasks.wait());
    }
}

async fn list_all_tools(peer: &Peer<RoleClient>) -> Result<serde_json::Value, McpFailure> {
    let mut cursor = None;
    let mut all_tools = Vec::new();
    for _ in 0..100 {
        let params = cursor
            .take()
            .map(|cursor| rmcp::model::PaginatedRequestParams::default().with_cursor(Some(cursor)));
        let listed = peer
            .list_tools(params)
            .await
            .map_err(|error| McpFailure::from_service(&error, "tools/list"))?;
        let listed = serde_json::to_value(listed)
            .map_err(|_| McpFailure::new(FailureKind::Protocol, "tools/list encoding"))?;
        let tools = listed
            .get("tools")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| McpFailure::new(FailureKind::Protocol, "tools/list"))?;
        all_tools.extend(tools.iter().cloned());
        cursor = listed
            .get("nextCursor")
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned);
        if cursor.is_none() {
            return Ok(serde_json::json!({"tools":all_tools}));
        }
    }
    Err(McpFailure::new(
        FailureKind::Protocol,
        "tools/list exceeded 100 pages",
    ))
}

async fn list_resource_count(peer: &Peer<RoleClient>) -> usize {
    let supported = peer
        .peer_info()
        .and_then(|info| serde_json::to_value(info.capabilities.clone()).ok())
        .and_then(|capabilities| capabilities.get("resources").cloned())
        .is_some();
    if !supported {
        return 0;
    }
    match peer.list_resources(None).await {
        Ok(resources) => serde_json::to_value(resources)
            .ok()
            .and_then(|value| {
                value
                    .get("resources")
                    .and_then(serde_json::Value::as_array)
                    .map(Vec::len)
            })
            .unwrap_or(0),
        Err(error) => {
            tracing::warn!(phase = "resources/list", error = %McpFailure::from_service(&error, "resources/list"), "optional MCP resource discovery failed");
            0
        }
    }
}
