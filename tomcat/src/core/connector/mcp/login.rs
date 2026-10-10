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
        let key = self.resolve_key(identifier)?;
        self.ensure_runnable(&key)?;
        self.cancel_login(&key);
        let mut entries = self.entries.write();
        let entry = entries.get_mut(&key).ok_or_else(superseded)?;
        entry.retire_connection();
        entry.generation = self.invalidate_generation(&key);
        entry.suspended = true;
        entry.budget = None;
        entry.status.state = ServerState::Disconnected;
        // Even logout with no stored token invalidates in-flight authorization
        // snapshots (including other managers sharing this credential store).
        self.oauth_store.remove(&key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, Arc<McpManager>, crate::AppConfig, String) {
        let temp = tempfile::tempdir().unwrap();
        let mut cfg = crate::AppConfig::default();
        cfg.storage.work_dir = Some(temp.path().to_string_lossy().into_owned());
        crate::core::connector::mcp::config::add_global_server(
            &cfg,
            "login".into(),
            serde_json::from_value(
                serde_json::json!({"url":"http://127.0.0.1:9/mcp", "auth":"none"}),
            )
            .unwrap(),
        )
        .unwrap();
        let manager = McpManager::new(&cfg, None).unwrap();
        let key = manager.configured_server("login").unwrap().config_key;
        (temp, manager, cfg, key)
    }
    fn token(value: &str) -> StoredOAuthToken {
        serde_json::from_value(serde_json::json!({"accessToken":value,"tokenEndpoint":"http://127.0.0.1/token","clientId":"fixture"})).unwrap()
    }

    #[tokio::test]
    async fn older_login_cannot_save_erase_cancel_or_reconnect_a_newer_login() {
        let (_temp, manager, _, key) = fixture();
        let (_, generation, old_snapshot, old) = manager.begin_login(&key).unwrap();
        let (_, current_generation, current_snapshot, current) = manager.begin_login(&key).unwrap();
        assert!(old.is_cancelled());
        manager
            .commit_login(
                &key,
                current_generation,
                &current,
                &current_snapshot,
                token("new"),
            )
            .unwrap();
        assert!(manager
            .commit_login(&key, generation, &old, &old_snapshot, token("late-old"))
            .is_err());
        assert!(manager
            .reconnect_after_login(&key, generation, old.clone())
            .await
            .is_err());
        drop(LoginOwner {
            manager: &manager,
            key: key.clone(),
            cancel: old,
        });
        assert!(!current.is_cancelled());
        assert!(manager
            .oauth_cancellations
            .get(&key)
            .is_some_and(|value| Arc::ptr_eq(value.value(), &current)));
        assert_eq!(
            manager
                .oauth_store
                .load(&key)
                .unwrap()
                .unwrap()
                .access_token,
            "new"
        );
    }

    #[test]
    fn revoke_and_replacement_do_not_resurrect_or_erase_credentials() {
        for action in ["logout", "remove-readd"] {
            let (_temp, manager, cfg, key) = fixture();
            let (definition, generation, expected, cancel) = manager.begin_login(&key).unwrap();
            match action {
                "logout" => {
                    assert!(!manager.logout_server(&key).unwrap());
                }
                _ => {
                    manager.remove_configured_server(&key, &cfg).unwrap();
                    crate::core::connector::mcp::config::add_global_server(
                        &cfg,
                        "login".into(),
                        definition.config,
                    )
                    .unwrap();
                    manager.reload_configuration(&cfg).unwrap();
                }
            }
            manager.oauth_store.save(&key, token("new-owner")).unwrap();
            assert!(manager
                .commit_login(&key, generation, &cancel, &expected, token("late-old"))
                .is_err());
            assert_eq!(
                manager
                    .oauth_store
                    .load(&key)
                    .unwrap()
                    .unwrap()
                    .access_token,
                "new-owner"
            );
        }
    }
}
