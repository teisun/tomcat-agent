use super::*;
use crate::infra::{wire, DefaultEventBus, EventBus, EventContext};

async fn wait_until_finished(
    registry: &Arc<crate::core::tools::primitive::BashTaskRegistry>,
    task_id: &str,
) -> crate::core::tools::primitive::BashTaskOutputChunk {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        let chunk = registry
            .read_output(task_id, Some(0))
            .await
            .expect("read_output");
        if chunk.finished {
            return chunk;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "task {task_id} did not finish within 3s; last chunk={chunk:?}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

async fn wait_until_has_output(
    registry: &Arc<crate::core::tools::primitive::BashTaskRegistry>,
    task_id: &str,
    since: u64,
) -> crate::core::tools::primitive::BashTaskOutputChunk {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        let chunk = registry
            .read_output(task_id, Some(since))
            .await
            .expect("read_output");
        if !chunk.content.is_empty() {
            return chunk;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "task {task_id} produced no output within 3s from offset {since}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

#[tokio::test]
async fn finish_blocking_with_finished_marks_delivered() {
    use crate::core::tools::primitive::BashTaskRegistry;

    let dir = tempfile::tempdir().expect("tempdir");
    let registry = Arc::new(BashTaskRegistry::new(dir.path().join("tool-results")));
    let ticket = registry
        .spawn("echo done; exit 0".to_string(), None)
        .await
        .expect("spawn");

    let _ = wait_until_finished(&registry, &ticket.task_id).await;

    let (text, delivered) =
        finish_blocking_with(&registry, &ticket.task_id, 0, BlockingWakeKind::Finished)
            .await
            .expect("finish_blocking_with");
    let chunk: serde_json::Value = serde_json::from_str(&text).expect("valid json");

    assert_eq!(
        chunk["wakeReason"],
        serde_json::Value::String("finished".into())
    );
    assert_eq!(chunk["finished"], serde_json::Value::Bool(true));
    assert!(delivered, "finished must claim Delivered");
}

#[test]
fn clamp_block_timeout_preserves_zero_and_bounds_wait_slices() {
    assert_eq!(clamp_block_wait_ms(None), 5_000);
    assert_eq!(clamp_block_wait_ms(Some(0)), 5_000);
    assert_eq!(clamp_block_wait_ms(Some(1)), 5_000);
    assert_eq!(clamp_block_wait_ms(Some(4_999)), 5_000);
    assert_eq!(clamp_block_wait_ms(Some(5_000)), 5_000);
    assert_eq!(clamp_block_wait_ms(Some(600_000)), 600_000);
    assert_eq!(clamp_block_wait_ms(Some(999_999_999)), 600_000);
}

#[tokio::test]
async fn finish_blocking_with_timeout_returns_empty_content_when_no_new_output_since() {
    use crate::core::tools::primitive::BashTaskRegistry;

    let dir = tempfile::tempdir().expect("tempdir");
    let registry = Arc::new(BashTaskRegistry::new(dir.path().join("tool-results")));
    let ticket = registry
        .spawn("printf SNAPSHOT_TIMEOUT; sleep 3".to_string(), None)
        .await
        .expect("spawn");

    let first = wait_until_has_output(&registry, &ticket.task_id, 0).await;
    assert!(
        first.content.contains("SNAPSHOT_TIMEOUT"),
        "首个增量应包含 token，实际 {:?}",
        first.content
    );

    let (text, delivered) = finish_blocking_with(
        &registry,
        &ticket.task_id,
        first.next_offset,
        BlockingWakeKind::WaitWindowElapsed,
    )
    .await
    .expect("finish_blocking_with");
    let chunk: serde_json::Value = serde_json::from_str(&text).expect("valid json");

    assert_eq!(
        chunk["wakeReason"],
        serde_json::Value::String("wait_window_elapsed".into())
    );
    assert_eq!(chunk["finished"], serde_json::Value::Bool(false));
    assert_eq!(chunk["content"], serde_json::Value::String(String::new()));
    assert_eq!(chunk["nextOffset"].as_u64(), Some(first.next_offset));
    assert!(!delivered, "running timeout 不应 claim Delivered");
}

#[tokio::test]
async fn finish_blocking_with_timeout_promotes_finished_race() {
    use crate::core::tools::primitive::BashTaskRegistry;

    let dir = tempfile::tempdir().expect("tempdir");
    let registry = Arc::new(BashTaskRegistry::new(dir.path().join("tool-results")));
    let ticket = registry
        .spawn("printf FINISHED_TIMEOUT".to_string(), None)
        .await
        .expect("spawn");

    let _ = wait_until_finished(&registry, &ticket.task_id).await;
    let (text, delivered) = finish_blocking_with(
        &registry,
        &ticket.task_id,
        0,
        BlockingWakeKind::WaitWindowElapsed,
    )
    .await
    .expect("finish_blocking_with");
    let chunk: serde_json::Value = serde_json::from_str(&text).expect("valid json");

    assert_eq!(
        chunk["wakeReason"],
        serde_json::Value::String("finished".into())
    );
    assert_eq!(chunk["finished"], serde_json::Value::Bool(true));
    assert!(delivered, "timeout 命中已 finished 应 claim Delivered");
}

#[tokio::test]
async fn finish_blocking_with_timeout_caps_tail_and_advances_to_eof() {
    use crate::core::tools::primitive::BashTaskRegistry;

    let dir = tempfile::tempdir().expect("tempdir");
    let registry = Arc::new(BashTaskRegistry::new(dir.path().join("tool-results")));
    let ticket = registry
        .spawn(
            "awk 'BEGIN{for(i=0;i<7000;i++) printf \"A\"}'; sleep 3".to_string(),
            None,
        )
        .await
        .expect("spawn");

    let _ = wait_until_has_output(&registry, &ticket.task_id, 0).await;
    let (text, delivered) = finish_blocking_with(
        &registry,
        &ticket.task_id,
        0,
        BlockingWakeKind::WaitWindowElapsed,
    )
    .await
    .expect("finish_blocking_with");
    let chunk: serde_json::Value = serde_json::from_str(&text).expect("valid json");

    assert_eq!(
        chunk["wakeReason"],
        serde_json::Value::String("wait_window_elapsed".into())
    );
    let content = chunk["content"].as_str().unwrap_or_default();
    assert!(
        content.len() <= TASK_OUTPUT_WAIT_TAIL_MAX_BYTES as usize,
        "timeout tail 长度 {} 不应超过上限 {}",
        content.len(),
        TASK_OUTPUT_WAIT_TAIL_MAX_BYTES
    );
    let start_offset = chunk["startOffset"].as_u64().unwrap_or(0);
    let next_offset = chunk["nextOffset"].as_u64().unwrap_or(0);
    assert!(start_offset > 0, "大输出 timeout 应前移 startOffset");
    assert!(
        next_offset >= 7000,
        "nextOffset 应指向当前 EOF，实际 {}",
        next_offset
    );
    assert!(!delivered, "running timeout 不应 claim Delivered");

    let _ = registry.stop(&ticket.task_id).await;
}

#[test]
fn task_output_update_event_inherits_scoped_session_id() {
    let bus: Arc<dyn EventBus> = Arc::new(DefaultEventBus::new());
    let captured = Arc::new(std::sync::Mutex::new(None::<EventContext>));
    let captured_cb = Arc::clone(&captured);
    bus.on(
        wire::WIRE_TOOL_EXECUTION_UPDATE,
        Box::new(move |ctx: EventContext| {
            *captured_cb.lock().unwrap() = Some(ctx);
            Ok(())
        }),
    );
    let emitter = ScopedEventEmitter::new(Arc::clone(&bus), "sid-task-output");
    let tc = ToolCallInfo {
        id: "call-task".to_string(),
        name: "task_output".to_string(),
        arguments: r#"{"task_id":"task-1","since":0,"block":true,"wait_ms":150}"#.to_string(),
    };

    emit_task_output_update(
        Some(&emitter),
        &tc,
        "task-1",
        0,
        150,
        100,
        "waiting_for_output",
        None,
    );

    let ctx = captured
        .lock()
        .unwrap()
        .clone()
        .expect("应捕获到 tool_execution_update");
    assert_eq!(ctx.session_id.as_deref(), Some("sid-task-output"));
    assert_eq!(
        ctx.payload.get("sessionId").and_then(|v| v.as_str()),
        Some("sid-task-output")
    );
    assert_eq!(
        ctx.payload.get("toolName").and_then(|v| v.as_str()),
        Some("task_output")
    );
}
