//! One guarded deletion boundary shared by CLI, Serve and creation compensation.
use super::SessionManager;
use crate::core::session::{
    attachments::validate_session_id, resume_index::remove_resume_index,
    tool_display_sidecar::tool_display_sidecar_path, usage_guard::SessionUsageGuard,
    user_message_sidecar::user_message_sidecar_path, SessionEntry,
};
use crate::AppError;
use std::path::Path;

#[derive(Debug, Clone, serde::Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DeleteSessionOutcome {
    pub warnings: Vec<String>,
}

pub struct SessionDeletion<'a> {
    manager: &'a SessionManager,
    id: String,
    entry: Option<SessionEntry>,
    guard: Option<SessionUsageGuard>,
}

impl SessionManager {
    pub fn begin_delete_session(&self, id: &str) -> Result<SessionDeletion<'_>, AppError> {
        validate_session_id(id)?;
        let entry = self.get_session_by_id(id)?;
        if let Some(entry) = &entry {
            if entry.session_key != self.current_session_key() {
                return Err(AppError::Config("session_scope_mismatch".into()));
            }
        } else {
            return Ok(SessionDeletion {
                manager: self,
                id: id.into(),
                entry: None,
                guard: None,
            });
        }
        let guard = SessionUsageGuard::exclusive(self.sessions_dir(), id)?;
        // A process may have deleted the target while this caller was opening the lock file.
        let entry = self.get_session_by_id(id)?;
        if entry
            .as_ref()
            .is_some_and(|e| e.session_key != self.current_session_key())
        {
            return Err(AppError::Config("session_scope_mismatch".into()));
        }
        Ok(SessionDeletion {
            manager: self,
            id: id.into(),
            entry,
            guard: Some(guard),
        })
    }

    pub fn delete_session(&self, id: &str) -> Result<DeleteSessionOutcome, AppError> {
        self.begin_delete_session(id)?.commit(None)
    }
}

impl SessionDeletion<'_> {
    pub fn exists(&self) -> bool {
        self.entry.is_some()
    }

    /// The trusted trail root comes from configuration, never from frontend paths.
    pub fn commit(self, trail_dir: Option<&Path>) -> Result<DeleteSessionOutcome, AppError> {
        let mut warnings = Vec::new();
        if let Some(entry) = &self.entry {
            self.manager.with_store_mut(|store| {
                if store
                    .sessions
                    .get(&self.id)
                    .is_some_and(|e| e.session_key != entry.session_key)
                {
                    return Err(AppError::Config("session_scope_mismatch".into()));
                }
                store.sessions.remove(&self.id);
                SessionManager::repoint_current_after_removal(store, &entry.session_key, &self.id);
                Ok(())
            })?;
            let transcript = self.manager.transcript_path(&self.id);
            let preheat = crate::core::session::preheat_cache::preheat_cache_path(&transcript);
            for path in [
                &transcript,
                &user_message_sidecar_path(&transcript),
                &tool_display_sidecar_path(&transcript),
                &preheat,
            ] {
                record_cleanup(std::fs::remove_file(path), path, &mut warnings);
            }
            if let Err(error) = remove_resume_index(&transcript) {
                warnings.push(error.to_string());
            }
            let baselines = crate::core::checkpoint::file_baselines::session_dir(&transcript);
            record_cleanup(
                std::fs::remove_dir_all(&baselines),
                &baselines,
                &mut warnings,
            );
            if let Some(root) = trail_dir {
                let todo = crate::core::plan_runtime::todo_runtime::todo_path_for(root, &self.id);
                record_cleanup(std::fs::remove_file(&todo), &todo, &mut warnings);
                let results = root.join("tool-results").join(&self.id);
                record_cleanup(std::fs::remove_dir_all(&results), &results, &mut warnings);
            }
            let store = self.manager.attachment_store();
            if let Err(error) = store.clear_session(&self.id) {
                warnings.push(error.to_string());
            }
            match self.manager.collect_live_blob_shas() {
                Ok(mut live) => {
                    if let Err(error) =
                        store.gc_pending(crate::core::session::attachments::PENDING_BLOB_TTL)
                    {
                        warnings.push(error.to_string());
                    }
                    match store.collect_pending_blob_shas() {
                        Ok(pending) => {
                            live.refresh_pending_blob_shas(pending);
                            if let Err(error) = store.sweep_orphan_blobs(
                                &live.shas,
                                crate::core::session::attachments::ORPHAN_BLOB_GRACE,
                            ) {
                                warnings.push(error.to_string());
                            }
                        }
                        Err(error) => warnings.push(error.to_string()),
                    }
                }
                Err(error) => warnings.push(error.to_string()),
            }
        }
        if let Some(guard) = self.guard {
            if let Err(error) = guard.remove_deleted_lock() {
                warnings.push(error.to_string());
            }
        }
        Ok(DeleteSessionOutcome { warnings })
    }
}

fn record_cleanup(result: std::io::Result<()>, path: &Path, warnings: &mut Vec<String>) {
    if let Err(error) = result {
        if error.kind() != std::io::ErrorKind::NotFound {
            warnings.push(format!("{}: {error}", path.display()));
        }
    }
}
