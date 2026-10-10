//! OS-backed occupancy, not a second session registry. Recheck metadata after acquiring a lock.
use super::attachments::validate_session_id;
use crate::AppError;
use fs2::FileExt;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

pub(crate) struct SessionUsageGuard {
    _file: File,
    path: PathBuf,
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
            if error.kind() == std::io::ErrorKind::WouldBlock {
                AppError::Config("session_in_use".into())
            } else {
                AppError::Io(error)
            }
        })?;
        Ok(Self { _file: file, path })
    }
    /// Only after metadata removal. IDs are never reused and every new holder rechecks existence.
    pub(crate) fn remove_deleted_lock(&self) -> std::io::Result<()> {
        match std::fs::remove_file(&self.path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            result => result,
        }
    }
}
