//! User-owned project trust, independent of any particular connector or tool.
use std::collections::BTreeSet;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::core::session::project_root;
use crate::infra::config::get_work_dir;
use crate::infra::error::AppError;
use crate::infra::i18n::tr;
use crate::AppConfig;

#[derive(Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProjectTrustFile {
    trusted_projects: BTreeSet<PathBuf>,
}

/// All agents using the same Tomcat work directory share this one state file.
#[derive(Debug, Clone)]
pub struct ProjectTrustStore {
    path: PathBuf,
}

impl ProjectTrustStore {
    pub fn open(cfg: &AppConfig) -> Result<Self, AppError> {
        Ok(Self {
            path: get_work_dir(cfg)?.join("project-trust.json"),
        })
    }

    /// Convert an existing absolute folder to the same identity used by sessions.
    pub fn root_for(path: &Path) -> Result<PathBuf, AppError> {
        if !path.is_absolute() {
            return Err(AppError::Config(tr("trust.absolute", &[])));
        }
        let canonical = std::fs::canonicalize(path).map_err(|error| {
            AppError::Config(tr(
                "trust.directory",
                &[
                    ("path", &path.display().to_string()),
                    ("detail", &error.to_string()),
                ],
            ))
        })?;
        if !canonical.is_dir() {
            return Err(AppError::Config(tr(
                "trust.notDirectory",
                &[("path", &path.display().to_string())],
            )));
        }
        Ok(project_root(&canonical))
    }

    /// Mutating requests must name the root the user saw, not an arbitrary child.
    pub fn validate_root(path: &Path) -> Result<PathBuf, AppError> {
        let root = Self::root_for(path)?;
        if path != root {
            return Err(AppError::Config(tr(
                "trust.notCanonical",
                &[
                    ("path", &path.display().to_string()),
                    ("root", &root.display().to_string()),
                ],
            )));
        }
        Ok(root)
    }

    /// A corrupt or unreadable record is an error, never implicit consent.
    pub fn is_trusted(&self, root: &Path) -> Result<bool, AppError> {
        Ok(self.read()?.trusted_projects.contains(root))
    }

    pub fn trust(&self, root: &Path) -> Result<(), AppError> {
        let root = Self::validate_root(root)?;
        let parent = self
            .path
            .parent()
            .ok_or_else(|| AppError::Config(tr("trust.noParent", &[])))?;
        std::fs::create_dir_all(parent)?;
        // Lock a stable sidecar: write_file_atomic replaces the data file inode.
        let lock_path = self.path.with_extension("lock");
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)?;
        lock.lock_exclusive()?;
        let result = self.trust_locked(root);
        // A dropped File releases the lock even if read or write failed.
        drop(lock);
        result
    }

    fn trust_locked(&self, root: PathBuf) -> Result<(), AppError> {
        let mut state = self.read()?;
        if state.trusted_projects.insert(root) {
            let bytes = serde_json::to_vec_pretty(&state).map_err(|error| {
                AppError::Config(tr("trust.serialize", &[("detail", &error.to_string())]))
            })?;
            crate::infra::platform::write_file_atomic(&self.path, &bytes)?;
        }
        Ok(())
    }

    fn read(&self) -> Result<ProjectTrustFile, AppError> {
        if !self.path.exists() {
            return Ok(ProjectTrustFile::default());
        }
        let bytes = std::fs::read(&self.path)?;
        serde_json::from_slice(&bytes).map_err(|error| {
            AppError::Config(tr(
                "trust.read",
                &[
                    ("path", &self.path.display().to_string()),
                    ("detail", &error.to_string()),
                ],
            ))
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}
