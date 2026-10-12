use super::*;
use crate::infra::DefaultEventBus;

#[test]
fn clamp_host_task_output_wait_ms_caps_at_sixty_seconds() {
    assert_eq!(clamp_host_task_output_wait_ms(None), 5_000);
    assert_eq!(clamp_host_task_output_wait_ms(Some(0)), 5_000);
    assert_eq!(clamp_host_task_output_wait_ms(Some(1_000)), 5_000);
    assert_eq!(clamp_host_task_output_wait_ms(Some(60_000)), 60_000);
    assert_eq!(clamp_host_task_output_wait_ms(Some(600_000)), 60_000);
}

#[tokio::test]
async fn do_task_output_block_false_returns_current_chunk() {
    let dir = tempfile::tempdir().expect("tempdir");
    let registry = Arc::new(BashTaskRegistry::new(dir.path().join("tool-results")));
    let ticket = registry
        .spawn(
            "printf ext-host-tail".to_string(),
            Some(dir.path().to_path_buf()),
        )
        .await
        .expect("spawn");
    registry
        .wait_for_finish(&ticket.task_id)
        .await
        .expect("finish");
    let dispatcher =
        HostApiDispatcher::new(Arc::new(DefaultEventBus::new())).with_bash_task_registry(registry);

    let response = dispatcher
        .do_task_output("plugin", &serde_json::json!({ "taskId": ticket.task_id }))
        .await
        .expect("host response");
    assert!(response.ok);
    let data = response.data.expect("chunk data");
    assert_eq!(data["finished"], serde_json::json!(true));
    assert!(data["content"]
        .as_str()
        .unwrap_or_default()
        .contains("ext-host-tail"));
}

#[tokio::test]
async fn do_task_output_unknown_task_id_returns_err() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dispatcher = HostApiDispatcher::new(Arc::new(DefaultEventBus::new()))
        .with_bash_task_registry(Arc::new(BashTaskRegistry::new(
            dir.path().join("tool-results"),
        )));

    let err = dispatcher
        .do_task_output("plugin", &serde_json::json!({ "taskId": "missing-task" }))
        .await
        .expect_err("unknown task id should error");
    assert!(err.to_string().contains("bash task not found"));
}
