use super::super::noop::NoopStore;
use super::super::store::CheckpointStore;
use super::super::types::{
    CheckpointKind, CheckpointRecordRequest, ListOptions, RetentionPolicy,
};

#[test]
fn noop_store_returns_null_and_empty_views() {
    let store = NoopStore;
    let id = store
        .record(CheckpointRecordRequest {
            session_id: "s1".to_string(),
            turn_id: "t1".to_string(),
            kind: CheckpointKind::TurnEnd,
            message_anchor: None,
            notes: None,
        })
        .unwrap();
    assert!(id.is_null());
    assert!(store.list("s1", ListOptions::default()).unwrap().is_empty());
    assert!(store.show(&id).unwrap().is_none());
    assert_eq!(store.prune(RetentionPolicy::default()).unwrap(), 0);
}
