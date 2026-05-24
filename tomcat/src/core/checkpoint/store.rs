use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

use parking_lot::RwLock;

use super::types::{
    CheckpointDiff, CheckpointError, CheckpointId, CheckpointMeta, CheckpointRecordRequest,
    CheckpointRestoreReport, ListOptions, RestoreOptions, RetentionPolicy,
};
use super::{NoopStore, ShadowGitStore};

pub trait CheckpointStore: Send + Sync {
    fn record(&self, request: CheckpointRecordRequest) -> Result<CheckpointId, CheckpointError>;

    fn list(
        &self,
        session_id: &str,
        opts: ListOptions,
    ) -> Result<Vec<CheckpointMeta>, CheckpointError>;

    fn show(&self, id: &CheckpointId) -> Result<Option<CheckpointMeta>, CheckpointError>;

    fn diff(&self, id: &CheckpointId) -> Result<CheckpointDiff, CheckpointError>;

    fn restore(
        &self,
        id: &CheckpointId,
        opts: RestoreOptions,
    ) -> Result<CheckpointRestoreReport, CheckpointError>;

    fn prune(&self, retention: RetentionPolicy) -> Result<usize, CheckpointError>;
}

const STORE_KIND_NOOP: u8 = 0;
const STORE_KIND_SHADOW: u8 = 1;

pub struct SwitchingCheckpointStore {
    inner: RwLock<Arc<dyn CheckpointStore>>,
    agent_trail_dir: PathBuf,
    work_tree: PathBuf,
    kind: AtomicU8,
}

impl SwitchingCheckpointStore {
    pub fn new(agent_trail_dir: PathBuf, work_tree: PathBuf, git_available: bool) -> Self {
        let (inner, kind) = if git_available {
            (
                Arc::new(ShadowGitStore::new(
                    agent_trail_dir.clone(),
                    work_tree.clone(),
                )) as Arc<dyn CheckpointStore>,
                STORE_KIND_SHADOW,
            )
        } else {
            (
                Arc::new(NoopStore) as Arc<dyn CheckpointStore>,
                STORE_KIND_NOOP,
            )
        };
        Self {
            inner: RwLock::new(inner),
            agent_trail_dir,
            work_tree,
            kind: AtomicU8::new(kind),
        }
    }

    pub fn is_shadow(&self) -> bool {
        self.kind.load(Ordering::Acquire) == STORE_KIND_SHADOW
    }

    pub fn force_activate_shadow(&self) {
        self.activate_shadow_if_available(true);
    }

    fn current(&self) -> Arc<dyn CheckpointStore> {
        self.activate_shadow_if_available(false);
        self.inner.read().clone()
    }

    fn activate_shadow_if_available(&self, force: bool) {
        if self.is_shadow() {
            return;
        }
        if !force && !git_available() {
            return;
        }
        let mut guard = self.inner.write();
        if self.kind.load(Ordering::Acquire) == STORE_KIND_SHADOW {
            return;
        }
        *guard = Arc::new(ShadowGitStore::new(
            self.agent_trail_dir.clone(),
            self.work_tree.clone(),
        ));
        self.kind.store(STORE_KIND_SHADOW, Ordering::Release);
    }
}

impl CheckpointStore for SwitchingCheckpointStore {
    fn record(&self, request: CheckpointRecordRequest) -> Result<CheckpointId, CheckpointError> {
        self.current().record(request)
    }

    fn list(
        &self,
        session_id: &str,
        opts: ListOptions,
    ) -> Result<Vec<CheckpointMeta>, CheckpointError> {
        self.current().list(session_id, opts)
    }

    fn show(&self, id: &CheckpointId) -> Result<Option<CheckpointMeta>, CheckpointError> {
        self.current().show(id)
    }

    fn diff(&self, id: &CheckpointId) -> Result<CheckpointDiff, CheckpointError> {
        self.current().diff(id)
    }

    fn restore(
        &self,
        id: &CheckpointId,
        opts: RestoreOptions,
    ) -> Result<CheckpointRestoreReport, CheckpointError> {
        self.current().restore(id, opts)
    }

    fn prune(&self, retention: RetentionPolicy) -> Result<usize, CheckpointError> {
        self.current().prune(retention)
    }
}

fn git_available() -> bool {
    std::process::Command::new("git")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

