//! Session-owned MCP bindings. Shared project connectors remain in ScopeContainer.
use super::ChatContext;
use crate::core::connector::mcp::manager::{McpManager, McpToolView, SessionMcpConfig};
use crate::infra::{error::AppError, platform::EnvironmentSnapshot};
use std::sync::Arc;
use std::time::Duration;

impl ChatContext {
    pub(crate) fn connector_tools(&self) -> Option<Arc<McpToolView>> {
        let private = self.session_runtime.session_mcp.read().clone();
        self.global_services
            .connector_registry
            .as_ref()
            .map(|registry| McpToolView::new(registry.mcp_manager(), private))
    }

    pub(crate) fn prepare_session_mcp(
        &self,
        config: &SessionMcpConfig,
        session_id: &str,
        environment: Option<Arc<EnvironmentSnapshot>>,
    ) -> Result<Option<Arc<McpManager>>, AppError> {
        let shared = self
            .global_services
            .connector_registry
            .as_ref()
            .map(|r| r.mcp_manager());
        config.prepare(
            &self.config,
            &self.scope_services.agent_workspace_dir,
            session_id,
            shared.as_ref(),
            environment,
        )
    }

    pub(crate) async fn replace_session_mcp(
        &self,
        prepared: Option<Arc<McpManager>>,
    ) -> Result<(), AppError> {
        // Leave the old manager reachable until cleanup finishes. Disconnect can
        // then join its existing retirement receipts even if this future is dropped.
        self.stop_session_mcp().await?;
        *self.session_runtime.session_mcp.write() = prepared;
        Ok(())
    }

    pub(crate) async fn ready_session_mcp(
        &self,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<(), AppError> {
        let private = self.session_runtime.session_mcp.read().clone();
        let Some(private) = private else {
            return Ok(());
        };
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Ok(()),
            ready = private.ready_session_servers() => ready,
        }
    }

    pub(crate) fn start_session_mcp(&self) {
        let private = self.session_runtime.session_mcp.read().clone();
        if let Some(private) = private {
            private.start_session_servers();
        }
    }

    pub(crate) fn suspend_session_mcp(&self) {
        let private = self.session_runtime.session_mcp.read().clone();
        if let Some(private) = private {
            private.suspend_session_servers();
        }
    }

    pub(crate) async fn stop_session_mcp(&self) -> Result<(), AppError> {
        let private = self.session_runtime.session_mcp.read().clone();
        if let Some(private) = private {
            tokio::time::timeout(Duration::from_secs(5), private.stop_session_servers())
                .await
                .map_err(|_| AppError::Config("session_mcp_stop_timeout".into()))?;
        }
        Ok(())
    }
}
