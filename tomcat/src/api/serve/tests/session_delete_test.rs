use super::*;
use serial_test::serial;
use std::sync::Arc;

async fn wait_for_line(
    buffer: &SharedWriterBuffer,
    predicate: impl Fn(&serde_json::Value) -> bool,
) -> Vec<serde_json::Value> {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let lines = read_ndjson_lines(buffer);
            if lines.iter().any(&predicate) {
                return lines;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("deletion response must arrive")
}

#[tokio::test]
#[serial(env_lock)]
async fn session_delete_rejects_busy_then_deletes_idle_with_lingering_context() {
    let _key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;
    let id = slot.session_id.clone();
    slot.mark_busy();
    handle_command(
        Arc::clone(&state),
        ServeCommand::DeleteSession {
            id: Some("busy-delete".into()),
            session_id: id.clone(),
        },
    )
    .await
    .unwrap();
    let frames = wait_for_line(&buffer, |v| v["id"] == "busy-delete").await;
    assert!(frames
        .iter()
        .any(|v| v["id"] == "busy-delete" && v["error"] == "busy"));
    assert!(slot
        .ctx
        .session_runtime
        .session
        .transcript_path(&id)
        .exists());
    slot.mark_idle();
    handle_command(
        Arc::clone(&state),
        ServeCommand::DeleteSession {
            id: Some("idle-delete".into()),
            session_id: id.clone(),
        },
    )
    .await
    .unwrap();
    let frames = wait_for_line(&buffer, |v| v["id"] == "idle-delete").await;
    assert!(frames.iter().any(|v| v["id"] == "idle-delete"
        && v["success"] == true
        && v["payload"]["warnings"].is_array()));
    assert!(state.registry.get(&id).is_none());
    assert!(!slot
        .ctx
        .session_runtime
        .session
        .transcript_path(&id)
        .exists());
    handle_command(
        Arc::clone(&state),
        ServeCommand::ListSessions {
            id: Some("empty-disk".into()),
            scope: Some(ListSessionsScope::Disk),
        },
    )
    .await
    .unwrap();
    let frames = wait_for_line(&buffer, |v| v["id"] == "empty-disk").await;
    assert!(frames.iter().any(|v| v["id"] == "empty-disk"
        && v["success"] == true
        && v["payload"]["sessions"]
            .as_array()
            .is_some_and(Vec::is_empty)
        && v["payload"]["sessionKey"].is_string()));
}

#[tokio::test]
#[serial(env_lock)]
async fn strict_delete_cleanup_keeps_an_unstopped_task_and_registry_entry() {
    let _key = install_test_api_key();
    let (state, _buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;
    *slot.run_task.lock() = Some(tokio::spawn(std::future::pending::<()>()));
    let error =
        super::super::cleanup_session_slot(&state, &slot, super::super::SlotCleanup::Delete)
            .await
            .expect_err("delete must not continue while a writer is unconfirmed");
    assert!(matches!(error, crate::AppError::Config(ref code) if code == "stop_timeout"));
    assert!(slot.run_task.lock().is_some());
    assert!(state.registry.get(&slot.session_id).is_some());
    assert!(slot
        .ctx
        .session_runtime
        .session
        .transcript_path(&slot.session_id)
        .exists());
    super::super::cleanup_session_slot(&state, &slot, super::super::SlotCleanup::Close)
        .await
        .unwrap();
    assert!(slot.run_task.lock().is_none());
    assert!(state.registry.get(&slot.session_id).is_none());
}

#[tokio::test]
#[serial(env_lock)]
async fn session_delete_rejects_foreign_scope_and_other_live_manager() {
    let _key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;
    let directory = slot
        .ctx
        .session_runtime
        .session
        .sessions_dir()
        .to_path_buf();
    let foreign = crate::SessionManager::new_scoped(directory.clone(), "foreign");
    let foreign_entry = foreign.new_current_session(None).unwrap();
    handle_command(
        Arc::clone(&state),
        ServeCommand::DeleteSession {
            id: Some("foreign-delete".into()),
            session_id: foreign_entry.session_id.clone(),
        },
    )
    .await
    .unwrap();
    let frames = wait_for_line(&buffer, |v| v["id"] == "foreign-delete").await;
    assert!(frames
        .iter()
        .any(|v| v["id"] == "foreign-delete" && v["error"] == "session_scope_mismatch"));
    assert!(foreign.transcript_path(&foreign_entry.session_id).exists());
    let other = crate::SessionManager::new_scoped(
        directory,
        slot.ctx.session_runtime.session.current_session_key(),
    );
    other.pin_session(&slot.session_id).unwrap();
    handle_command(
        Arc::clone(&state),
        ServeCommand::DeleteSession {
            id: Some("occupied-delete".into()),
            session_id: slot.session_id.clone(),
        },
    )
    .await
    .unwrap();
    let frames = wait_for_line(&buffer, |v| v["id"] == "occupied-delete").await;
    assert!(frames
        .iter()
        .any(|v| v["id"] == "occupied-delete" && v["error"] == "session_in_use"));
    assert!(other.transcript_path(&slot.session_id).exists());
}
