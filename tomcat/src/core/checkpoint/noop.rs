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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_store_returns_null_and_empty_views() {
        let store = NoopStore;
        let id = store
            .record(CheckpointRecordRequest {
                session_id: "s1".to_string(),
                turn_id: "t1".to_string(),
                kind: super::super::types::CheckpointKind::TurnEnd,
                message_anchor: None,
                notes: None,
            })
            .unwrap();
        assert!(id.is_null());
        assert!(store.list("s1", ListOptions::default()).unwrap().is_empty());
        assert!(store.show(&id).unwrap().is_none());
        assert_eq!(store.prune(RetentionPolicy::default()).unwrap(), 0);
    }
}
