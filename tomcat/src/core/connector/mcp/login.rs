//! Interactive authorization belongs to one source incarnation and one login.
//! Network work returns a credential; only the current owner may persist it.
use crate::infra::i18n::tr;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use super::{AppError, ConfiguredMcpServer, McpManager, ServerState};
use crate::core::connector::mcp::oauth::{self, CredentialSnapshot, StoredOAuthToken};
use crate::core::connector::mcp::transport::http_client_for;

struct LoginOwner<'a> {
    manager: &'a McpManager,
    key: String,
    cancel: Arc<CancellationToken>,
}
impl Drop for LoginOwner<'_> {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.manager
            .oauth_cancellations
            .remove_if(&self.key, |_, current| Arc::ptr_eq(current, &self.cancel));
    }
}

fn superseded() -> AppError {
    AppError::Tool(tr("mcp.login.superseded", &[]))
}

impl McpManager {
    pub async fn login_server(&self, identifier: &str) -> Result<(), AppError> {
        let key = self.resolve_key(identifier)?;
        let (server, generation, expected, cancel) = self.begin_login(&key)?;
        let owner = LoginOwner {
            manager: self,
            key: key.clone(),
            cancel,
        };
        let url = server
            .config
            .url
            .as_deref()
            .ok_or_else(|| AppError::Tool(tr("mcp.login.notHttp", &[])))?;
        let client = http_client_for(url)?;
        let oauth = server.config.oauth.clone().unwrap_or_default();
        let result = tokio::select! {
            biased;
            _ = owner.cancel.cancelled() => return Err(superseded()),
            result = oauth::authorize_token(&client, &key, url, &oauth, true) => result,
        };
        let token = match result {
            Ok(token) => token,
            Err(_) => {
                let mut entries = self.entries.write();
                if let Some(entry) = entries
                    .get_mut(&key)
                    .filter(|entry| entry.generation == generation && !owner.cancel.is_cancelled())
                {
                    if entry.connection.is_none() {
                        entry.status.state = ServerState::NeedsAuthorization;
                    }
                }
                return Err(AppError::Tool(tr("mcp.login.incomplete", &[])));
            }
        };
        self.commit_login(&key, generation, &owner.cancel, &expected, token)?;
        // The reservation checks this same login nonce under the entries lock;
        // an older continuation must not restart a newer login's connection.
        self.reconnect_after_login(&key, generation, owner.cancel.clone())
            .await
    }

    fn begin_login(
        &self,
        key: &str,
    ) -> Result<
        (
            ConfiguredMcpServer,
            u64,
            CredentialSnapshot,
            Arc<CancellationToken>,
        ),
        AppError,
    > {
        let mut entries = self.entries.write();
        let entry = entries.get_mut(key).ok_or_else(superseded)?;
        if entry.status.overridden {
            return Err(superseded());
        }
        if !self.project_trusted_for(&entry.server) {
            entry.status.state = ServerState::AwaitingProjectTrust;
            return Err(AppError::Tool(tr("mcp.login.trustRequired", &[])));
        }
        let expected = self.oauth_store.snapshot(key)?;
        let cancel = Arc::new(CancellationToken::new());
        if let Some(previous) = self
            .oauth_cancellations
            .insert(key.to_owned(), cancel.clone())
        {
            previous.cancel();
        }
        Ok((entry.server.clone(), entry.generation, expected, cancel))
    }

    pub(super) fn commit_login(
        &self,
        key: &str,
        generation: u64,
        cancel: &Arc<CancellationToken>,
        expected: &CredentialSnapshot,
        token: StoredOAuthToken,
    ) -> Result<(), AppError> {
        let entries = self.entries.read();
        let entry = entries.get(key).ok_or_else(superseded)?;
        if cancel.is_cancelled()
            || entry.generation != generation
            || entry.status.overridden
            || !self
                .oauth_cancellations
                .get(key)
                .is_some_and(|current| Arc::ptr_eq(current.value(), cancel))
        {
            return Err(superseded());
        }
        if !self
            .oauth_store
            .save_if_snapshot_current(key, expected, token)?
        {
            return Err(superseded());
        }
        Ok(())
    }

    pub fn cancel_login(&self, identifier: &str) -> bool {
        let Ok(key) = self.resolve_key(identifier) else {
            return false;
        };
        self.oauth_cancellations
            .remove(&key)
            .map(|(_, cancel)| cancel.cancel())
            .is_some()
    }

    pub fn save_static_bearer(
        &self,
        identifier: &str,
        token: String,
        resource: Option<String>,
    ) -> Result<(), AppError> {
        let key = self.resolve_key(identifier)?;
        self.ensure_runnable(&key)?;
        self.cancel_login(&key);
        let mut entries = self.entries.write();
        let entry = entries.get_mut(&key).ok_or_else(superseded)?;
        self.oauth_store.save_static_bearer(&key, token, resource)?;
        entry.retire_connection();
        entry.generation = self.invalidate_generation(&key);
        entry.suspended = false;
        entry.budget = None;
        entry.status.state = ServerState::Pending;
        Ok(())
    }

    pub fn logout_server(&self, identifier: &str) -> Result<bool, AppError> {
        self.logout_server_with_peers(identifier, Vec::new())
    }

    /// Lock order: registry cache (when present), sorted manager entries, then
    /// the credential store. No network wait or cache lookup occurs inside.
    pub(crate) fn logout_server_with_peers(
        &self,
        identifier: &str,
        mut peers: Vec<Arc<McpManager>>,
    ) -> Result<bool, AppError> {
        let key = self.resolve_key(identifier)?;
        self.ensure_runnable(&key)?;
        peers.push(self.self_weak.upgrade().ok_or_else(superseded)?);
        peers.sort_unstable_by_key(Arc::as_ptr);
        peers.dedup_by(|first, second| Arc::ptr_eq(first, second));
        let owner = peers
            .iter()
            .position(|manager| std::ptr::eq(manager.as_ref(), self))
            .ok_or_else(superseded)?;
        let mut entries: Vec<_> = peers
            .iter()
            .map(|manager| manager.entries.write())
            .collect();
        if entries[owner]
            .get(&key)
            .is_none_or(|entry| entry.status.overridden)
        {
            return Err(superseded());
        }
        for (manager, entries) in peers.iter().zip(&mut entries) {
            let Some(entry) = entries.get_mut(&key) else {
                continue;
            };
            if let Some((_, cancel)) = manager.oauth_cancellations.remove(&key) {
                cancel.cancel();
            }
            entry.retire_connection();
            entry.generation = manager.invalidate_generation(&key);
            entry.suspended = true;
            entry.budget = None;
            entry.status.state = ServerState::Disconnected;
        }
        // Preserve fail-closed local retirement even when persistence fails.
        // Empty-store logout still fences older cross-process login snapshots.
        self.oauth_store.remove(&key)
    }
}

#[cfg(test)]
#[path = "tests/login_test.rs"]
mod tests;
