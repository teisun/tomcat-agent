use super::store::CheckpointStore;
use super::types::{
    CheckpointDiff, CheckpointError, CheckpointId, CheckpointMeta, CheckpointRecordRequest,
    CheckpointRestoreReport, ListOptions, RestoreOptions, RetentionPolicy,
};

#[derive(Debug, Default)]
pub struct NoopStore;

impl CheckpointStore for NoopStore {
    fn record(&self, _request: CheckpointRecordRequest) -> Result<CheckpointId, CheckpointError> {
        Ok(CheckpointId::null())
    }

    fn list(
        &self,
        _session_id: &str,
        _opts: ListOptions,
    ) -> Result<Vec<CheckpointMeta>, CheckpointError> {
        Ok(Vec::new())
    }

    fn show(&self, _id: &CheckpointId) -> Result<Option<CheckpointMeta>, CheckpointError> {
        Ok(None)
    }

    fn diff(&self, _id: &CheckpointId) -> Result<CheckpointDiff, CheckpointError> {
        Err(CheckpointError::Unsupported(
            "checkpoint diff unavailable without git".to_string(),
        ))
    }

    fn restore(
        &self,
        _id: &CheckpointId,
        _opts: RestoreOptions,
    ) -> Result<CheckpointRestoreReport, CheckpointError> {
        Err(CheckpointError::Unsupported(
            "checkpoint restore unavailable without git".to_string(),
        ))
    }

    fn prune(&self, _retention: RetentionPolicy) -> Result<usize, CheckpointError> {
        Ok(0)
    }
}
