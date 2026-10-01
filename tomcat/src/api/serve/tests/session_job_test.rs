use super::super::session_job::spawn_session_job;
use super::*;
use serial_test::serial;
use std::sync::Arc;
use std::time::Duration;

async fn response(buffer: &SharedWriterBuffer, id: &str) -> serde_json::Value {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Some(frame) = read_ndjson_lines(buffer)
                .into_iter()
                .find(|frame| frame["id"] == id)
            {
                return frame;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("response must arrive")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(env_lock)]
async fn session_job_releases_fifo_blocks_mutation_and_is_idle_before_response() {
    let _home_lock = crate::test_support::home_env_lock().lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let _home = EnvGuard::set("HOME", home.path().as_os_str());
    let _key = EnvGuard::set(TEST_API_KEY_ENV, "stub");
    let (state, buffer, _work, a) = build_initialized_state_with_streams(vec![]).await;
    let workspace_b = tempfile::tempdir().unwrap();
    let b = create_session_slot(
        Arc::clone(&state),
        NewSessionParams {
            cwd: Some(workspace_b.path().to_string_lossy().into_owned()),
            ..Default::default()
        },
        false,
    )
    .await
    .unwrap();
    state.registry.insert(Arc::clone(&b)).unwrap();
    let active = state.registry.active_session_id();
    let before = a
        .ctx
        .session_runtime
        .session
        .get_entries(256)
        .unwrap()
        .len();
    let (release, wait) = tokio::sync::oneshot::channel::<()>();
    spawn_session_job(
        Arc::clone(&state),
        Arc::clone(&a),
        Some("job".into()),
        async move {
            wait.await.map_err(|error| error.to_string())?;
            Ok(serde_json::json!({"done":true}))
        },
    )
    .unwrap();
    assert!(a.is_busy() && a.is_command_job_running());
    assert_eq!(state.registry.active_session_id(), active);
    tokio::time::timeout(
        Duration::from_millis(500),
        handle_command(
            Arc::clone(&state),
            ServeCommand::GetState {
                id: Some("b-state".into()),
                session_id: Some(b.session_id.clone()),
            },
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(response(&buffer, "b-state").await["success"], true);
    handle_command(
        Arc::clone(&state),
        ServeCommand::GetState {
            id: Some("a-state".into()),
            session_id: Some(a.session_id.clone()),
        },
    )
    .await
    .unwrap();
    assert_eq!(response(&buffer, "a-state").await["payload"]["busy"], false);
    assert!(
        !state
            .registry
            .list()
            .iter()
            .find(|session| session.session_id == a.session_id)
            .unwrap()
            .busy
    );
    for (id, command) in [
        (
            "prompt",
            ServeCommand::Prompt {
                id: Some("prompt".into()),
                session_id: Some(a.session_id.clone()),
                text: "must not append".into(),
                params: Default::default(),
            },
        ),
        (
            "follow",
            ServeCommand::FollowUp {
                id: Some("follow".into()),
                session_id: Some(a.session_id.clone()),
                text: "must not queue".into(),
                params: Default::default(),
            },
        ),
        (
            "steer",
            ServeCommand::Steer {
                id: Some("steer".into()),
                session_id: Some(a.session_id.clone()),
                text: "must not queue".into(),
                params: Default::default(),
            },
        ),
    ] {
        handle_command(Arc::clone(&state), command).await.unwrap();
        assert_eq!(response(&buffer, id).await["error"], "busy");
    }
    assert_eq!(
        a.ctx
            .session_runtime
            .session
            .get_entries(256)
            .unwrap()
            .len(),
        before
    );
    assert!(a.ctx.session_runtime.follow_up_queue.lock().is_empty());
    assert!(a.ctx.session_runtime.steering_queue.lock().is_empty());
    assert!(!read_ndjson_lines(&buffer)
        .iter()
        .any(|frame| frame["type"] == "agent_start" || frame["type"] == "agent_idle"));
    release.send(()).unwrap();
    assert_eq!(response(&buffer, "job").await["success"], true);
    assert!(!a.is_busy() && !a.is_command_job_running());
    assert!(a.run_task.lock().is_none());
    cleanup_session_slot(&state, &a, true, "test_finished")
        .await
        .unwrap();
    cleanup_session_slot(&state, &b, true, "test_finished")
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(env_lock)]
async fn session_job_panic_and_immediate_completion_clear_handle_and_busy_gate() {
    let _home_lock = crate::test_support::home_env_lock().lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let _home = EnvGuard::set("HOME", home.path().as_os_str());
    let _key = EnvGuard::set(TEST_API_KEY_ENV, "stub");
    let (state, buffer, _work, slot) = build_initialized_state_with_streams(vec![]).await;
    spawn_session_job(
        Arc::clone(&state),
        Arc::clone(&slot),
        Some("panic".into()),
        async {
            panic!("intentional command panic");
            #[allow(unreachable_code)]
            Ok(serde_json::Value::Null)
        },
    )
    .unwrap();
    assert_eq!(
        response(&buffer, "panic").await["error"],
        "command task panicked"
    );
    assert!(!slot.is_busy());
    assert!(slot.run_task.lock().is_none());
    for index in 0..20 {
        let id = format!("fast-{index}");
        spawn_session_job(
            Arc::clone(&state),
            Arc::clone(&slot),
            Some(id.clone()),
            async { Ok(serde_json::Value::Null) },
        )
        .unwrap();
        assert_eq!(response(&buffer, &id).await["success"], true);
        assert!(!slot.is_busy());
        assert!(
            slot.run_task.lock().is_none(),
            "fast completion must not leave a finished handle"
        );
    }
    assert!(slot.mark_busy(), "next turn can enter after the response");
    slot.mark_idle();
    cleanup_session_slot(&state, &slot, true, "test_finished")
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(env_lock)]
async fn session_job_close_aborts_at_existing_shutdown_timeout() {
    let _home_lock = crate::test_support::home_env_lock().lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let _home = EnvGuard::set("HOME", home.path().as_os_str());
    let _key = EnvGuard::set(TEST_API_KEY_ENV, "stub");
    let (state, buffer, _work, slot) = build_initialized_state_with_streams(vec![]).await;
    spawn_session_job(
        Arc::clone(&state),
        Arc::clone(&slot),
        Some("never".into()),
        std::future::pending(),
    )
    .unwrap();
    tokio::time::timeout(
        Duration::from_secs(5),
        handle_command(
            Arc::clone(&state),
            ServeCommand::CloseSession {
                id: Some("close".into()),
                session_id: Some(slot.session_id.clone()),
            },
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(response(&buffer, "close").await["success"], true);
    assert!(state.registry.get(&slot.session_id).is_none());
    assert!(slot.run_task.lock().is_none());
}
