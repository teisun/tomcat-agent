use super::wire_preview_tail;

#[test]
fn lag_snapshot_tail_is_utf8_bounded_and_keeps_durable_offset() {
    let output = format!("{}€tail", "x".repeat(9 * 1024));
    let original_len = output.len();
    let (tail, start_offset) = wire_preview_tail(output, 100);
    assert!(tail.len() <= 8 * 1024);
    assert!(tail.is_char_boundary(0));
    assert_eq!(start_offset + tail.len() as u64, 100 + original_len as u64);
    assert!(tail.ends_with("€tail"));
}

/// Regression: the outer tool dispatcher's `biased` `tokio::select!` wins the
/// cancellation race and drops the `handle_bash` future before any inline
/// `cancel.cancelled()` arm can run. The detached cancel watcher must survive that drop
/// and still tear down the tracked process group, otherwise a cancelled foreground
/// command leaks a live `sleep 30` (here) for its full duration.
#[cfg(unix)]
#[tokio::test]
async fn cancel_watcher_stops_tracked_task_after_future_drop() {
    use crate::core::tools::primitive::{BashTaskRegistry, BashTaskStatus};
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    let dir = tempfile::tempdir().expect("tempdir");
    let registry = Arc::new(BashTaskRegistry::new(dir.path().join("tool-results")));
    let ticket = registry
        .spawn_tracked("sleep 30".to_string(), None::<std::path::PathBuf>, false)
        .await
        .expect("spawn tracked");

    let cancel = CancellationToken::new();
    let watcher =
        super::spawn_cancel_stop(cancel.clone(), registry.clone(), ticket.task_id.clone());
    // Mirror the dispatcher dropping `handle_bash`: dropping the JoinHandle detaches the
    // watcher rather than aborting it.
    drop(watcher);
    cancel.cancel();

    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        registry.wait_for_finish(&ticket.task_id),
    )
    .await
    .expect("cancellation must stop the process well before `sleep 30` finishes")
    .expect("wait_for_finish");

    let info = registry.get_info(&ticket.task_id).expect("task info");
    assert!(
        !matches!(info.status, BashTaskStatus::Running),
        "cancelled task must leave Running, got {:?}",
        info.status
    );
}
