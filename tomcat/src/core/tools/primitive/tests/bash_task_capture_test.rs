use super::*;
use std::time::Duration;

#[tokio::test]
async fn failed_capture_is_reported_after_draining_the_pipe() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("readonly-capture");
    std::fs::write(&path, "").unwrap();
    let capture = tokio::fs::File::from_std(std::fs::File::open(path).unwrap());
    let log = dir.path().join("mixed.log");
    let writer = Arc::new(AsyncMutex::new(
        tokio::fs::File::create(&log).await.unwrap(),
    ));
    let task = Arc::new(BashTask {
        info: RwLock::new(BashTaskInfo {
            task_id: "capture-error".into(),
            command: "owned pipe fixture".into(),
            started_at_unix_ms: 0,
            log_path: log.display().to_string(),
            status: BashTaskStatus::Running,
        }),
        pid: None,
        notify: Arc::new(Notify::new()),
        lifecycle_emitted: parking_lot::Mutex::new(false),
        delivery: parking_lot::Mutex::new(TaskDelivery::Foreground),
        preview: parking_lot::Mutex::new(RuntimePreview::default()),
        stop_requested: AtomicBool::new(false),
        preview_flush_required: false,
        preview_flushed: AtomicBool::new(true),
        preview_flush_notify: Arc::new(Notify::new()),
    });
    let (sender, _receiver) = tokio::sync::oneshot::channel();
    let (output_tx, _) = broadcast::channel(256);
    // The payload exceeds both the pipe and pump buffer: verify complete drain without
    // forcing hundreds of tiny filesystem flushes in this failure-path fixture.
    let (mut input, reader) = tokio::io::duplex(4096);
    let bytes = b"pipe content\n".repeat(2_048);
    let expected = bytes.clone();
    let producing = tokio::spawn(async move {
        input.write_all(&bytes).await.unwrap();
        input.shutdown().await.unwrap();
    });
    let pump = spawn_pump(
        Some(reader),
        writer,
        Some((capture, Arc::new(sender))),
        BashOutputStream::Stdout,
        task,
        output_tx,
    );
    let result = tokio::time::timeout(Duration::from_secs(5), pump)
        .await
        .unwrap()
        .unwrap();
    assert!(
        result.is_err(),
        "capture failure must not become successful empty output"
    );
    producing.await.unwrap();
    assert_eq!(
        tokio::fs::read(log).await.unwrap(),
        expected,
        "the existing mixed log keeps draining even when a foreground capture fails"
    );
}

#[tokio::test]
async fn closed_capture_owner_keeps_background_log_without_sidecar_files() {
    let dir = tempfile::tempdir().unwrap();
    let registry = BashTaskRegistry::new(dir.path().to_path_buf());
    let (ticket, capture) = registry
        .spawn_foreground_unchecked("echo stream-owner".into(), ResolvedCwd::inherited())
        .await
        .unwrap();
    drop(capture);
    registry.wait_for_finish(&ticket.task_id).await.unwrap();
    assert!(registry
        .read_output(&ticket.task_id, None)
        .await
        .unwrap()
        .content
        .contains("stream-owner"));
    let files: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(
        files,
        vec![PathBuf::from(ticket.log_path)],
        "captures are anonymous and must not add retained stdout/stderr sidecars"
    );
}
