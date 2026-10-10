//! One finite recovery owner per source (manager submodule).
use std::sync::{Arc, Weak};
use std::time::Duration;

use futures_util::FutureExt;
use serde::Serialize;
use tokio::time::Instant;
use tokio_util::task::TaskTracker;

use super::connection::{ConnectAttempt, ConnectionId};
use super::{visible_tools, AppError, ConnectorEntry, McpManager, ServerState};
use crate::core::connector::mcp::call::CLEANUP_GRACE;
use crate::core::connector::mcp::failure::{FailureKind, McpFailure};
use crate::core::connector::mcp::recovery::{stable, RecoveryBudget, RecoveryRun, MAX_ATTEMPTS};

#[cfg(test)]
#[path = "lifecycle_tests.rs"]
mod tests;

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReloadReceipt {
    pub config_key: String,
    pub accepted: bool,
    pub generation: String,
    pub recovery_timeout_ms: u64,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryStatus {
    pub phase: &'static str,
    pub max_attempts: u8,
    pub remaining_ms: u64,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum ConnectionEvent {
    ServiceEnded,
    SubmissionClosed,
    SessionExpired,
}

enum Trigger {
    Ensure,
    Reload,
    Authorized(u64, Arc<tokio_util::sync::CancellationToken>),
    Connection(ConnectionId, ConnectionEvent),
    Authorization(ConnectionId, McpFailure),
}

fn matches_run(entry: &ConnectorEntry, generation: u64, run: &Arc<RecoveryRun>) -> bool {
    entry.generation == generation
        && !entry.suspended
        && !entry.status.overridden
        && !run.cancel.is_cancelled()
        && entry
            .run
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, run))
}

fn cancelled() -> McpFailure {
    McpFailure::new(FailureKind::Cancelled, "recovery superseded")
}

impl McpManager {
    /// Startup/Trust/Add join the manager's existing work, never start another
    /// retry loop. Preserve the existing non-error status for untrusted sources.
    pub async fn connect_server(&self, identifier: &str) -> Result<(), AppError> {
        let key = self.resolve_key(identifier)?;
        self.ensure_runnable(&key)?;
        let server = self
            .configured_server(&key)
            .ok_or_else(|| cancelled().to_app_error(identifier))?;
        if !self.project_trusted_for(&server) {
            self.update_state(&key, ServerState::AwaitingProjectTrust, 0);
            return Ok(());
        }
        let (_, run) = self.reserve_recovery(&key, Trigger::Ensure, MAX_ATTEMPTS)?;
        match run {
            Some(run) => run.wait().await.map_err(|e| e.to_app_error(identifier)),
            None => Ok(()),
        }
    }

    /// Network work starts only after the receipt has been reserved under the
    /// entries lock. Serve can return this directly without waiting for it.
    pub fn request_reconnect(&self, identifier: &str) -> Result<ReloadReceipt, AppError> {
        let key = self.resolve_key(identifier)?;
        self.reserve_recovery(&key, Trigger::Reload, MAX_ATTEMPTS)
            .map(|(receipt, _)| receipt)
    }

    pub async fn reconnect_server(&self, identifier: &str) -> Result<(), AppError> {
        self.reconnect_wait(identifier, MAX_ATTEMPTS).await
    }

    /// TestConnector remains one complete startup attempt rather than silently
    /// inheriting three attempts behind the messenger's ordinary request wait.
    pub async fn test_server(&self, identifier: &str) -> Result<(), AppError> {
        self.reconnect_wait(identifier, 1).await
    }

    pub(super) async fn reconnect_after_login(
        &self,
        key: &str,
        generation: u64,
        cancel: Arc<tokio_util::sync::CancellationToken>,
    ) -> Result<(), AppError> {
        let (_, run) =
            self.reserve_recovery(key, Trigger::Authorized(generation, cancel), MAX_ATTEMPTS)?;
        match run {
            Some(run) => run.wait().await.map_err(|error| error.to_app_error(key)),
            None => Ok(()),
        }
    }

    async fn reconnect_wait(&self, identifier: &str, limit: u8) -> Result<(), AppError> {
        let key = self.resolve_key(identifier)?;
        let (_, run) = self.reserve_recovery(&key, Trigger::Reload, limit)?;
        match run {
            Some(run) => run.wait().await.map_err(|e| e.to_app_error(identifier)),
            None => Ok(()),
        }
    }

    pub(super) fn connection_event(&self, key: &str, id: ConnectionId, event: ConnectionEvent) {
        let _ = self.reserve_recovery(key, Trigger::Connection(id, event), MAX_ATTEMPTS);
    }
    pub(super) fn authorization_failure(&self, key: &str, id: ConnectionId, failure: McpFailure) {
        let _ = self.reserve_recovery(key, Trigger::Authorization(id, failure), MAX_ATTEMPTS);
    }

    fn reserve_recovery(
        &self,
        key: &str,
        trigger: Trigger,
        limit: u8,
    ) -> Result<(ReloadReceipt, Option<Arc<RecoveryRun>>), AppError> {
        let mut entries = self.entries.write();
        let entry = entries
            .get_mut(key)
            .ok_or_else(|| cancelled().to_app_error(key))?;
        let receipt = |entry: &ConnectorEntry| ReloadReceipt {
            config_key: key.to_owned(),
            accepted: true,
            generation: entry.generation.to_string(),
            recovery_timeout_ms: entry
                .budget
                .as_ref()
                .map_or(1, |budget| budget.remaining_ms().max(1)),
        };
        if let Trigger::Connection(id, _) | Trigger::Authorization(id, _) = &trigger {
            if !entry.connection.as_ref().is_some_and(|connection| {
                connection.id == *id && !connection.calls.retired.is_cancelled()
            }) {
                return Err(cancelled().to_app_error(key));
            }
        }
        if let Trigger::Authorized(generation, cancel) = &trigger {
            if entry.generation != *generation
                || cancel.is_cancelled()
                || !self
                    .oauth_cancellations
                    .get(key)
                    .is_some_and(|current| Arc::ptr_eq(current.value(), cancel))
            {
                return Err(cancelled().to_app_error(key));
            }
        }
        if entry.suspended && !matches!(trigger, Trigger::Reload | Trigger::Authorized(..)) {
            return Err(McpFailure::new(
                FailureKind::Cancelled,
                "source explicitly stopped; use Reload",
            )
            .to_app_error(key));
        }
        if entry.status.overridden {
            return Err(
                McpFailure::new(FailureKind::Configuration, "source overridden").to_app_error(key),
            );
        }
        if !self.project_trusted_for(&entry.server) {
            entry.retire_connection();
            entry.status.state = ServerState::AwaitingProjectTrust;
            return Err(AppError::Tool(crate::infra::i18n::tr(
                "mcp.awaitingTrust",
                &[],
            )));
        }
        if !matches!(trigger, Trigger::Authorized(..)) {
            if let Some(run) = entry.run.as_ref().filter(|run| run.is_active()) {
                return Ok((receipt(entry), Some(run.clone())));
            }
        }
        if matches!(trigger, Trigger::Ensure) {
            if entry.connection.is_some() {
                return Ok((receipt(entry), None));
            }
            // Polling and repeated startup callers cannot replenish a stopped round.
            if let Some(run) = &entry.run {
                return Ok((receipt(entry), Some(run.clone())));
            }
        }
        let fresh = matches!(trigger, Trigger::Reload | Trigger::Authorized(..))
            || entry.budget.is_none()
            || stable(entry.ready_since, Instant::now());
        if fresh {
            let budget = RecoveryBudget::new(self.runtime.startup_timeout_ms, limit)
                .map_err(|e| e.to_app_error(key))?;
            entry.generation = self.invalidate_generation(key);
            entry.budget = Some(budget);
            entry.suspended = false;
        }
        let refresh = match trigger {
            Trigger::Authorization(_, failure) => Some(failure),
            Trigger::Connection(_, event) => {
                tracing::info!(config_key = key, generation = entry.generation, event = ?event, "MCP connection event accepted");
                None
            }
            _ => None,
        };
        entry.retire_connection();
        entry.ready_since = None;
        entry.status.state = ServerState::Connecting;
        entry.status.tool_count = 0;
        entry.status.resource_count = 0;
        entry.recovery_phase = "retiring";
        let run = Arc::new(RecoveryRun::new());
        entry.run = Some(run.clone());
        let generation = entry.generation;
        let retiring = entry.retiring.clone();
        let accepted = receipt(entry);
        let manager = self.self_weak.clone();
        let key = key.to_owned();
        let worker_run = run.clone();
        tokio::spawn(async move {
            let _done = worker_run.done.clone().drop_guard();
            let result = std::panic::AssertUnwindSafe(recover(
                manager.clone(),
                key.clone(),
                generation,
                worker_run.clone(),
                retiring,
                refresh,
            ))
            .catch_unwind()
            .await
            .unwrap_or_else(|_| Err(McpFailure::new(FailureKind::Unknown, "recovery task")));
            if let Some(manager) = manager.upgrade() {
                manager.finish_recovery(&key, generation, &worker_run, &result);
            }
            worker_run.finish(result);
        });
        Ok((accepted, Some(run)))
    }

    fn next_attempt(
        &self,
        key: &str,
        generation: u64,
        run: &Arc<RecoveryRun>,
        refresh: Option<McpFailure>,
    ) -> Result<Option<(ConnectAttempt, Duration, Instant)>, McpFailure> {
        let mut entries = self.entries.write();
        let entry = entries
            .get_mut(key)
            .filter(|entry| matches_run(entry, generation, run))
            .ok_or_else(cancelled)?;
        let budget = entry.budget.as_mut().ok_or_else(cancelled)?;
        let Some((attempt, delay)) = budget.next(Instant::now()) else {
            return Ok(None);
        };
        entry.recovery_phase = if delay.is_zero() {
            "starting"
        } else {
            "backoff"
        };
        Ok(Some((
            ConnectAttempt {
                server: entry.server.clone(),
                runtime: self.runtime,
                id: ConnectionId {
                    generation,
                    attempt,
                },
                manager: self.self_weak.clone(),
                workspace_root: self.workspace_root.clone(),
                oauth_store: self.oauth_store.clone(),
                cancel: run.cancel.child_token(),
                cleanup: run.cleanup.clone(),
                refresh,
                refresh_used: budget.refresh_used.clone(),
            },
            delay,
            budget.deadline,
        )))
    }

    fn publish_connection(
        &self,
        attempt: &ConnectAttempt,
        run: &Arc<RecoveryRun>,
        connection: Arc<super::ConnectedServer>,
    ) -> Result<(), McpFailure> {
        let mut entries = self.entries.write();
        let entry = entries
            .get_mut(&attempt.server.config_key)
            .filter(|entry| {
                matches_run(entry, attempt.id.generation, run)
                    && entry
                        .budget
                        .as_ref()
                        .is_some_and(|budget| budget.used == attempt.id.attempt)
            })
            .ok_or_else(cancelled)?;
        if connection.owner.peer.is_transport_closed() {
            return Err(McpFailure::new(
                FailureKind::Transient,
                "service ended before ready",
            ));
        }
        entry.status.tool_count = visible_tools(&connection, &entry.server.config.tool_filter)
            .map_err(|_| McpFailure::new(FailureKind::Configuration, "tool filter"))?
            .len();
        entry.status.resource_count = connection.resource_count;
        entry.connection = Some(connection);
        entry.status.state = ServerState::Ready;
        entry.ready_since = Some(Instant::now());
        entry.recovery_phase = "connected";
        // Atomically release the recovery claim with Ready, so an immediate
        // service exit can reserve the next attempt in this same budget.
        run.finish(Ok(()));
        Ok(())
    }

    fn finish_recovery(
        &self,
        key: &str,
        generation: u64,
        run: &Arc<RecoveryRun>,
        result: &Result<(), McpFailure>,
    ) {
        let Err(error) = result else {
            return;
        };
        let mut entries = self.entries.write();
        let Some(entry) = entries
            .get_mut(key)
            .filter(|entry| matches_run(entry, generation, run))
        else {
            return;
        };
        entry.status.state = if error.kind == FailureKind::Authorization {
            ServerState::NeedsAuthorization
        } else {
            ServerState::Failed(error.to_string())
        };
        entry.status.tool_count = 0;
        entry.status.resource_count = 0;
        entry.recovery_phase = "failed";
        tracing::warn!(config_key = key, generation, attempt = entry.budget.as_ref().map_or(0, |b| b.used), error = %error, "MCP recovery stopped");
    }
}

async fn recover(
    manager: Weak<McpManager>,
    key: String,
    generation: u64,
    run: Arc<RecoveryRun>,
    retiring: Option<TaskTracker>,
    mut refresh: Option<McpFailure>,
) -> Result<(), McpFailure> {
    if let Some(retiring) = retiring {
        if tokio::time::timeout(CLEANUP_GRACE, retiring.wait())
            .await
            .is_err()
        {
            let error = McpFailure::new(FailureKind::Unknown, "retirement cleanup not confirmed");
            if let Some(manager) = manager.upgrade() {
                manager.finish_recovery(&key, generation, &run, &Err(error.clone()));
            }
            run.finish(Err(error.clone()));
            retiring.wait().await; // retain ownership; never fake successful cleanup
            return Err(error);
        }
    }
    let mut refreshed = refresh.is_some();
    let mut last_failure = None;
    loop {
        let Some((attempt, delay, total_deadline)) = manager
            .upgrade()
            .ok_or_else(cancelled)?
            .next_attempt(&key, generation, &run, refresh.take())?
        else {
            return Err(last_failure.unwrap_or_else(|| {
                McpFailure::new(FailureKind::Unknown, "recovery budget exhausted")
            }));
        };
        tokio::select! {
            biased;
            _ = run.cancel.cancelled() => return Err(cancelled()),
            _ = tokio::time::sleep_until(total_deadline) => return Err(McpFailure::new(FailureKind::Timeout, "recovery deadline")),
            _ = tokio::time::sleep(delay) => {},
        }
        let deadline = Instant::now()
            .checked_add(Duration::from_millis(attempt.runtime.startup_timeout_ms))
            .ok_or_else(|| {
                McpFailure::new(FailureKind::Configuration, "startup deadline overflow")
            })?
            .min(total_deadline);
        let result = run_attempt(&attempt, deadline, &run).await;
        match result {
            Ok(connection) => {
                let connection = Arc::new(connection);
                let publish = manager.upgrade().ok_or_else(cancelled)?.publish_connection(
                    &attempt,
                    &run,
                    connection.clone(),
                );
                if publish.is_ok() {
                    return Ok(());
                }
                connection.retire();
                drop(connection);
                drain_attempt(&attempt, &run, Instant::now() + CLEANUP_GRACE).await?;
                let error = publish.unwrap_err();
                if !error.retryable_startup() {
                    return Err(error);
                }
                last_failure = Some(error);
            }
            Err(error) => {
                last_failure = Some(error.clone());
                if error.kind == FailureKind::Authorization
                    && !refreshed
                    && !attempt
                        .refresh_used
                        .load(std::sync::atomic::Ordering::SeqCst)
                {
                    // Eligibility and stored credential identity are rechecked
                    // by HTTP preparation. This is not a second tools/call.
                    refreshed = true;
                    refresh = Some(error);
                } else if !error.retryable_startup() {
                    return Err(error);
                }
            }
        }
    }
}

async fn run_attempt(
    attempt: &ConnectAttempt,
    deadline: Instant,
    run: &Arc<RecoveryRun>,
) -> Result<super::ConnectedServer, McpFailure> {
    let (error, cleanup_deadline) = {
        let connect = super::ConnectedServer::connect(attempt);
        tokio::pin!(connect);
        let (error, pending) = tokio::select! {
            biased;
            _ = run.cancel.cancelled() => (cancelled(), true),
            _ = tokio::time::sleep_until(deadline) => (McpFailure::new(FailureKind::Timeout, "startup"), true),
            result = &mut connect => match result {
                Ok(connection) => return Ok(connection),
                Err(error) => (error, false),
            },
        };
        let cleanup_deadline = Instant::now() + CLEANUP_GRACE;
        attempt.cancel.cancel();
        if pending {
            // Do not poll a completed Future again. One K bounds SDK startup
            // cancellation AND its subsequent owned transport-close receipts.
            let _ = tokio::time::timeout_at(cleanup_deadline, &mut connect).await;
        }
        (error, cleanup_deadline)
    };
    drain_attempt(attempt, run, cleanup_deadline).await?;
    Err(error)
}

async fn drain_attempt(
    attempt: &ConnectAttempt,
    run: &Arc<RecoveryRun>,
    deadline: Instant,
) -> Result<(), McpFailure> {
    attempt.cancel.cancel();
    attempt.cleanup.close();
    if tokio::time::timeout_at(deadline, attempt.cleanup.wait())
        .await
        .is_ok()
    {
        return Ok(());
    }
    let error = McpFailure::new(FailureKind::Unknown, "startup cleanup not confirmed");
    if let Some(manager) = attempt.manager.upgrade() {
        manager.finish_recovery(
            &attempt.server.config_key,
            attempt.id.generation,
            run,
            &Err(error.clone()),
        );
    }
    run.finish(Err(error.clone()));
    attempt.cleanup.wait().await;
    Err(error)
}
