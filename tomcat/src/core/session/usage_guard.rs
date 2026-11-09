//! OS-backed occupancy, not a second session registry. Recheck metadata after acquiring a lock.
use super::attachments::validate_session_id;
use crate::AppError;
use fs2::FileExt;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

pub(crate) struct SessionUsageGuard {
    _file: File,
    path: PathBuf,
    owner_pid: u32,
}

impl Drop for SessionUsageGuard {
    fn drop(&mut self) {
        // Command's pre-exec child may still hold a CLOEXEC copy of this open
        // description. Only the process that acquired the lock owns its lifetime.
        if self.owner_pid == std::process::id() {
            let _ = FileExt::unlock(&self._file);
        }
    }
}

impl SessionUsageGuard {
    pub(crate) fn shared(sessions_dir: &Path, id: &str) -> Result<Self, AppError> {
        Self::acquire(sessions_dir, id, false)
    }
    pub(crate) fn exclusive(sessions_dir: &Path, id: &str) -> Result<Self, AppError> {
        Self::acquire(sessions_dir, id, true)
    }
    fn acquire(sessions_dir: &Path, id: &str, exclusive: bool) -> Result<Self, AppError> {
        validate_session_id(id)?;
        let directory = sessions_dir.join("locks");
        std::fs::create_dir_all(&directory)?;
        let path = directory.join(format!("{id}.lock"));
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)?;
        let result = if exclusive {
            FileExt::try_lock_exclusive(&file)
        } else {
            FileExt::try_lock_shared(&file)
        };
        result.map_err(|error| {
            if crate::infra::platform::is_lock_contended(&error) {
                AppError::Config("session_in_use".into())
            } else {
                AppError::Io(error)
            }
        })?;
        Ok(Self {
            _file: file,
            path,
            owner_pid: std::process::id(),
        })
    }
    /// Only after metadata removal. IDs are never reused and every new holder rechecks existence.
    pub(crate) fn remove_deleted_lock(&self) -> std::io::Result<()> {
        match std::fs::remove_file(&self.path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            result => result,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn foreign_process_drop_does_not_unlock_owner() {
        let temp = tempfile::tempdir().unwrap();
        let mut guard = SessionUsageGuard::shared(temp.path(), "owned-session").unwrap();
        let inherited = guard._file.try_clone().unwrap();
        // Exercise a copied guard's foreign-process branch with a real shared
        // open description, without running Rust destructors after an unsafe fork.
        guard.owner_pid = std::process::id().wrapping_add(1);
        drop(guard);
        assert!(matches!(
            SessionUsageGuard::exclusive(temp.path(), "owned-session"),
            Err(AppError::Config(code)) if code == "session_in_use"
        ));
        drop(inherited);
        assert!(SessionUsageGuard::exclusive(temp.path(), "owned-session").is_ok());
    }
}
