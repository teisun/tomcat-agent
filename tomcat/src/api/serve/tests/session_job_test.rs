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

#[tokio::test]
#[serial(env_lock)]
async fn start_turn_writer_failure_releases_admission_without_starting_work() {
    let _key = install_test_api_key();
    let (state, _buffer, _work, slot) = build_initialized_state_with_streams(vec![]).await;
    // A failed physical writer sets this same closed signal before the next send.
    state.writer.close();
    let result = super::super::commands::start_turn(
        state.clone(),
        slot.clone(),
        Some("closed-writer".into()),
        None,
        super::super::commands::TurnAck::Accepted,
    )
    .await;
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("connection_closed"));
    assert!(
        slot.run_task.lock().is_none(),
        "no turn may start after a failed acknowledgement"
    );
    assert!(
        !slot.is_busy(),
        "a failed start must release its busy admission"
    );
    assert!(slot.ctx.session_runtime.cancel_token.lock().is_cancelled());
    assert!(!slot.ctx.session_runtime.steering_queue.lock().is_open());
    assert!(slot.mark_busy(), "a later controller must be able to enter");
    slot.mark_idle();
    cleanup_session_slot(&state, &slot, super::super::SlotCleanup::Close)
        .await
        .unwrap();
}

#[tokio::test]
#[serial(env_lock)]
async fn session_stop_catches_a_shell_registered_after_the_first_scan() {
    let _key = install_test_api_key();
    let (state, _buffer, work, slot) = build_initialized_state_with_streams(vec![]).await;
    let (release, pending) = tokio::sync::oneshot::channel::<()>();
    let (registered, ticket) = tokio::sync::oneshot::channel();
    let worker_slot = slot.clone();
    let cwd = work.path().to_owned();
    assert!(slot.mark_busy());
    *slot.run_task.lock() = Some(tokio::spawn(async move {
        pending.await.unwrap();
        let task = worker_slot
            .ctx
            .session_runtime
            .bash_task_registry
            .spawn("sleep 30".into(), Some(cwd))
            .await
            .unwrap();
        registered.send(task).unwrap();
        worker_slot.mark_idle();
    }));
    let stopping = state.stop_slot_work(&slot);
    tokio::pin!(stopping);
    // The first poll passes the empty Bash scan and blocks on the controlled turn.
    assert!(futures_util::poll!(&mut stopping).is_pending());
    assert!(slot
        .ctx
        .session_runtime
        .bash_task_registry
        .list()
        .is_empty());
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), &mut stopping)
        .await
        .unwrap()
        .unwrap();
    let ticket = ticket.await.unwrap();
    let bash = &slot.ctx.session_runtime.bash_task_registry;
    let stopped = bash
        .list()
        .into_iter()
        .find(|task| task.task_id == ticket.task_id)
        .unwrap();
    // Always clean up the real process, including on the pre-fix failure path.
    bash.stop(&ticket.task_id).await.unwrap();
    bash.wait_for_finish(&ticket.task_id).await.unwrap();
    assert!(
        matches!(
            stopped.status,
            crate::core::tools::primitive::BashTaskStatus::Stopped
        ),
        "successful session stop left a live late registration: {stopped:?}"
    );
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
    let active = state.active_session_id();
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
    assert_eq!(state.active_session_id(), active);
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
    cleanup_session_slot(&state, &a, super::super::SlotCleanup::Close)
        .await
        .unwrap();
    cleanup_session_slot(&state, &b, super::super::SlotCleanup::Close)
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
    cleanup_session_slot(&state, &slot, super::super::SlotCleanup::Close)
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(env_lock)]
async fn session_job_close_retains_unfinished_work_and_can_be_retried() {
    let _home_lock = crate::test_support::home_env_lock().lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let _home = EnvGuard::set("HOME", home.path().as_os_str());
    let _key = EnvGuard::set(TEST_API_KEY_ENV, "stub");
    let (state, buffer, _work, slot) = build_initialized_state_with_streams(vec![]).await;
    let (release, pending) = tokio::sync::oneshot::channel::<()>();
    spawn_session_job(
        Arc::clone(&state),
        Arc::clone(&slot),
        Some("never".into()),
        async move {
            pending.await.unwrap();
            Ok(serde_json::Value::Null)
        },
    )
    .unwrap();
    tokio::time::timeout(
        Duration::from_secs(5),
        super::super::stdin::dispatch_command(
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
    let rejected = response(&buffer, "close").await;
    assert_eq!(rejected["success"], false, "{rejected}");
    assert!(rejected["error"].as_str().unwrap().contains("stop_timeout"));
    assert!(state.registry.get(&slot.session_id).is_some());
    assert!(slot.is_busy() && slot.run_task.lock().is_some());
    release.send(()).unwrap();
    assert_eq!(response(&buffer, "never").await["success"], true);
    handle_command(
        state.clone(),
        ServeCommand::CloseSession {
            id: Some("retry-close".into()),
            session_id: Some(slot.session_id.clone()),
        },
    )
    .await
    .unwrap();
    assert_eq!(response(&buffer, "retry-close").await["success"], true);
    assert!(state.registry.get(&slot.session_id).is_none());
    assert!(slot.run_task.lock().is_none());
}
