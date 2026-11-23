use super::*;
use serial_test::serial;
use std::sync::atomic::Ordering;
use std::sync::Arc;

#[tokio::test]
#[serial(env_lock)]
async fn shared_initialize_reports_attachment_root_before_workspace_scope() {
    let _key = install_test_api_key();
    let (runtime, work) = shared_runtime(2);
    let (connection, output) = connect(&runtime);
    assert!(connection.current_scope().is_none());
    handle_control_or_interrupt(
        connection.clone(),
        ServeCommand::ControlRequest {
            request_id: "initialize-root".into(),
            subtype: "initialize".into(),
            session_id: None,
            payload: serde_json::Value::Null,
        },
    )
    .await
    .unwrap();
    let reply = frame(&output, |f| f["requestId"] == "initialize-root").await;
    let root = reply["payload"]["attachmentRoot"]
        .as_str()
        .expect("attachment root before workspace scope");
    assert!(connection.current_scope().is_none());
    new_shared(&connection, work.path(), "new").await;
    let slot = runtime
        .registry
        .get(&connection.active_session_id().unwrap())
        .unwrap();
    assert_eq!(
        std::path::Path::new(root),
        slot.ctx.session_runtime.session.attachment_store().root()
    );
    runtime.stop_all().await.unwrap();
}

#[tokio::test]
#[serial(env_lock)]
async fn disk_sessions_require_workspace_scope_before_bootstrap() {
    let _key = install_test_api_key();
    let (runtime, work) = shared_runtime(2);
    let (connection, output) = connect(&runtime);
    super::super::stdin::dispatch_command(
        connection.clone(),
        ServeCommand::ListSessions {
            id: Some("without-scope".into()),
            scope: Some(ListSessionsScope::Disk),
        },
    )
    .await
    .unwrap();
    let reply = frame(&output, |f| f["id"] == "without-scope").await;
    assert_eq!(reply["success"], false);
    assert_eq!(reply["error"], "scope_requires_cwd");
    assert!(connection.current_scope().is_none());
    assert_eq!(runtime.registry.len(), 0);

    new_shared(&connection, work.path(), "new").await;
    handle_command(
        connection.clone(),
        ServeCommand::ListSessions {
            id: Some("with-scope".into()),
            scope: Some(ListSessionsScope::Disk),
        },
    )
    .await
    .unwrap();
    let reply = frame(&output, |f| f["id"] == "with-scope").await;
    assert_eq!(reply["success"], true);
    assert!(connection.current_scope().is_some());
    assert_eq!(reply["payload"]["sessions"].as_array().unwrap().len(), 1);
    runtime.stop_all().await.unwrap();
}

#[tokio::test]
#[serial(env_lock)]
async fn session_bootstrap_reuses_current_without_creating_empty_sessions() {
    let _key = install_test_api_key();
    let (runtime, work) = shared_runtime(4);
    let (first, _) = connect(&runtime);
    let bootstrap = || ServeCommand::NewSession {
        id: Some("bootstrap".into()),
        params: NewSessionParams {
            cwd: Some(work.path().to_string_lossy().into_owned()),
            resume_current: true,
            ..Default::default()
        },
    };
    handle_command(first.clone(), bootstrap()).await.unwrap();
    let id = first.active_session_id().unwrap();
    let slot = runtime.registry.get(&id).unwrap();
    let before = std::fs::read(slot.ctx.session_runtime.session.transcript_path(&id)).unwrap();
    first.disconnect().await.unwrap();
    let (second, output) = connect(&runtime);
    handle_command(second.clone(), bootstrap()).await.unwrap();
    assert_eq!(second.active_session_id().as_deref(), Some(id.as_str()));
    assert_eq!(
        frame(&output, |f| f["id"] == "bootstrap").await["payload"]["access"],
        "control"
    );
    assert_eq!(runtime.registry.len(), 1);
    assert_eq!(
        slot.ctx.session_runtime.session.list_session_ids().unwrap(),
        vec![id.clone()]
    );
    assert_eq!(
        std::fs::read(slot.ctx.session_runtime.session.transcript_path(&id)).unwrap(),
        before
    );
    // The ordinary New Session button must still create a distinct conversation.
    new_shared(&second, work.path(), "new").await;
    assert_ne!(second.active_session_id().unwrap(), id);
    runtime.stop_all().await.unwrap();
}

#[tokio::test]
#[serial(env_lock)]
async fn shared_native_delete_is_idempotent_after_its_last_session_is_gone() {
    let _key = install_test_api_key();
    let (runtime, work) = shared_runtime(2);
    let (connection, output) = connect(&runtime);
    new_shared(&connection, work.path(), "new").await;
    let id = connection.active_session_id().unwrap();
    let transcript = runtime
        .registry
        .get(&id)
        .unwrap()
        .ctx
        .session_runtime
        .session
        .transcript_path(&id);
    for request_id in ["delete", "delete-again"] {
        super::super::stdin::dispatch_command(
            connection.clone(),
            ServeCommand::DeleteSession {
                id: Some(request_id.into()),
                session_id: id.clone(),
            },
        )
        .await
        .unwrap();
        let reply = frame(&output, |f| f["id"] == request_id).await;
        assert_eq!(reply["success"], true, "{reply}");
        assert_eq!(reply["payload"]["warnings"], serde_json::json!([]));
        assert!(!transcript.exists());
        assert_eq!(runtime.registry.len(), 0);
    }
    runtime.stop_all().await.unwrap();
}

#[tokio::test]
#[serial(env_lock)]
async fn session_bootstrap_opens_current_host_readonly_and_keeps_project_scope() {
    use serde_json::json;
    let _key = install_test_api_key();
    let (runtime, work) = shared_runtime(4);
    let (writer, _) = spawn_buffered_writer(&runtime.cfg.serve);
    let host = ServeState::connect_acp(runtime.clone(), writer);
    host.initialized.store(true, Ordering::SeqCst);
    new_shared(&host, work.path(), "host-new").await;
    let id = host.active_session_id().unwrap();
    let slot = runtime.registry.get(&id).unwrap();
    let before = std::fs::read(slot.ctx.session_runtime.session.transcript_path(&id)).unwrap();
    emit_replay_message(&slot, "boot-prefix", "message_start", json!(null));
    emit_replay_message(
        &slot,
        "boot-prefix",
        "message_update",
        json!({"kind":"content_delta","delta":"already generating"}),
    );
    let (view, output) = connect(&runtime);
    handle_command(
        view.clone(),
        ServeCommand::NewSession {
            id: Some("bootstrap".into()),
            params: NewSessionParams {
                cwd: Some(work.path().to_string_lossy().into_owned()),
                resume_current: true,
                ..Default::default()
            },
        },
    )
    .await
    .unwrap();
    assert_eq!(
        frame(&output, |f| f["id"] == "bootstrap").await["payload"]["access"],
        "observe"
    );
    assert_eq!(
        frame(&output, |f| f["assistantMessageId"] == "boot-prefix").await["message"]["content"],
        "already generating"
    );
    assert_eq!(runtime.registry.len(), 1);
    assert_eq!(*slot.controller.lock(), Some(host.connection_id));
    assert_eq!(
        std::fs::read(slot.ctx.session_runtime.session.transcript_path(&id)).unwrap(),
        before
    );
    assert_eq!(
        view.current_scope().unwrap().key,
        slot.ctx.session_runtime.session.current_session_key()
    );
    view.disconnect().await.unwrap();
    assert!(!slot.is_interrupted());
    let other = tempfile::tempdir().unwrap();
    let (foreign, foreign_output) = connect(&runtime);
    handle_command(
        foreign.clone(),
        ServeCommand::NewSession {
            id: Some("foreign-bootstrap".into()),
            params: NewSessionParams {
                cwd: Some(other.path().to_string_lossy().into_owned()),
                resume_current: true,
                ..Default::default()
            },
        },
    )
    .await
    .unwrap();
    handle_command(
        foreign,
        ServeCommand::SwitchSession {
            id: Some("foreign-open".into()),
            session_id: id,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        frame(&foreign_output, |f| f["id"] == "foreign-open").await["error"],
        "unknown_session"
    );
    runtime.stop_all().await.unwrap();
}

#[tokio::test]
#[serial(env_lock)]
async fn session_bootstrap_validates_before_changing_scope() {
    let _key = install_test_api_key();
    let (runtime, work) = shared_runtime(2);
    let (view, _) = connect(&runtime);
    for (cwd, detached) in [
        (Some("relative".into()), false),
        (None, false),
        (Some(work.path().to_string_lossy().into_owned()), true),
    ] {
        let result = handle_command(
            view.clone(),
            ServeCommand::NewSession {
                id: None,
                params: NewSessionParams {
                    cwd,
                    detached,
                    resume_current: true,
                    ..Default::default()
                },
            },
        )
        .await;
        assert!(result.is_err());
        assert!(view.current_scope().is_none());
        assert_eq!(runtime.registry.len(), 0);
    }
    runtime.stop_all().await.unwrap();
}

fn emit_replay_message(
    slot: &registry::SessionSlot,
    id: &str,
    kind: &str,
    delta: serde_json::Value,
) {
    let mut payload = serde_json::json!({
        "type":kind, "sessionId":slot.session_id, "assistantMessageId":id, "message":{}
    });
    if kind == "message_update" {
        payload["assistantMessageEvent"] = delta;
    }
    slot.ctx
        .global_services
        .event_bus
        .emit_sync(
            kind,
            crate::infra::event_bus::EventContext::new(kind, payload)
                .with_session_id(slot.session_id.clone()),
        )
        .unwrap();
}

#[tokio::test]
#[serial(env_lock)]
async fn observer_replay_keeps_one_message_through_end_reconnect_and_project_checks() {
    use serde_json::json;
    let _key = install_test_api_key();
    let (runtime, work) = shared_runtime(4);
    let (owner, _) = connect(&runtime);
    new_shared(&owner, work.path(), "new").await;
    let id = owner.active_session_id().unwrap();
    let slot = runtime.registry.get(&id).unwrap();
    let (view, output) = connect_in_project(&runtime, work.path()).await;
    let foreign_dir = work.path().join("foreign");
    std::fs::create_dir_all(&foreign_dir).unwrap();
    let (foreign, foreign_output) = connect_in_project(&runtime, &foreign_dir).await;
    emit_replay_message(&slot, "m1", "message_start", json!(null));
    emit_replay_message(
        &slot,
        "m1",
        "message_update",
        json!({"kind":"thinking_delta","delta":"先检查 "}),
    );
    emit_replay_message(
        &slot,
        "m1",
        "message_update",
        json!({"kind":"content_delta","delta":"你好，\n"}),
    );
    flush(&view, &output).await;
    assert!(!read_ndjson_lines(&output)
        .iter()
        .any(|f| f["assistantMessageId"] == "m1"));
    handle_command(
        view.clone(),
        ServeCommand::SwitchSession {
            id: Some("watch".into()),
            session_id: id.clone(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        frame(&output, |f| f["id"] == "watch").await["payload"]["access"],
        "observe"
    );
    let prefix = frame(&output, |f| f["assistantMessageId"] == "m1").await;
    assert_eq!(prefix["message"]["content"], "你好，\n");
    assert_eq!(prefix["message"]["thinking_text"], "先检查 ");
    assert!(prefix["assistantMessageEvent"].is_null());
    emit_replay_message(
        &slot,
        "m1",
        "message_update",
        json!({"kind":"content_delta","delta":"世界 👋"}),
    );
    emit_replay_message(
        &slot,
        "m1",
        "message_update",
        json!({"kind":"thinking_delta","delta":"再验证"}),
    );
    emit_replay_message(&slot, "m1", "message_end", json!(null));
    view.clear_displayed_session(&id);
    view.set_active_session(&id).unwrap();
    flush(&view, &output).await;
    let frames = read_ndjson_lines(&output);
    let snapshots: Vec<_> = frames
        .iter()
        .filter(|f| {
            f["type"] == "message_update"
                && f["assistantMessageId"] == "m1"
                && f["assistantMessageEvent"].is_null()
        })
        .collect();
    assert_eq!(snapshots.len(), 2);
    assert_eq!(snapshots[1]["message"]["content"], "你好，\n世界 👋");
    assert_eq!(snapshots[1]["message"]["thinking_text"], "先检查 再验证");
    assert_eq!(
        frames
            .iter()
            .filter(|f| f["type"] == "message_end" && f["assistantMessageId"] == "m1")
            .count(),
        2
    );
    assert_eq!(*slot.controller.lock(), Some(owner.connection_id));
    let (reconnected, reconnect_output) = connect_in_project(&runtime, work.path()).await;
    reconnected.set_active_session(&id).unwrap();
    assert_eq!(
        frame(&reconnect_output, |f| f["assistantMessageId"] == "m1").await["message"]["content"],
        "你好，\n世界 👋"
    );
    reconnected.disconnect().await.unwrap();
    assert!(!slot.is_interrupted());
    assert!(foreign.set_active_session(&id).is_err());
    view.clear_displayed_session(&id);
    emit_replay_message(&slot, "m2", "message_start", json!(null));
    view.set_active_session(&id).unwrap();
    let empty = frame(&output, |f| f["assistantMessageId"] == "m2").await;
    assert_eq!(empty["message"]["content"], "");
    assert_eq!(empty["message"]["thinking_text"], "");
    runtime.emit_event(
        &id,
        json!({"type":"agent_end","sessionId":id,"messages":[],"error":"interrupted"}),
    );
    view.clear_displayed_session(&id);
    view.set_active_session(&id).unwrap();
    frame(&output, |f| {
        f["type"] == "message_end" && f["assistantMessageId"] == "m2"
    })
    .await;
    flush(&foreign, &foreign_output).await;
    assert!(!read_ndjson_lines(&foreign_output)
        .iter()
        .any(|f| f["assistantMessageId"].is_string()));
    // The shared history rewrite boundary also discards a stale replay.
    view.clear_displayed_session(&id);
    flush(&view, &output).await;
    let before = read_ndjson_lines(&output)
        .iter()
        .filter(|f| f["type"] == "message_update")
        .count();
    super::super::commands::rehydrate_slot_context_state(&slot).unwrap();
    view.set_active_session(&id).unwrap();
    flush(&view, &output).await;
    assert_eq!(
        read_ndjson_lines(&output)
            .iter()
            .filter(|f| f["type"] == "message_update")
            .count(),
        before
    );
    runtime.stop_all().await.unwrap();
}

#[tokio::test]
#[serial(env_lock)]
async fn observer_replay_and_concurrent_delta_have_one_atomic_handoff() {
    use serde_json::json;
    let _key = install_test_api_key();
    let (runtime, work) = shared_runtime(2);
    let (owner, _) = connect(&runtime);
    new_shared(&owner, work.path(), "new").await;
    let id = owner.active_session_id().unwrap();
    let slot = runtime.registry.get(&id).unwrap();
    let (view, output) = connect_in_project(&runtime, work.path()).await;
    for i in 0..24 {
        let message_id = format!("race-{i}");
        view.clear_displayed_session(&id);
        emit_replay_message(&slot, &message_id, "message_start", json!(null));
        emit_replay_message(
            &slot,
            &message_id,
            "message_update",
            json!({"kind":"content_delta","delta":"a"}),
        );
        let barrier = std::sync::Barrier::new(2);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                barrier.wait();
                view.set_active_session(&id).unwrap();
            });
            scope.spawn(|| {
                barrier.wait();
                emit_replay_message(
                    &slot,
                    &message_id,
                    "message_update",
                    json!({"kind":"content_delta","delta":"b"}),
                );
            });
        });
        emit_replay_message(&slot, &message_id, "message_end", json!(null));
        flush(&view, &output).await;
        let events: Vec<_> = read_ndjson_lines(&output)
            .into_iter()
            .filter(|f| f["assistantMessageId"] == message_id)
            .collect();
        assert!(
            events[0]["assistantMessageEvent"].is_null(),
            "prefix must precede live deltas: {events:?}"
        );
        let mut text = events[0]["message"]["content"].as_str().unwrap().to_owned();
        for event in &events[1..] {
            if let Some(delta) = event["assistantMessageEvent"]["delta"].as_str() {
                text.push_str(delta);
            }
        }
        assert_eq!(text, "ab", "{events:?}");
        assert_eq!(events.last().unwrap()["type"], "message_end");
    }
    runtime.stop_all().await.unwrap();
}

#[test]
fn internal_external_load_is_not_part_of_the_native_wire_protocol() {
    let frame = serde_json::json!({
        "type":"load_external_session", "id":"load", "sessionId":"s", "cwd":"/project"
    });
    assert!(serde_json::from_value::<ServeCommand>(frame).is_err());
    let schema = serde_json::to_string(&schemars::schema_for!(ServeCommand)).unwrap();
    assert!(!schema.contains("load_external_session"));
}

#[tokio::test]
#[serial(env_lock)]
async fn external_load_checks_cwd_before_acquisition_and_needs_no_placeholder_session() {
    let _key = install_test_api_key();
    let (runtime, work) = shared_runtime(3);
    let repo = work.path().join("repo");
    let cwd = repo.join("a");
    let wrong = repo.join("b");
    for dir in [&cwd, &wrong, &repo.join(".git")] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let host = connect_acp(&runtime);
    new_shared(&host, &cwd, "original").await;
    let id = host.active_session_id().unwrap();
    let original = runtime.registry.get(&id).unwrap();
    let manager = original.ctx.session_runtime.session.clone();
    let history = manager.transcript_path(&id);
    let bytes = std::fs::read(&history).unwrap();
    let sessions = manager.list_session_ids().unwrap();
    host.disconnect().await.unwrap();
    cleanup_session_slot(&host, &original, SlotCleanup::Close)
        .await
        .unwrap();
    assert_eq!(runtime.registry.len(), 0);

    let (writer, output) = spawn_buffered_writer(&runtime.cfg.serve);
    let next = ServeState::connect_acp(runtime.clone(), writer);
    next.initialized.store(true, Ordering::SeqCst);
    let load = |dir: &std::path::Path| ServeCommand::LoadExternalSession {
        id: Some("load".into()),
        session_id: id.clone(),
        cwd: dir.to_string_lossy().into_owned(),
    };
    // A different cwd must not load history, even within the same git repository.
    handle_command(next.clone(), load(&wrong)).await.unwrap();
    assert_eq!(
        frame(&output, |f| f["id"] == "load").await["error"],
        "unknown_session"
    );
    assert!(next.active_session_id().is_none());
    assert!(next.current_scope().is_none());
    assert_eq!(runtime.registry.len(), 0);
    assert_eq!(std::fs::read(&history).unwrap(), bytes);

    handle_command(next.clone(), load(&cwd)).await.unwrap();
    assert_eq!(next.active_session_id(), Some(id.clone()));
    let loaded = runtime.registry.get(&id).unwrap();
    assert_eq!(*loaded.controller.lock(), Some(next.connection_id));
    assert_eq!(manager.list_session_ids().unwrap(), sessions);
    assert_eq!(std::fs::read(&history).unwrap(), bytes);
    // The same checks apply to an already-live session, including its current controller.
    let error = handle_command(next.clone(), load(&wrong))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("session_cwd_mismatch"));
    assert_eq!(next.active_session_id(), Some(id.clone()));
    let rival = connect_acp(&runtime);
    loaded.mark_busy();
    let error = handle_command(rival.clone(), load(&cwd)).await.unwrap_err();
    assert!(error.to_string().contains("session_in_use"));
    assert_eq!(*loaded.controller.lock(), Some(next.connection_id));
    loaded.mark_idle();
    handle_command(rival.clone(), load(&cwd)).await.unwrap();
    assert_eq!(*loaded.controller.lock(), Some(rival.connection_id));
    handle_command(
        next.clone(),
        ServeCommand::Prompt {
            id: Some("lost-control".into()),
            session_id: Some(id.clone()),
            text: "must reload before prompting".into(),
            params: Default::default(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        frame(&output, |f| f["id"] == "lost-control").await["error"],
        "read_only_session"
    );

    // Even Rust-side construction cannot grant a native connection the private ACP load path.
    let (native, output) = connect(&runtime);
    handle_command(native.clone(), load(&cwd)).await.unwrap();
    assert_eq!(
        frame(&output, |f| f["id"] == "load").await["error"],
        "read_only_session"
    );
    assert!(native.active_session_id().is_none());
    assert_eq!(std::fs::read(&history).unwrap(), bytes);
    runtime.stop_all().await.unwrap();
}

#[tokio::test]
#[serial(env_lock)]
async fn shared_registry_does_not_share_selection_initialization_or_response_ids() {
    let _key = install_test_api_key();
    let temp = tempfile::tempdir().unwrap();
    let cfg = serve_test_config(temp.path(), "http://127.0.0.1:1");
    let config = write_test_config(&cfg);
    let runtime = runtime::ServeRuntime::new(
        cfg.clone(),
        config,
        build_shared_model_prefs(&cfg).unwrap(),
        true,
    )
    .unwrap();
    let (writer_a, buffer_a) = spawn_buffered_writer(&cfg.serve);
    let (writer_b, buffer_b) = spawn_buffered_writer(&cfg.serve);
    let a = ServeState::connect(runtime.clone(), writer_a);
    let b = ServeState::connect(runtime.clone(), writer_b);
    assert!(Arc::ptr_eq(&a.registry, &b.registry));
    a.initialized.store(true, Ordering::SeqCst);
    assert!(!b.initialized.load(Ordering::SeqCst));
    let workspace_a = tempfile::tempdir().unwrap();
    let workspace_b = tempfile::tempdir().unwrap();
    for (state, cwd) in [(&a, workspace_a.path()), (&b, workspace_b.path())] {
        state.initialized.store(true, Ordering::SeqCst);
        handle_command(
            state.clone(),
            ServeCommand::NewSession {
                id: Some("same-request-id".into()),
                params: NewSessionParams {
                    cwd: Some(cwd.to_string_lossy().into()),
                    ..Default::default()
                },
            },
        )
        .await
        .unwrap();
    }
    let a_id = a.active_session_id().unwrap();
    let b_id = b.active_session_id().unwrap();
    assert_ne!(a_id, b_id);
    assert_eq!(a.resolve_session_id(None).unwrap(), a_id);
    assert_eq!(b.resolve_session_id(None).unwrap(), b_id);
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            if !read_ndjson_lines(&buffer_a).is_empty() && !read_ndjson_lines(&buffer_b).is_empty()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    for (buffer, id) in [(&buffer_a, &a_id), (&buffer_b, &b_id)] {
        let replies: Vec<_> = read_ndjson_lines(buffer)
            .into_iter()
            .filter(|f| f["id"] == "same-request-id")
            .collect();
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0]["sessionId"], *id);
    }
    let a_scope = a.current_scope().unwrap().key;
    let b_scope = b.current_scope().unwrap().key;
    assert_ne!(a_scope, b_scope);
    let slot_a = a.registry.get(&a_id).unwrap();
    let slot_b = b.registry.get(&b_id).unwrap();
    cleanup_session_slot(&a, &slot_a, SlotCleanup::Close)
        .await
        .unwrap();
    assert!(a.active_session_id().is_none());
    assert_eq!(a.current_scope().unwrap().key, a_scope);
    assert_eq!(b.active_session_id(), Some(b_id));
    assert_eq!(b.current_scope().unwrap().key, b_scope);
    cleanup_session_slot(&b, &slot_b, SlotCleanup::Close)
        .await
        .unwrap();
}

fn connect_acp(runtime: &Arc<runtime::ServeRuntime>) -> Arc<ServeState> {
    let (writer, _) = spawn_buffered_writer(&runtime.cfg.serve);
    let connection = ServeState::connect_acp(runtime.clone(), writer);
    connection.set_acp_client_name(Some("paperclip".into()));
    connection.initialized.store(true, Ordering::SeqCst);
    connection
}

#[tokio::test]
#[serial(env_lock)]
async fn disk_list_reports_origin_and_write_access_without_acquiring_control() {
    let _key = install_test_api_key();
    let (runtime, work) = shared_runtime(4);
    let host = connect_acp(&runtime);
    new_shared(&host, work.path(), "host").await;
    let host_id = host.active_session_id().unwrap();
    let external = runtime.registry.get(&host_id).unwrap();
    let (viewer, output) = connect(&runtime);
    new_shared(&viewer, work.path(), "own").await;
    let own_id = viewer.active_session_id().unwrap();
    let (other, _) = connect(&runtime);
    new_shared(&other, work.path(), "other").await;
    let other_id = other.active_session_id().unwrap();
    for stage in ["live", "cold"] {
        if stage == "cold" {
            host.disconnect().await.unwrap();
            cleanup_session_slot(&host, &external, SlotCleanup::Close)
                .await
                .unwrap();
        }
        handle_command(
            viewer.clone(),
            ServeCommand::ListSessions {
                id: Some(stage.into()),
                scope: Some(ListSessionsScope::Disk),
            },
        )
        .await
        .unwrap();
        let reply = frame(&output, |f| f["id"] == stage).await;
        let rows = reply["payload"]["sessions"].as_array().unwrap();
        let row = |id: &str| rows.iter().find(|row| row["sessionId"] == id).unwrap();
        assert_eq!(
            row(&host_id)["origin"],
            serde_json::json!({"kind":"acp","clientName":"paperclip"})
        );
        assert_eq!(
            row(&host_id)["access"],
            if stage == "cold" {
                "control"
            } else {
                "observe"
            }
        );
        assert_eq!(row(&other_id)["access"], "observe");
        assert_eq!(row(&own_id)["access"], "control");
        assert_eq!(viewer.active_session_id().as_deref(), Some(own_id.as_str()));
        assert_eq!(
            *runtime.registry.get(&other_id).unwrap().controller.lock(),
            Some(other.connection_id)
        );
    }
    assert!(
        runtime.registry.get(&host_id).is_none(),
        "listing cold metadata must not load a session"
    );
    runtime.stop_all().await.unwrap();
}

#[tokio::test]
#[serial(env_lock)]
async fn acp_origin_is_display_metadata_across_disconnect_reload_and_delete() {
    let _key = install_test_api_key();
    let (runtime, work) = shared_runtime(4);
    let host = connect_acp(&runtime);
    new_shared(&host, work.path(), "external").await;
    let id = host.active_session_id().unwrap();
    let external = runtime.registry.get(&id).unwrap();
    let history = external.ctx.session_runtime.session.transcript_path(&id);
    let origin = Some(crate::core::session::SessionOrigin::Acp {
        client_name: Some("paperclip".into()),
    });
    assert_eq!(external.origin, origin);
    assert_eq!(
        crate::core::session::read_header(&history).unwrap().origin,
        origin
    );
    let original_bytes = std::fs::read(&history).unwrap();
    let (viewer, output) = connect(&runtime);
    new_shared(&viewer, work.path(), "native").await;
    let native = viewer.active_session_id().unwrap();
    let manager = SessionManager::new_scoped(
        crate::resolve_sessions_dir(&runtime.cfg).unwrap(),
        viewer.current_scope().unwrap().key,
    );
    for (request, disconnect) in [("during", false), ("after", true)] {
        if disconnect {
            host.disconnect().await.unwrap();
        }
        handle_command(
            viewer.clone(),
            ServeCommand::SwitchSession {
                id: Some(request.into()),
                session_id: id.clone(),
            },
        )
        .await
        .unwrap();
        let response = frame(&output, |f| f["id"] == request).await;
        assert_eq!(
            response["payload"]["access"],
            if disconnect { "control" } else { "observe" }
        );
        assert_eq!(
            response["payload"]["origin"],
            serde_json::json!({"kind":"acp","clientName":"paperclip"})
        );
        assert_eq!(
            manager.current_session_entry().unwrap().unwrap().session_id,
            if disconnect {
                id.clone()
            } else {
                native.clone()
            }
        );
        assert_eq!(std::fs::read(&history).unwrap(), original_bytes);
    }
    cleanup_session_slot(&host, &external, SlotCleanup::Close)
        .await
        .unwrap();
    handle_command(
        viewer.clone(),
        ServeCommand::SwitchSession {
            id: Some("cold".into()),
            session_id: id.clone(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        frame(&output, |f| f["id"] == "cold").await["payload"]["access"],
        "control"
    );
    let reloaded = runtime.registry.get(&id).unwrap();
    assert_eq!(reloaded.origin, origin);
    assert_eq!(*reloaded.controller.lock(), Some(viewer.connection_id));
    assert_eq!(
        manager.current_session_entry().unwrap().unwrap().session_id,
        id
    );
    assert_eq!(std::fs::read(&history).unwrap(), original_bytes);
    let next = connect_acp(&runtime);
    handle_command(
        next.clone(),
        ServeCommand::SwitchSession {
            id: None,
            session_id: id.clone(),
        },
    )
    .await
    .unwrap();
    assert_eq!(*reloaded.controller.lock(), Some(next.connection_id));
    let rival = connect_acp(&runtime);
    handle_command(
        rival.clone(),
        ServeCommand::SwitchSession {
            id: None,
            session_id: id.clone(),
        },
    )
    .await
    .unwrap();
    assert_eq!(*reloaded.controller.lock(), Some(rival.connection_id));
    assert_eq!(
        crate::core::session::read_header(&history).unwrap().origin,
        origin
    );
    handle_command(
        viewer.clone(),
        ServeCommand::DeleteSession {
            id: Some("occupied-delete".into()),
            session_id: id.clone(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        frame(&output, |f| f["id"] == "occupied-delete").await["error"],
        "session_in_use"
    );
    rival.disconnect().await.unwrap();
    handle_command(
        viewer.clone(),
        ServeCommand::DeleteSession {
            id: Some("unowned-delete".into()),
            session_id: id.clone(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        frame(&output, |f| f["id"] == "unowned-delete").await["success"],
        true
    );
    assert!(!history.exists());
    for slot in runtime.registry.slots() {
        cleanup_session_slot(&viewer, &slot, SlotCleanup::Close)
            .await
            .unwrap();
    }
}

#[tokio::test]
#[serial(env_lock)]
async fn concurrent_creation_is_pinned_and_acp_controls_keep_all_their_slots() {
    let _key = install_test_api_key();
    let (runtime, work) = shared_runtime(2);
    let a = connect_acp(&runtime);
    let b = connect_acp(&runtime);
    tokio::join!(
        new_shared(&a, work.path(), "a"),
        new_shared(&b, work.path(), "b")
    );
    let a_id = a.active_session_id().unwrap();
    let b_id = b.active_session_id().unwrap();
    assert_ne!(a_id, b_id);
    for (connection, id) in [(&a, &a_id), (&b, &b_id)] {
        let slot = runtime.registry.get(id).unwrap();
        assert_eq!(
            slot.ctx
                .session_runtime
                .session
                .current_session_entry()
                .unwrap()
                .unwrap()
                .session_id,
            *id
        );
        assert_eq!(*slot.controller.lock(), Some(connection.connection_id));
    }
    a.clear_displayed_session(&a_id);
    b.clear_displayed_session(&b_id);
    let (native, output) = connect(&runtime);
    new_shared(&native, work.path(), "full").await;
    assert_eq!(
        frame(&output, |f| f["id"] == "full").await["error"],
        "too_many_sessions"
    );
    a.disconnect().await.unwrap();
    new_shared(&native, work.path(), "available").await;
    assert_eq!(
        frame(&output, |f| f["id"] == "available").await["success"],
        true
    );
    assert!(runtime.registry.get(&a_id).is_none());
    assert!(runtime.registry.get(&b_id).is_some());
    for slot in runtime.registry.slots() {
        cleanup_session_slot(&native, &slot, SlotCleanup::Close)
            .await
            .unwrap();
    }
}

#[tokio::test]
#[serial(env_lock)]
async fn failed_shared_bootstrap_rolls_back_only_the_new_session() {
    let _key = install_test_api_key();
    let (runtime, work) = shared_runtime(4);
    let (connection, _) = connect(&runtime);
    new_shared(&connection, work.path(), "existing").await;
    let existing = connection.active_session_id().unwrap();
    let manager = SessionManager::new_scoped(
        crate::resolve_sessions_dir(&runtime.cfg).unwrap(),
        connection.current_scope().unwrap().key,
    );
    let before = manager.list_session_ids().unwrap();
    let config = std::fs::read(&runtime.config_path).unwrap();
    std::fs::write(&runtime.config_path, "[broken config").unwrap();
    let result = handle_command(
        connection.clone(),
        ServeCommand::NewSession {
            id: None,
            params: NewSessionParams {
                cwd: Some(work.path().to_string_lossy().into()),
                ..Default::default()
            },
        },
    )
    .await;
    std::fs::write(&runtime.config_path, config).unwrap();
    assert!(result.is_err());
    assert_eq!(runtime.registry.len(), 1);
    assert_eq!(manager.list_session_ids().unwrap(), before);
    assert_eq!(
        manager.current_session_entry().unwrap().unwrap().session_id,
        existing
    );
    assert_eq!(connection.active_session_id(), Some(existing));
    for slot in runtime.registry.slots() {
        cleanup_session_slot(&connection, &slot, SlotCleanup::Close)
            .await
            .unwrap();
    }
}

fn shared_runtime(max_sessions: usize) -> (Arc<runtime::ServeRuntime>, tempfile::TempDir) {
    let temp = tempfile::tempdir().unwrap();
    let mut cfg = serve_test_config(temp.path(), "http://127.0.0.1:1");
    cfg.serve.max_sessions = max_sessions;
    cfg.primitive.auto_confirm = true;
    let config = write_test_config(&cfg);
    let runtime = runtime::ServeRuntime::new(
        cfg.clone(),
        config,
        build_shared_model_prefs(&cfg).unwrap(),
        true,
    )
    .unwrap();
    (runtime, temp)
}

fn connect(runtime: &Arc<runtime::ServeRuntime>) -> (Arc<ServeState>, SharedWriterBuffer) {
    let (writer, buffer) = spawn_buffered_writer(&runtime.cfg.serve);
    let state = ServeState::connect_with_source(
        runtime.clone(),
        writer,
        None,
        crate::infra::platform::EnvironmentSnapshot::capture().unwrap(),
        None,
        None,
    );
    state.initialized.store(true, Ordering::SeqCst);
    (state, buffer)
}

async fn frame(
    buffer: &SharedWriterBuffer,
    predicate: impl Fn(&serde_json::Value) -> bool,
) -> serde_json::Value {
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            if let Some(frame) = read_ndjson_lines(buffer).into_iter().find(&predicate) {
                return frame;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("response must arrive")
}

async fn flush(state: &ServeState, buffer: &SharedWriterBuffer) {
    // A global marker can overtake session buffers. Fence each live session so
    // negative routing assertions actually examine all previously enqueued events.
    let mut sessions: Vec<_> = state
        .registry
        .slots()
        .into_iter()
        .map(|slot| Some(slot.session_id.clone()))
        .collect();
    sessions.push(None);
    for session in sessions {
        let id = uuid::Uuid::new_v4().to_string();
        state
            .writer
            .send(OutFrame::Response(ResponseFrame::ok(
                Some(id.clone()),
                session,
                None,
            )))
            .unwrap();
        frame(buffer, |frame| frame["id"] == id).await;
    }
}

async fn new_shared(state: &Arc<ServeState>, cwd: &std::path::Path, id: &str) {
    handle_command(
        state.clone(),
        ServeCommand::NewSession {
            id: Some(id.into()),
            params: NewSessionParams {
                cwd: Some(cwd.to_string_lossy().into()),
                ..Default::default()
            },
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
#[serial(env_lock)]
async fn acp_observer_questions_wait_replay_validate_and_have_one_winner() {
    use crate::core::plan_runtime::{AskQuestionOutcome, AskQuestionTermination};
    use serde_json::json;
    let _key = install_test_api_key();
    let (runtime, work) = shared_runtime(8);
    let host = connect_acp(&runtime);
    new_shared(&host, work.path(), "host").await;
    let id = host.active_session_id().unwrap();
    let slot = runtime.registry.get(&id).unwrap();
    let panel = slot
        .ctx
        .session_runtime
        .plan_runtime
        .ask_question_panel()
        .unwrap();
    let questions = serde_json::from_value(json!([{"id":"choice", "prompt":"Choose", "options":[
        {"id":"first", "label":"First", "recommended":true}, {"id":"second", "label":"Second"}
    ]}]))
    .unwrap();
    let waiting = tokio::spawn(async move {
        panel
            .ask(questions, AskQuestionTermination::default())
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while !runtime.ask_question.has_pending_session(&id) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(!waiting.is_finished(), "no observers must still wait");
    let (first, output) = connect_in_project(&runtime, work.path()).await;
    let switch = |label: &str| ServeCommand::SwitchSession {
        id: Some(label.into()),
        session_id: id.clone(),
    };
    handle_command(first.clone(), switch("late")).await.unwrap();
    let question = frame(&output, |value| value["subtype"] == "ask_question").await;
    let request_id = question["requestId"].as_str().unwrap().to_owned();
    first.disconnect().await.unwrap();
    assert!(!waiting.is_finished());
    let (second, second_output) = connect_in_project(&runtime, work.path()).await;
    handle_command(second.clone(), switch("reconnected"))
        .await
        .unwrap();
    let replay = frame(&second_output, |value| value["subtype"] == "ask_question").await;
    assert_eq!(replay["requestId"], request_id);
    let (third, third_output) = connect_in_project(&runtime, work.path()).await;
    handle_command(third.clone(), switch("third"))
        .await
        .unwrap();
    frame(&third_output, |value| value["subtype"] == "ask_question").await;
    let result = |option: &str| {
        json!({"outcome":"answered","answers":[{
            "questionId":"choice","optionIds":[option],"pickedRecommended":false
        }]})
    };
    let answer = |payload| ServeCommand::ControlResponse {
        request_id: request_id.clone(),
        session_id: Some(id.clone()),
        payload,
    };
    super::super::stdin::dispatch_command(
        second.clone(),
        ServeCommand::ControlCancel {
            request_id: request_id.clone(),
            session_id: Some(id.clone()),
            payload: json!({}),
        },
    )
    .await
    .unwrap();
    for payload in [
        json!({"outcome":"interrupted", "answers":[]}),
        result("invalid"),
        json!({"requestId":"wrong-request", "result":result("second")}),
        json!({"requestId":request_id, "sessionId":"wrong-session", "result":result("second")}),
    ] {
        super::super::stdin::dispatch_command(second.clone(), answer(payload))
            .await
            .unwrap();
        assert!(runtime.ask_question.has_pending_session(&id));
    }
    let answers = [
        (second.connection_id, result("second")),
        (third.connection_id, result("first")),
    ];
    let barrier = std::sync::Barrier::new(2);
    let wins = std::thread::scope(|scope| {
        let threads = answers
            .iter()
            .map(|(connection, payload)| {
                let barrier = &barrier;
                let bridge = &runtime.ask_question;
                let frame =
                    ControlFrame::response(request_id.clone(), Some(id.clone()), payload.clone());
                scope.spawn(move || {
                    barrier.wait();
                    bridge
                        .handle_control_response_from(&frame, *connection)
                        .unwrap()
                })
            })
            .collect::<Vec<_>>();
        threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(wins.iter().filter(|won| **won).count(), 1);
    let answered = tokio::time::timeout(std::time::Duration::from_secs(3), waiting)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(answered.outcome, AskQuestionOutcome::Answered);
    assert_eq!(
        answered.answers[0].option_ids,
        [if wins[0] { "second" } else { "first" }]
    );
    let loser = if wins[0] {
        &third_output
    } else {
        &second_output
    };
    let withdrawn = frame(loser, |value| {
        value["type"] == "control_cancel" && value["requestId"] == request_id
    })
    .await;
    assert_eq!(withdrawn["payload"]["reason"], "resolved");
    assert!(!runtime.ask_question.has_pending_session(&id));
    super::super::stdin::dispatch_command(third.clone(), answer(result("first")))
        .await
        .unwrap();
    assert_eq!(*slot.controller.lock(), Some(host.connection_id));
    for connection in [second, third] {
        connection.disconnect().await.unwrap();
    }
    host.disconnect().await.unwrap();
    runtime.stop_all().await.unwrap();
}

#[tokio::test]
#[serial(env_lock)]
async fn locally_terminated_question_retires_the_observer_route() {
    use crate::core::plan_runtime::{AskQuestionOutcome, AskQuestionTermination};
    let _key = install_test_api_key();
    let (runtime, work) = shared_runtime(4);
    let host = connect_acp(&runtime);
    new_shared(&host, work.path(), "host").await;
    let id = host.active_session_id().unwrap();
    let slot = runtime.registry.get(&id).unwrap();
    let (observer, output) = connect_in_project(&runtime, work.path()).await;
    handle_command(
        observer.clone(),
        ServeCommand::SwitchSession {
            id: Some("observe".into()),
            session_id: id.clone(),
        },
    )
    .await
    .unwrap();
    let mut previous = String::new();
    for disconnected in [false, true] {
        let panel = slot
            .ctx
            .session_runtime
            .plan_runtime
            .ask_question_panel()
            .unwrap();
        let termination = AskQuestionTermination::default();
        let signal = termination.clone();
        let waiting = tokio::spawn(async move {
            panel
                .ask(
                    serde_json::from_value(
                        serde_json::json!([{"id":"q", "prompt":"Choose", "options":[
                            {"id":"a", "label":"A", "recommended":true}, {"id":"b", "label":"B"}
                        ]}]),
                    )
                    .unwrap(),
                    termination,
                )
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while !runtime.ask_question.has_pending_session(&id) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let question = frame(&output, |frame| {
            frame["subtype"] == "ask_question" && frame["requestId"] != previous
        })
        .await;
        let request = question["requestId"].as_str().unwrap().to_owned();
        previous = request.clone();
        if disconnected {
            signal.host_disconnected();
        } else {
            signal.interrupt();
        }
        let result = tokio::time::timeout(std::time::Duration::from_secs(3), waiting)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            result.outcome,
            if disconnected {
                AskQuestionOutcome::HostDisconnected
            } else {
                AskQuestionOutcome::Interrupted
            }
        );
        assert!(!runtime.ask_question.has_pending_session(&id));
        frame(&output, |frame| {
            frame["type"] == "control_cancel" && frame["requestId"] == request
        })
        .await;
    }
    observer.disconnect().await.unwrap();
    host.disconnect().await.unwrap();
    let no_controller = slot
        .ctx
        .session_runtime
        .plan_runtime
        .ask_question_panel()
        .unwrap();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        no_controller.ask(vec![], Default::default()),
    )
    .await
    .unwrap();
    assert_eq!(result.outcome, AskQuestionOutcome::HostDisconnected);
    assert!(!runtime.ask_question.has_pending_session(&id));
    runtime.stop_all().await.unwrap();
}

#[tokio::test]
#[serial(env_lock)]
async fn ask_question_panel_survives_controller_changes_without_acp_load_starting_work() {
    use serde_json::json;

    async fn assert_support(slot: &registry::SessionSlot, expected: bool) {
        assert_eq!(
            slot.ctx
                .session_runtime
                .plan_runtime
                .ask_question_panel()
                .is_some(),
            expected
        );
        let snapshot = crate::api::chat::build_prompt_snapshot(&slot.ctx, 10_000).await;
        assert_eq!(snapshot.system_text().contains("- ask_question:"), expected);
    }

    use crate::core::plan_runtime::{
        InteractionPolicy, PlanReviewPolicy, PlanStartPolicy, QuestionPolicy,
    };
    let _key = install_test_api_key();
    let (runtime, work) = shared_runtime(4);
    let launch = crate::api::acp::AcpLaunchOptions {
        plan_review: Some(PlanReviewPolicy::Auto),
        question_policy: Some(QuestionPolicy::Recommended),
        plan_start: Some(PlanStartPolicy::Auto),
        ..Default::default()
    };
    let connect_auto = || {
        let (writer, _) = spawn_buffered_writer(&runtime.cfg.serve);
        let connection = ServeState::connect_with_source(
            runtime.clone(),
            writer,
            Some(crate::core::session::SessionOrigin::Acp { client_name: None }),
            crate::infra::platform::EnvironmentSnapshot::capture().unwrap(),
            None,
            Some(launch),
        );
        connection.initialized.store(true, Ordering::SeqCst);
        connection
    };
    let host = connect_auto();
    new_shared(&host, work.path(), "host-new").await;
    let host_slot = runtime
        .registry
        .get(&host.active_session_id().unwrap())
        .unwrap();
    assert_support(&host_slot, true).await;

    let (native, _) = connect(&runtime);
    new_shared(&native, work.path(), "native-new").await;
    let id = native.active_session_id().unwrap();
    let slot = runtime.registry.get(&id).unwrap();
    assert!(slot.origin.is_none());
    assert_support(&slot, true).await;
    native.disconnect().await.unwrap();
    let load = || ServeCommand::LoadExternalSession {
        id: Some("load".into()),
        session_id: id.clone(),
        cwd: work.path().to_string_lossy().into_owned(),
    };
    handle_command(host.clone(), load()).await.unwrap();
    assert_eq!(*slot.controller.lock(), Some(host.connection_id));
    assert_support(&slot, true).await;
    assert_eq!(
        slot.ctx.session_runtime.plan_runtime.interaction_policy(),
        launch.interaction_policy()
    );

    let (next_native, output) = connect_in_project(&runtime, work.path()).await;
    for (request, takes_control) in [("observe", false), ("take-back", true)] {
        if takes_control {
            host.disconnect().await.unwrap();
        }
        handle_command(
            next_native.clone(),
            ServeCommand::SwitchSession {
                id: Some(request.into()),
                session_id: id.clone(),
            },
        )
        .await
        .unwrap();
        assert_eq!(
            frame(&output, |f| f["id"] == request).await["payload"]["access"],
            if takes_control { "control" } else { "observe" }
        );
        assert_support(&slot, true).await;
        assert_eq!(
            slot.ctx.session_runtime.plan_runtime.interaction_policy(),
            if takes_control {
                InteractionPolicy::default()
            } else {
                launch.interaction_policy()
            }
        );
    }
    next_native.disconnect().await.unwrap();
    let next_host = connect_auto();
    handle_command(next_host.clone(), load()).await.unwrap();
    assert_eq!(*slot.controller.lock(), Some(next_host.connection_id));
    assert_eq!(
        slot.ctx.session_runtime.plan_runtime.interaction_policy(),
        launch.interaction_policy()
    );
    next_host.disconnect().await.unwrap();

    // Persist a restart-interrupted question, then exercise the real cold-load path.
    let session = &slot.ctx.session_runtime.session;
    let arguments = json!({"questions":[{"id":"choice","prompt":"Which option?","options":[
        {"id":"a","label":"A","recommended":true}, {"id":"b","label":"B"}
    ]}]})
    .to_string();
    session
        .append_message(json!({"role":"user","content":"Ask before proceeding"}))
        .unwrap();
    session.append_message(json!({"role":"assistant","content":null,"tool_calls":[{
        "id":"unfinished-question","type":"function","function":{"name":"ask_question","arguments":arguments}
    }]})).unwrap();
    session
        .append_message(
            json!({"role":"tool","tool_call_id":"unfinished-question","content":"[pending]"}),
        )
        .unwrap();
    assert!(crate::api::chat::has_resumable_tail_ask_question(session).unwrap());
    let history = session.transcript_path(&id);
    let before = std::fs::read(&history).unwrap();
    cleanup_session_slot(&next_host, &slot, SlotCleanup::Close)
        .await
        .unwrap();
    assert!(runtime.registry.get(&id).is_none());

    let (writer, output) = spawn_buffered_writer(&runtime.cfg.serve);
    let cold_host = ServeState::connect_acp(runtime.clone(), writer);
    cold_host.initialized.store(true, Ordering::SeqCst);
    handle_command(cold_host.clone(), load()).await.unwrap();
    let loaded = runtime.registry.get(&id).unwrap();
    assert_eq!(*loaded.controller.lock(), Some(cold_host.connection_id));
    assert!(
        loaded.origin.is_none(),
        "capability must not depend on origin"
    );
    assert_support(&loaded, true).await;
    assert!(
        !loaded.resume_pending_ask_question.load(Ordering::SeqCst),
        "ACP load must consume the deferred resume flag without starting a turn"
    );
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    flush(&cold_host, &output).await;
    assert!(!loaded.is_busy());
    let frames = read_ndjson_lines(&output);
    assert!(
        !frames.iter().any(|f| f["subtype"] == "ask_question"
            || matches!(
                f["type"].as_str(),
                Some("agent_start" | "agent_end" | "agent_idle")
            )),
        "loading must not start a recovery turn: {frames:?}"
    );
    assert_eq!(
        std::fs::read(&history).unwrap(),
        before,
        "loading must not answer or cancel the saved question"
    );
    runtime.stop_all().await.unwrap();
}

#[tokio::test]
#[serial(env_lock)]
async fn reads_and_approvals_do_not_wait_behind_another_connections_admission() {
    use serde_json::json;
    use std::time::Duration;
    let _key = install_test_api_key();
    let (runtime, work) = shared_runtime(2);
    let (b, output) = connect(&runtime);
    new_shared(&b, work.path(), "new").await;
    let session = b.active_session_id().unwrap();
    // Stand in for another connection's slow cold-open or cleanup.
    let admission = runtime.admission.lock().await;
    for wire in [
        json!({"type":"get_state","id":"state","sessionId":session}),
        json!({"type":"get_messages","id":"history","sessionId":session}),
        json!({"type":"list_sessions","id":"list","scope":"disk"}),
    ] {
        let id = wire["id"].as_str().unwrap().to_owned();
        let command: ServeCommand = serde_json::from_value(wire).unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            handle_command(b.clone(), command).await.unwrap();
            let reply = frame(&output, |f| f["id"] == id).await;
            assert_eq!(reply["success"], true, "{reply}");
        })
        .await
        .expect("reads must bypass another connection's admission");
    }
    tokio::time::timeout(Duration::from_secs(1), async {
        handle_command(
            b.clone(),
            ServeCommand::ControlResponse {
                request_id: "not-pending".into(),
                session_id: Some(session.clone()),
                payload: json!({}),
            },
        )
        .await
        .unwrap();
        // It must reach ownership validation now, not wait for the unrelated write.
        frame(&output, |f| f["error"] == "read_only_session").await;
    })
    .await
    .expect("approval replies must bypass admission without bypassing authorization");

    let prompt = handle_command(
        b.clone(),
        ServeCommand::Prompt {
            id: Some("blocked-write".into()),
            session_id: Some(session),
            text: "admit only after the lock is released".into(),
            params: Default::default(),
        },
    );
    tokio::pin!(prompt);
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut prompt)
            .await
            .is_err()
    );
    assert!(!read_ndjson_lines(&output)
        .iter()
        .any(|f| f["id"] == "blocked-write"));
    drop(admission);
    tokio::time::timeout(Duration::from_secs(1), &mut prompt)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        frame(&output, |f| f["id"] == "blocked-write").await["success"],
        true
    );
    runtime.stop_all().await.unwrap();
}

#[tokio::test]
#[serial(env_lock)]
async fn queued_new_session_does_not_block_a_pending_approval_reply() {
    use crate::core::tools::contract::confirmation::ConfirmDecision;
    use crate::core::tools::primitive::PrimitiveOperation;
    use std::time::Duration;
    let _key = install_test_api_key();
    let (runtime, work) = shared_runtime(3);
    let (connection, output) = connect(&runtime);
    new_shared(&connection, work.path(), "initial").await;
    let id = connection.active_session_id().unwrap();
    let slot = runtime.registry.get(&id).unwrap();
    assert!(slot.mark_busy());
    let provider = runtime.confirmation.provider_for_session(id.clone());
    let mut approval = tokio::spawn(async move {
        provider
            .confirm_decision(
                PrimitiveOperation::Write,
                "pending write",
                "test",
                None,
                None,
            )
            .await
    });
    let request = frame(&output, |frame| frame["subtype"] == "confirmation").await;
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel::<ServeCommand>();
    let commands = futures_util::stream::unfold(receiver, |mut receiver| async move {
        receiver
            .recv()
            .await
            .map(|command| (Ok::<_, crate::AppError>(command), receiver))
    });
    let connection_loop = tokio::spawn(super::super::stdin::run_command_stream(
        connection.clone(),
        Box::pin(commands),
    ));
    sender
        .send(ServeCommand::NewSession {
            id: Some("queued-new".into()),
            params: NewSessionParams {
                cwd: Some(work.path().to_string_lossy().into()),
                ..Default::default()
            },
        })
        .unwrap();
    sender
        .send(ServeCommand::ControlResponse {
            request_id: request["requestId"].as_str().unwrap().into(),
            session_id: Some(id),
            payload: serde_json::json!({"decision":"allow_once"}),
        })
        .unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(3), &mut approval).await;
    slot.mark_idle();
    drop(sender);
    tokio::time::timeout(Duration::from_secs(3), connection_loop)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    runtime.stop_all().await.unwrap();
    assert!(
        matches!(outcome, Ok(Ok(Ok(ConfirmDecision::AllowOnce)))),
        "approval must finish through the real FIFO, without a new routing mechanism"
    );
    assert!(read_ndjson_lines(&output)
        .iter()
        .any(|frame| frame["id"] == "queued-new" && frame["success"] == true));
}

#[tokio::test]
#[serial(env_lock)]
async fn busy_observer_writes_and_forged_approvals_preserve_controller() {
    use crate::core::tools::contract::confirmation::ConfirmDecision;
    use crate::core::tools::primitive::PrimitiveOperation;
    let _key = install_test_api_key();
    let (runtime, work) = shared_runtime(4);
    let (a, a_buffer) = connect(&runtime);
    let (b, b_buffer) = connect_in_project(&runtime, work.path()).await;
    new_shared(&a, work.path(), "new").await;
    let id = a.active_session_id().unwrap();
    let slot = a.registry.get(&id).unwrap();
    handle_command(
        b.clone(),
        ServeCommand::SwitchSession {
            id: Some("observe".into()),
            session_id: id.clone(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        frame(&b_buffer, |f| f["id"] == "observe").await["payload"]["access"],
        "observe"
    );
    assert!(slot.mark_busy());
    let before = std::fs::read(slot.ctx.session_runtime.session.transcript_path(&id)).unwrap();
    for (index, requested) in [Some(id.clone()), None].into_iter().enumerate() {
        let request = format!("prompt-{index}");
        handle_command(
            b.clone(),
            ServeCommand::Prompt {
                id: Some(request.clone()),
                session_id: requested.clone(),
                text: "must not persist".into(),
                params: Default::default(),
            },
        )
        .await
        .unwrap();
        assert_eq!(
            frame(&b_buffer, |f| f["id"] == request).await["error"],
            "busy"
        );
    }
    assert_eq!(
        std::fs::read(slot.ctx.session_runtime.session.transcript_path(&id)).unwrap(),
        before
    );
    assert!(!slot.is_interrupted());
    slot.mark_idle();
    let provider = runtime.confirmation.provider_for_session(id.clone());
    let wait = tokio::spawn(async move {
        provider
            .confirm_decision(PrimitiveOperation::Write, "write test", "test", None, None)
            .await
    });
    let request = frame(&a_buffer, |f| f["subtype"] == "confirmation").await;
    let request_id = request["requestId"].as_str().unwrap().to_owned();
    flush(&b, &b_buffer).await;
    assert!(!read_ndjson_lines(&b_buffer)
        .iter()
        .any(|f| f["subtype"] == "confirmation"));
    let reply = ServeCommand::ControlResponse {
        request_id,
        session_id: Some(id.clone()),
        payload: serde_json::json!({"decision":"allow_once"}),
    };
    handle_command(b.clone(), reply.clone()).await.unwrap();
    assert!(runtime.confirmation.has_pending_session(&id));
    handle_command(a.clone(), reply).await.unwrap();
    assert!(matches!(
        wait.await.unwrap().unwrap(),
        ConfirmDecision::AllowOnce
    ));
    handle_command(
        b.clone(),
        ServeCommand::CloseSession {
            id: Some("close-view".into()),
            session_id: Some(id.clone()),
        },
    )
    .await
    .unwrap();
    assert!(b.active_session_id().is_none());
    assert_eq!(*slot.controller.lock(), Some(a.connection_id));
    assert!(runtime.registry.get(&id).is_some());
    runtime.emit_event(
        &id,
        serde_json::json!({"type":"agent_start", "sessionId":id}),
    );
    flush(&a, &a_buffer).await;
    flush(&b, &b_buffer).await;
    assert!(read_ndjson_lines(&a_buffer)
        .iter()
        .any(|f| f["type"] == "agent_start"));
    assert!(!read_ndjson_lines(&b_buffer)
        .iter()
        .any(|f| f["type"] == "agent_start"));
    b.disconnect().await.unwrap();
    assert!(!slot.is_interrupted());
    a.disconnect().await.unwrap();
    cleanup_session_slot(&a, &slot, SlotCleanup::Close)
        .await
        .unwrap();
}

#[tokio::test]
#[serial(env_lock)]
async fn disconnect_waits_for_slow_checkpoints_and_retains_timed_out_writers() {
    let _key = install_test_api_key();
    let (runtime, work) = shared_runtime(2);
    let (connection, _) = connect(&runtime);
    new_shared(&connection, work.path(), "new").await;
    let slot = runtime
        .registry
        .get(&connection.active_session_id().unwrap())
        .unwrap();
    let (release, pending) = tokio::sync::oneshot::channel();
    slot.ctx
        .session_runtime
        .checkpoint_record_tasks
        .lock()
        .push(tokio::spawn(async move {
            pending.await.unwrap();
        }));
    // A short failed drain must keep the writer reachable for real disconnect cleanup.
    assert!(
        !crate::api::chat::drain_checkpoint_record_tasks(
            &slot.ctx,
            std::time::Duration::from_millis(10),
        )
        .await
    );
    assert_eq!(
        slot.ctx
            .session_runtime
            .checkpoint_record_tasks
            .lock()
            .len(),
        1
    );
    let mut closing = tokio::spawn(async move { connection.disconnect().await });
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(4), &mut closing)
            .await
            .is_err(),
        "checkpoint cleanup must not use the 3s turn-stop deadline"
    );
    release.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), closing)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(*slot.controller.lock(), None);
    assert!(slot
        .ctx
        .session_runtime
        .checkpoint_record_tasks
        .lock()
        .is_empty());
    runtime.stop_all().await.unwrap();
}

#[tokio::test]
#[serial(env_lock)]
async fn disconnect_discards_queued_follow_up_before_the_next_controller_prompt() {
    use crate::core::llm::{ChatMessageRole, StreamEvent};
    let _key = install_test_api_key();
    let (runtime, work) = shared_runtime(3);
    let (a, output_a) = connect(&runtime);
    let (b, output_b) = connect_in_project(&runtime, work.path()).await;
    let (provider, requests) = RecordingMockLlm::new(vec![vec![
        Ok(StreamEvent::ContentDelta {
            delta: "B response".into(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".into(),
        }),
    ]]);
    let mut slot = create_session_slot(
        a.clone(),
        NewSessionParams {
            cwd: Some(work.path().to_string_lossy().into_owned()),
            ..Default::default()
        },
        true,
    )
    .await
    .unwrap();
    let ctx = Arc::get_mut(&mut Arc::get_mut(&mut slot).unwrap().ctx).unwrap();
    ctx.global_services.llm_resolver = Arc::new(FixedResolver::new(
        Arc::new(provider),
        "gpt-5.4",
        ctx.global_services.model_catalog.snapshot(),
    ));
    *slot.controller.lock() = Some(a.connection_id);
    runtime.registry.insert(slot.clone()).unwrap();
    register_slot_hooks(&a, &slot);
    a.set_active_session(&slot.session_id).unwrap();
    let cancel = slot.ctx.session_runtime.cancel_token.lock().clone();
    let worker_slot = slot.clone();
    assert!(slot.mark_busy());
    *slot.run_task.lock() = Some(tokio::spawn(async move {
        cancel.cancelled().await;
        worker_slot.mark_idle();
    }));
    handle_command(
        a.clone(),
        ServeCommand::FollowUp {
            id: Some("queued-a".into()),
            session_id: Some(slot.session_id.clone()),
            text: "A queued instruction".into(),
            params: Default::default(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        frame(&output_a, |f| f["id"] == "queued-a").await["payload"]["queued"],
        true
    );
    assert_eq!(slot.ctx.session_runtime.follow_up_queue.lock().len(), 1);
    let transcript = slot
        .ctx
        .session_runtime
        .session
        .transcript_path(&slot.session_id);
    let history = std::fs::read_to_string(&transcript).unwrap();
    assert!(history.contains("A queued instruction"));
    a.disconnect().await.unwrap();
    handle_command(
        b.clone(),
        ServeCommand::SwitchSession {
            id: Some("takeover".into()),
            session_id: slot.session_id.clone(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        frame(&output_b, |f| f["id"] == "takeover").await["payload"]["access"],
        "control"
    );
    let inherited_queue = slot.ctx.session_runtime.follow_up_queue.lock().clone();
    handle_command(
        b.clone(),
        ServeCommand::Prompt {
            id: Some("prompt-b".into()),
            session_id: Some(slot.session_id.clone()),
            text: "B new instruction".into(),
            params: Default::default(),
        },
    )
    .await
    .unwrap();
    frame(&output_b, |f| f["type"] == "agent_idle").await;
    runtime.stop_all().await.unwrap();
    assert!(
        inherited_queue.is_empty(),
        "old controller's queue survived takeover: {inherited_queue:?}"
    );
    let requests = requests.0.lock();
    assert_eq!(requests.len(), 1);
    let last_user = requests[0]
        .messages
        .iter()
        .rev()
        .find(|m| {
            m.role == ChatMessageRole::User && m.kind == crate::core::llm::MessageKind::Normal
        })
        .unwrap();
    assert_eq!(last_user.text_content(), Some("B new instruction"));
    assert!(
        std::fs::read_to_string(transcript)
            .unwrap()
            .starts_with(&history),
        "clearing pending input must not rewrite durable history"
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn child_view_is_scoped_filters_private_payloads_and_detaches() {
    use crate::core::{
        agent_loop::SubagentType, session::subagent_transcript::open_subagent_transcript,
    };
    use serde_json::json;
    let _key = install_test_api_key();
    let (runtime, work) = shared_runtime(2);
    let (owner, out) = connect(&runtime);
    new_shared(&owner, work.path(), "new").await;
    let parent = owner.active_session_id().unwrap();
    let child = format!("{parent}-child-view");
    let slot = runtime.registry.get(&parent).unwrap();
    let dir = crate::resolve_agent_trail_dir(&runtime.cfg).unwrap();
    let sink = open_subagent_transcript(
        dir.to_str().unwrap(),
        &child,
        SubagentType::Explorer,
        "model",
        &parent,
    )
    .unwrap();
    sink.append_message(json!({"role":"system","content":"PRIVATE_SYSTEM"}))
        .unwrap();
    sink.append_message(json!({"role":"assistant","content":"visible history","reasoning_continuation":{"opaque_payload":"PRIVATE_BLOB"}})).unwrap();
    let (viewer, view_out) = connect_in_project(&runtime, work.path()).await;
    handle_command(
        viewer.clone(),
        ServeCommand::SwitchSession {
            id: Some("observe".into()),
            session_id: parent.clone(),
        },
    )
    .await
    .unwrap();
    let emitter = crate::infra::ScopedEventEmitter::new(
        slot.ctx.global_services.event_bus.clone(),
        parent.clone(),
    );
    emitter.emit_payload("background.updated",json!({"type":"background.updated","row":{"id":child,"kind":"subagent","state":"running","revision":1}})).unwrap();
    let child_bus = emitter.for_session(&child);
    child_bus
        .emit_payload(
            "message_start",
            json!({"type":"message_start","assistantMessageId":"mid","message":{}}),
        )
        .unwrap();
    let delta = |text: &str| {
        child_bus.emit_payload("message_update",json!({"type":"message_update","assistantMessageId":"mid","message":{},"assistantMessageEvent":{"kind":"content_delta","delta":text,"signature":"PRIVATE_SIG"}})).unwrap()
    };
    delta("prefix");
    let view = |request: &str, id: &str| ServeCommand::ViewChild {
        id: Some(request.into()),
        session_id: parent.clone(),
        child_id: id.into(),
        cursor: None,
    };
    handle_command(viewer.clone(), view("watch", &child))
        .await
        .unwrap();
    let history = frame(&view_out, |f| f["id"] == "watch").await;
    assert!(history["success"] == true, "{history}");
    assert!(history.to_string().contains("visible history"));
    assert!(!history.to_string().contains("PRIVATE_"));
    frame(&view_out, |f| f["message"]["content"] == "prefix").await;
    delta("delta");
    let msg = frame(&view_out, |f| {
        f["assistantMessageEvent"]["delta"] == "delta"
    })
    .await;
    assert!(!msg.to_string().contains("PRIVATE_"));
    assert!(!read_ndjson_lines(&out)
        .iter()
        .any(|f| f["sessionId"] == child));
    handle_command(
        viewer.clone(),
        ServeCommand::CloseChildView {
            id: Some("leave".into()),
            session_id: parent.clone(),
        },
    )
    .await
    .unwrap();
    delta("after-close");
    for bad in [
        "other-child-x",
        "../child",
        &format!("{parent}-child-missing"),
    ] {
        handle_command(viewer.clone(), view(bad, bad))
            .await
            .unwrap();
        assert_eq!(frame(&view_out, |f| f["id"] == bad).await["success"], false);
    }
    assert!(runtime.registry.get(&child).is_none());
    assert!(owner.controls(&slot));
    assert!(!read_ndjson_lines(&view_out)
        .iter()
        .any(|f| f["assistantMessageEvent"]["delta"] == "after-close"));
    runtime.stop_all().await.unwrap();
}

#[tokio::test]
#[serial(env_lock)]
async fn targeted_stop_is_owned_idempotent_and_does_not_stop_siblings() {
    use super::super::types::BackgroundKind;
    use crate::core::{
        agent_loop::SubagentType,
        agent_registry::{SubagentOutcome, SubagentOutcomeLabel, SubagentSpawnContext},
    };
    let _key = install_test_api_key();
    let (runtime, work) = shared_runtime(2);
    let (owner, out) = connect(&runtime);
    let (observer, observed) = connect_in_project(&runtime, work.path()).await;
    new_shared(&owner, work.path(), "new").await;
    let session = owner.active_session_id().unwrap();
    let slot = owner.registry.get(&session).unwrap();
    handle_command(
        observer.clone(),
        ServeCommand::SwitchSession {
            id: Some("observe".into()),
            session_id: session.clone(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        frame(&observed, |f| f["id"] == "observe").await["payload"]["access"],
        "observe"
    );
    let job = |ctx: SubagentSpawnContext| async move {
        ctx.cancel_token.cancelled().await;
        SubagentOutcome {
            child_session_id: ctx.child_session_id,
            subagent_type: ctx.subagent_type,
            outcome_label: SubagentOutcomeLabel::Interrupted,
            error_message: None,
        }
    };
    let mut children = slot
        .ctx
        .agent_registry
        .start_described_batch(
            &session,
            SubagentType::Explorer,
            Some("call"),
            &["same", "same"],
            vec![job; 2],
        )
        .unwrap();
    let first = children.remove(0);
    let other = children.remove(0);
    let stop = |request: &str, id: &str| ServeCommand::StopBackground {
        id: Some(request.into()),
        session_id: session.clone(),
        task_id: id.into(),
        kind: BackgroundKind::Subagent,
    };
    handle_command(observer.clone(), stop("denied", &first.id))
        .await
        .unwrap();
    assert_eq!(
        frame(&observed, |f| f["id"] == "denied").await["error"],
        "read_only_session"
    );
    handle_command(owner.clone(), stop("wrong", "unowned"))
        .await
        .unwrap();
    assert_eq!(
        frame(&out, |f| f["id"] == "wrong").await["error"],
        "unknown_child"
    );
    handle_command(owner.clone(), stop("stop-one", &first.id))
        .await
        .unwrap();
    let reply = frame(&out, |f| f["id"] == "stop-one").await;
    assert_eq!(reply["payload"]["row"]["state"], "stopping");
    let id = first.id.clone();
    first.wait().await.unwrap();
    handle_command(owner.clone(), stop("again", &id))
        .await
        .unwrap();
    let final_row = frame(&out, |f| f["id"] == "again").await["payload"]["row"].clone();
    assert_eq!(final_row["state"], "stopped");
    handle_command(
        observer.clone(),
        ServeCommand::GetState {
            id: Some("snapshot".into()),
            session_id: Some(session.clone()),
        },
    )
    .await
    .unwrap();
    let snap = frame(&observed, |f| f["id"] == "snapshot").await;
    let rows = snap["payload"]["backgroundTasks"].as_array().unwrap();
    assert!(rows.contains(&final_row));
    assert!(rows
        .iter()
        .any(|r| r["id"] == other.id && r["state"] == "running"));
    assert_eq!(
        final_row,
        frame(&observed, |f| f["type"] == "background.updated"
            && f["row"]["state"] == "stopped")
        .await["row"]
    );
    runtime.stop_all().await.unwrap();
    other.wait().await.unwrap();
}

#[tokio::test]
#[serial(env_lock)]
async fn stop_timeout_retires_followups_even_if_the_controller_stays_connected() {
    use crate::core::{
        agent_loop::SubagentType,
        agent_registry::{SubagentOutcome, SubagentOutcomeLabel, SubagentSpawnContext},
        llm::ChatMessage,
        plan_runtime::explorer::ExplorerReport,
    };
    let _key = install_test_api_key();
    let (runtime, work) = shared_runtime(2);
    let (owner, _) = connect(&runtime);
    new_shared(&owner, work.path(), "new").await;
    let id = owner.active_session_id().unwrap();
    let slot = owner.registry.get(&id).unwrap();
    let reports = &slot.ctx.session_runtime.plan_runtime.child_reports;
    let batch = reports.batch().unwrap();
    let (release, gate) = tokio::sync::oneshot::channel();
    let child = slot
        .ctx
        .agent_registry
        .start_batch(
            &id,
            SubagentType::Explorer,
            vec![move |ctx: SubagentSpawnContext| async move {
                gate.await.unwrap();
                // A completion already in flight must be rejected by batch identity alone.
                batch.publish(
                    &ExplorerReport::default(),
                    &tokio_util::sync::CancellationToken::new(),
                );
                SubagentOutcome {
                    child_session_id: ctx.child_session_id,
                    subagent_type: ctx.subagent_type,
                    outcome_label: SubagentOutcomeLabel::Completed,
                    error_message: None,
                }
            }],
        )
        .unwrap()
        .pop()
        .unwrap();
    let queue = &slot.ctx.session_runtime.follow_up_queue;
    queue.lock().push(ChatMessage::user("pending input"));
    tokio::time::pause();
    let err = runtime.stop_slot_work(&slot).await.unwrap_err();
    assert!(err.to_string().contains("stop_timeout"));
    assert!(slot.ctx.agent_registry.has_children(&id));
    assert!(!reports.has_open_batch());
    assert!(queue.lock().is_empty());
    release.send(()).unwrap();
    child.wait().await.unwrap();
    assert!(queue.lock().is_empty() && !reports.ready() && !slot.is_busy());
    assert_eq!(*slot.controller.lock(), Some(owner.connection_id));
}

#[tokio::test]
#[serial(env_lock)]
async fn eof_stops_owned_background_processes_before_control_can_transfer() {
    let _key = install_test_api_key();
    let (runtime, work) = shared_runtime(2);
    let (a, _) = connect(&runtime);
    let (b, b_buffer) = connect_in_project(&runtime, work.path()).await;
    new_shared(&a, work.path(), "new").await;
    let id = a.active_session_id().unwrap();
    let slot = a.registry.get(&id).unwrap();
    let ticket = slot
        .ctx
        .session_runtime
        .bash_task_registry
        .spawn(
            "printf started; sleep 30".into(),
            Some(work.path().to_owned()),
        )
        .await
        .unwrap();
    handle_command(
        b.clone(),
        ServeCommand::SwitchSession {
            id: Some("before".into()),
            session_id: id.clone(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        frame(&b_buffer, |f| f["id"] == "before").await["payload"]["access"],
        "observe"
    );
    // An observer's EOF must not affect a controller's child; controller EOF must
    // join it, including late cleanup, before discarding unconsumed reports.
    let (cancelled_tx, cancelled_rx) = tokio::sync::oneshot::channel();
    let (release, cleanup) = tokio::sync::oneshot::channel();
    let queue = slot.ctx.session_runtime.follow_up_queue.clone();
    let children = slot
        .ctx
        .agent_registry
        .start_batch(
            &id,
            crate::core::agent_loop::SubagentType::Explorer,
            vec![
                move |ctx: crate::core::agent_registry::SubagentSpawnContext| async move {
                    ctx.cancel_token.cancelled().await;
                    cancelled_tx.send(()).unwrap();
                    cleanup.await.unwrap();
                    queue
                        .lock()
                        .push(crate::core::llm::ChatMessage::user("late child completion"));
                    crate::core::agent_registry::SubagentOutcome {
                        child_session_id: ctx.child_session_id,
                        subagent_type: ctx.subagent_type,
                        outcome_label:
                            crate::core::agent_registry::SubagentOutcomeLabel::Interrupted,
                        error_message: None,
                    }
                },
            ],
        )
        .unwrap();
    drop(children);
    let (observer, _) = connect_in_project(&runtime, work.path()).await;
    observer.disconnect().await.unwrap();
    assert!(slot.ctx.agent_registry.has_children(&id));
    // Exercise real EOF on the shared connection loop, not only its cleanup helper.
    let eof = tokio::spawn(super::super::stdin::run_connection_loop(
        a.clone(),
        tokio::io::BufReader::new(tokio::io::empty()),
    ));
    cancelled_rx.await.unwrap();
    assert!(!eof.is_finished());
    release.send(()).unwrap();
    eof.await.unwrap().unwrap_or_else(|error| {
        panic!(
            "EOF cleanup failed: {error}; bash tasks: {:?}",
            slot.ctx.session_runtime.bash_task_registry.list()
        )
    });
    assert!(!slot.ctx.agent_registry.has_children(&id));
    assert!(slot.ctx.session_runtime.follow_up_queue.lock().is_empty());
    assert!(a.is_closed());
    assert!(
        a.lifetime.child_token().is_cancelled(),
        "late-starting work inherits EOF cancellation"
    );
    assert!(slot
        .ctx
        .session_runtime
        .bash_task_registry
        .list()
        .iter()
        .any(|task| task.task_id == ticket.task_id
            && matches!(
                task.status,
                crate::core::tools::primitive::BashTaskStatus::Stopped
            )));
    assert_eq!(*slot.controller.lock(), None);
    handle_command(
        b.clone(),
        ServeCommand::SwitchSession {
            id: Some("after".into()),
            session_id: id.clone(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        frame(&b_buffer, |f| f["id"] == "after").await["payload"]["access"],
        "control"
    );
    assert_eq!(*slot.controller.lock(), Some(b.connection_id));
    cleanup_session_slot(&b, &slot, SlotCleanup::Close)
        .await
        .unwrap();
}

#[tokio::test]
#[serial(env_lock)]
async fn capacity_reclaims_only_idle_unviewed_slots_and_preserves_their_history() {
    let _key = install_test_api_key();
    let (runtime, work) = shared_runtime(2);
    let (owner, _) = connect(&runtime);
    let (observer, _) = connect_in_project(&runtime, work.path()).await;
    let (newcomer, buffer) = connect(&runtime);
    new_shared(&owner, work.path(), "b").await;
    let b = owner.active_session_id().unwrap();
    let b_path = owner
        .registry
        .get(&b)
        .unwrap()
        .ctx
        .session_runtime
        .session
        .transcript_path(&b);
    new_shared(&owner, work.path(), "a").await;
    let a = owner.active_session_id().unwrap();
    handle_command(
        observer.clone(),
        ServeCommand::SwitchSession {
            id: None,
            session_id: b.clone(),
        },
    )
    .await
    .unwrap();
    new_shared(&newcomer, work.path(), "full").await;
    assert_eq!(
        frame(&buffer, |f| f["id"] == "full").await["error"],
        "too_many_sessions"
    );
    handle_command(
        observer.clone(),
        ServeCommand::CloseSession {
            id: None,
            session_id: Some(b.clone()),
        },
    )
    .await
    .unwrap();
    new_shared(&newcomer, work.path(), "room").await;
    assert_eq!(frame(&buffer, |f| f["id"] == "room").await["success"], true);
    assert!(runtime.registry.get(&a).is_some());
    assert!(runtime.registry.get(&b).is_none());
    assert!(b_path.is_file(), "eviction must not delete history");
    newcomer.disconnect().await.unwrap();
    handle_command(
        owner.clone(),
        ServeCommand::SwitchSession {
            id: None,
            session_id: b.clone(),
        },
    )
    .await
    .unwrap();
    assert_eq!(owner.active_session_id(), Some(b));
    for slot in runtime.registry.slots() {
        cleanup_session_slot(&owner, &slot, SlotCleanup::Close)
            .await
            .unwrap();
    }
}

// Establish the existing connection scope through normal session operations. Closing
// the bootstrap also checks that scope survives an empty live-session selection.
async fn connect_in_project(
    runtime: &Arc<runtime::ServeRuntime>,
    cwd: &std::path::Path,
) -> (Arc<ServeState>, SharedWriterBuffer) {
    let (connection, output) = connect(runtime);
    new_shared(&connection, cwd, "bootstrap").await;
    handle_command(
        connection.clone(),
        ServeCommand::CloseSession {
            id: None,
            session_id: None,
        },
    )
    .await
    .unwrap();
    assert!(connection.active_session_id().is_none());
    assert!(connection.current_scope().is_some());
    (connection, output)
}

async fn assert_project_observation(
    host_cwd: &std::path::Path,
    window_cwd: &std::path::Path,
    visible: bool,
) {
    let (runtime, _work) = shared_runtime(3);
    let host = connect_acp(&runtime);
    new_shared(&host, host_cwd, "host").await;
    let id = host.active_session_id().unwrap();
    let host_slot = runtime.registry.get(&id).unwrap();
    let history = host_slot.ctx.session_runtime.session.transcript_path(&id);
    let original = std::fs::read(&history).unwrap();
    let (viewer, output) = connect_in_project(&runtime, window_cwd).await;
    let key = viewer.current_scope().unwrap().key;
    let manager = super::super::scoped_session_manager(&viewer).unwrap();
    let durable_current = manager.current_session_id().unwrap();

    for phase in ["live", "cold"] {
        if phase == "cold" {
            host.disconnect().await.unwrap();
            cleanup_session_slot(&host, &host_slot, SlotCleanup::Close)
                .await
                .unwrap();
        }
        let list_id = format!("list-{phase}");
        handle_command(
            viewer.clone(),
            ServeCommand::ListSessions {
                id: Some(list_id.clone()),
                scope: Some(ListSessionsScope::Disk),
            },
        )
        .await
        .unwrap();
        let list = frame(&output, |f| f["id"] == list_id).await;
        assert_eq!(list["payload"]["sessionKey"], key);
        assert_eq!(
            list["payload"]["sessions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["sessionId"] == id),
            visible,
            "{phase}: {list}"
        );

        if !visible && phase == "live" {
            // A caller cannot bypass switch_session by asking for history/state directly.
            for kind in [
                "get_state",
                "get_messages",
                "list_checkpoints",
                "get_session_files",
                "interrupt",
                "delete_session",
            ] {
                let request = serde_json::json!({"type":kind,"id":kind,"sessionId":id});
                super::super::stdin::dispatch_command(
                    viewer.clone(),
                    serde_json::from_value::<ServeCommand>(request).unwrap(),
                )
                .await
                .unwrap();
                let reply = frame(&output, |f| f["id"] == kind).await;
                assert_eq!(reply["error"], "unknown_session", "{kind}: {reply}");
            }
        }
        if !visible && phase == "cold" {
            super::super::stdin::dispatch_command(
                viewer.clone(),
                ServeCommand::DeleteSession {
                    id: Some("cold-delete".into()),
                    session_id: id.clone(),
                },
            )
            .await
            .unwrap();
            let reply = frame(&output, |f| f["id"] == "cold-delete").await;
            assert_eq!(reply["success"], false, "{reply}");
        }
        let open_id = format!("open-{phase}");
        super::super::stdin::dispatch_command(
            viewer.clone(),
            ServeCommand::SwitchSession {
                id: Some(open_id.clone()),
                session_id: id.clone(),
            },
        )
        .await
        .unwrap();
        let opened = frame(&output, |f| f["id"] == open_id).await;
        if visible {
            assert_eq!(
                opened["payload"]["access"],
                if phase == "cold" {
                    "control"
                } else {
                    "observe"
                },
                "{phase}: {opened}"
            );
            assert_eq!(viewer.active_session_id().as_deref(), Some(id.as_str()));
        } else {
            assert_eq!(opened["error"], "unknown_session", "{phase}: {opened}");
            assert!(viewer.active_session_id().is_none());
            assert_eq!(runtime.registry.get(&id).is_some(), phase == "live");
        }
        assert_eq!(viewer.current_scope().unwrap().key, key);
        assert_eq!(
            manager.current_session_id().unwrap(),
            if visible && phase == "cold" {
                Some(id.clone())
            } else {
                durable_current.clone()
            }
        );
        assert_eq!(std::fs::read(&history).unwrap(), original);
        if phase == "live" {
            assert_eq!(*host_slot.controller.lock(), Some(host.connection_id));
            runtime.emit_event(
                &id,
                serde_json::json!({"type":"agent_start","sessionId":id}),
            );
            flush(&viewer, &output).await;
            assert_eq!(
                read_ndjson_lines(&output)
                    .iter()
                    .any(|f| f["type"] == "agent_start" && f["sessionId"] == id),
                visible
            );
        }
    }
    runtime.stop_all().await.unwrap();
}

fn project_scope_git(cwd: &std::path::Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .current_dir(cwd)
        .args([
            "-c",
            "user.name=Tomcat Test",
            "-c",
            "user.email=test@example.com",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn project_observation_preserves_repo_root_and_subdirectory_aggregation() {
    let _key = install_test_api_key();
    let repo = tempfile::tempdir().unwrap();
    project_scope_git(repo.path(), &["init", "-q"]);
    let child = repo.path().join("nested");
    std::fs::create_dir(&child).unwrap();
    assert_project_observation(&child, repo.path(), true).await;
    assert_project_observation(repo.path(), &child, true).await;
}

#[tokio::test]
#[serial(env_lock)]
async fn project_observation_rejects_other_repository_live_and_cold() {
    let _key = install_test_api_key();
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    for cwd in [a.path(), b.path()] {
        project_scope_git(cwd, &["init", "-q"]);
    }
    assert_project_observation(a.path(), b.path(), false).await;
}

#[tokio::test]
#[serial(env_lock)]
async fn project_observation_worktree_requires_its_own_window() {
    let _key = install_test_api_key();
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("main");
    let worktree = dir.path().join("task");
    std::fs::create_dir(&repo).unwrap();
    project_scope_git(&repo, &["init", "-q"]);
    project_scope_git(&repo, &["commit", "--allow-empty", "-qm", "fixture"]);
    project_scope_git(
        &repo,
        &[
            "worktree",
            "add",
            "--detach",
            worktree.to_str().unwrap(),
            "HEAD",
        ],
    );
    assert_project_observation(&worktree, &repo, false).await;
    assert_project_observation(&worktree, &worktree, true).await;
}

#[tokio::test]
#[serial(env_lock)]
async fn project_observation_without_git_uses_the_existing_directory_scope() {
    let _key = install_test_api_key();
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    assert_project_observation(a.path(), b.path(), false).await;
    assert_project_observation(a.path(), a.path(), true).await;
}

#[tokio::test]
#[serial(env_lock)]
async fn project_observation_cannot_infer_window_scope_from_a_foreign_live_id() {
    let _key = install_test_api_key();
    let (runtime, work) = shared_runtime(1);
    let host = connect_acp(&runtime);
    new_shared(&host, work.path(), "host").await;
    let id = host.active_session_id().unwrap();
    let (viewer, output) = connect(&runtime);
    for kind in ["switch_session", "get_state", "get_messages"] {
        let request = serde_json::json!({"type":kind,"id":kind,"sessionId":id});
        super::super::stdin::dispatch_command(
            viewer.clone(),
            serde_json::from_value::<ServeCommand>(request).unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(
            frame(&output, |f| f["id"] == kind).await["error"],
            "unknown_session"
        );
    }
    assert!(viewer.current_scope().is_none());
    assert!(viewer.active_session_id().is_none());
    runtime.stop_all().await.unwrap();
}

#[test]
fn shared_sessions_require_existing_absolute_cwd_but_stdio_keeps_its_default() {
    assert!(resolve_session_paths(&NewSessionParams::default(), true).is_err());
    assert!(resolve_session_paths(&NewSessionParams::default(), false).is_ok());
    for value in ["", ".", "relative/project"] {
        let params = NewSessionParams {
            cwd: Some(value.into()),
            ..Default::default()
        };
        assert!(resolve_session_paths(&params, true).is_err());
    }
    let dir = tempfile::tempdir().unwrap();
    let params = NewSessionParams {
        cwd: Some(dir.path().join("missing").to_string_lossy().into()),
        ..Default::default()
    };
    assert!(resolve_session_paths(&params, true).is_err());
    let params = NewSessionParams {
        cwd: Some(dir.path().to_string_lossy().into()),
        ..Default::default()
    };
    assert_eq!(
        resolve_session_paths(&params, true).unwrap().0,
        crate::infra::platform::canonicalize(dir.path()).unwrap()
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn native_writes_transfer_control_without_interrupting_either_subscription() {
    use crate::core::llm::StreamEvent;
    let _key = install_test_api_key();
    let (runtime, work) = shared_runtime(3);
    let (a, a_output) = connect(&runtime);
    let (b, b_output) = connect_in_project(&runtime, work.path()).await;
    let (provider, requests) = RecordingMockLlm::new(
        (0..2)
            .map(|_| {
                vec![
                    Ok(StreamEvent::ContentDelta {
                        delta: "shared response".into(),
                    }),
                    Ok(StreamEvent::FinishReason {
                        reason: "stop".into(),
                    }),
                ]
            })
            .collect(),
    );
    let slot = shared_slot_with_provider(&a, work.path(), Arc::new(provider)).await;
    handle_command(
        b.clone(),
        ServeCommand::SwitchSession {
            id: Some("watch".into()),
            session_id: slot.session_id.clone(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        frame(&b_output, |f| f["id"] == "watch").await["payload"]["access"],
        "observe"
    );
    for (new, old, new_output, old_output) in [
        (&b, &a, &b_output, &a_output),
        (&a, &b, &a_output, &b_output),
    ] {
        flush(&a, &a_output).await;
        flush(&b, &b_output).await;
        a_output.0.lock().clear();
        b_output.0.lock().clear();
        handle_command(
            new.clone(),
            ServeCommand::Prompt {
                id: Some("continue".into()),
                session_id: Some(slot.session_id.clone()),
                text: "continue here".into(),
                params: Default::default(),
            },
        )
        .await
        .unwrap();
        assert_eq!(
            frame(new_output, |f| f["id"] == "continue").await["success"],
            true
        );
        for (output, access) in [(new_output, "control"), (old_output, "observe")] {
            frame(output, |f| f["type"] == "agent_idle").await;
            let frames = read_ndjson_lines(output);
            let access_index = frames
                .iter()
                .position(|f| f["type"] == "session_access" && f["access"] == access)
                .unwrap();
            let start_index = frames
                .iter()
                .position(|f| f["type"] == "agent_start")
                .unwrap();
            assert!(access_index < start_index, "{frames:?}");
            assert!(frames.iter().any(|f| f["type"] == "message_update"));
        }
        assert_eq!(*slot.controller.lock(), Some(new.connection_id));
        assert_eq!(old.active_session_id(), Some(slot.session_id.clone()));
    }
    assert_eq!(requests.0.lock().len(), 2);
    runtime.stop_all().await.unwrap();
}

#[tokio::test]
#[serial(env_lock)]
async fn current_controller_name_follows_handoffs_for_all_observers_not_origin() {
    use crate::core::llm::StreamEvent;
    let _key = install_test_api_key();
    let (runtime, work) = shared_runtime(5);
    let original = connect_acp(&runtime);
    original.set_acp_client_name(Some("original host".into()));
    let (provider, _) = RecordingMockLlm::new(vec![vec![
        Ok(StreamEvent::ContentDelta {
            delta: "done".into(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".into(),
        }),
    ]]);
    let slot = shared_slot_with_provider(&original, work.path(), Arc::new(provider)).await;
    let id = slot.session_id.clone();
    let (view, output) = connect_in_project(&runtime, work.path()).await;
    let (watcher, watch_output) = connect_in_project(&runtime, work.path()).await;
    let foreign_work = tempfile::tempdir().unwrap();
    let (foreign, foreign_output) = connect_in_project(&runtime, foreign_work.path()).await;
    for (connection, buffer) in [(&view, &output), (&watcher, &watch_output)] {
        handle_command(
            connection.clone(),
            ServeCommand::SwitchSession {
                id: Some("watch".into()),
                session_id: id.clone(),
            },
        )
        .await
        .unwrap();
        let opened = frame(buffer, |f| f["id"] == "watch").await;
        assert_eq!(opened["payload"]["controllerName"], "original host");
        assert_eq!(opened["payload"]["origin"]["clientName"], "original host");
        flush(connection, buffer).await;
        buffer.0.lock().clear();
    }
    let replacement = connect_acp(&runtime);
    replacement.set_acp_client_name(Some(" current host ".into()));
    let load = || ServeCommand::LoadExternalSession {
        id: Some("load".into()),
        session_id: id.clone(),
        cwd: work.path().to_string_lossy().into_owned(),
    };
    handle_command(replacement.clone(), load()).await.unwrap();
    for (connection, buffer) in [(&view, &output), (&watcher, &watch_output)] {
        let event = frame(buffer, |f| f["type"] == "session_access").await;
        assert_eq!(event["access"], "observe");
        assert_eq!(event["controllerName"], "current host");
        flush(connection, buffer).await;
        buffer.0.lock().clear();
    }
    // A native write clears the old ACP name for both the new owner and a third observer.
    handle_command(
        view.clone(),
        ServeCommand::Prompt {
            id: Some("native".into()),
            session_id: Some(id.clone()),
            text: "continue".into(),
            params: Default::default(),
        },
    )
    .await
    .unwrap();
    for (buffer, access) in [(&output, "control"), (&watch_output, "observe")] {
        frame(buffer, |f| f["type"] == "agent_idle").await;
        let frames = read_ndjson_lines(buffer);
        let changed = frames
            .iter()
            .position(|f| f["type"] == "session_access")
            .unwrap();
        let started = frames
            .iter()
            .position(|f| f["type"] == "agent_start")
            .unwrap();
        assert!(changed < started);
        assert_eq!(frames[changed]["access"], access);
        assert!(frames[changed]["controllerName"].is_null());
    }
    handle_command(replacement.clone(), load()).await.unwrap();
    // Opening a warm session also distinguishes the current controller from its origin.
    handle_command(
        watcher.clone(),
        ServeCommand::SwitchSession {
            id: Some("reopen".into()),
            session_id: id.clone(),
        },
    )
    .await
    .unwrap();
    let reopened = frame(&watch_output, |f| f["id"] == "reopen").await;
    assert_eq!(reopened["payload"]["controllerName"], "current host");
    assert_eq!(reopened["payload"]["origin"]["clientName"], "original host");
    for (connection, buffer) in [(&view, &output), (&watcher, &watch_output)] {
        flush(connection, buffer).await;
        buffer.0.lock().clear();
    }
    replacement.disconnect().await.unwrap();
    for buffer in [&output, &watch_output] {
        let event = frame(buffer, |f| f["type"] == "session_access").await;
        assert_eq!(event["access"], "observe");
        assert!(event["controllerName"].is_null());
    }
    flush(&foreign, &foreign_output).await;
    // flush() itself inserts response fences for every live session.
    let foreign_frames = read_ndjson_lines(&foreign_output);
    assert!(
        !foreign_frames
            .iter()
            .any(|f| f["sessionId"] == id && f["type"] != "response"),
        "{foreign_frames:?}"
    );
    runtime.stop_all().await.unwrap();
}

#[tokio::test]
#[serial(env_lock)]
async fn shared_input_prepares_attachments_steers_and_stops_without_acquiring() {
    use serde_json::json;
    let _key = install_test_api_key();
    let (runtime, work) = shared_runtime(3);
    let (writer, host_output) = spawn_buffered_writer(&runtime.cfg.serve);
    let host = ServeState::connect_acp(runtime.clone(), writer);
    host.initialized.store(true, Ordering::SeqCst);
    let (model, requests) = PausedModel::new();
    let slot = shared_slot_with_provider(&host, work.path(), model.clone()).await;
    let id = slot.session_id.clone();
    let (view, output) = connect_in_project(&runtime, work.path()).await;
    handle_command(
        view.clone(),
        ServeCommand::SwitchSession {
            id: Some("watch".into()),
            session_id: id.clone(),
        },
    )
    .await
    .unwrap();
    let prompt = || ServeCommand::Prompt {
        id: Some("host-prompt".into()),
        session_id: Some(id.clone()),
        text: "work on this".into(),
        params: Default::default(),
    };
    handle_command(host.clone(), prompt()).await.unwrap();
    model.wait_until_paused().await;
    let steer = |label: &str| {
        serde_json::from_value::<ServeCommand>(json!({
            "type":"steer", "id":label, "sessionId":id, "text":"test first", "params":{"onlyIfRunning":true}
        }))
        .unwrap()
    };
    handle_command(view.clone(), steer("steer")).await.unwrap();
    assert_eq!(
        frame(&output, |f| f["id"] == "steer").await["payload"]["queued"],
        true
    );
    let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVQI12NgYAAAAAIAAeUn3vwAAAAASUVORK5CYII=";
    handle_command(
        view.clone(),
        serde_json::from_value::<ServeCommand>(json!({
            "type":"ingest_attachment", "id":"image", "sessionId":id,
            "attachment":{"kind":"image", "mimeType":"image/png", "dataBase64":png}
        }))
        .unwrap(),
    )
    .await
    .unwrap();
    let image = frame(&output, |f| f["id"] == "image").await;
    assert_eq!(image["success"], true, "{image}");
    let sha = &image["payload"]["blobSha"];
    for wire in [
        json!({"type":"retain_attachment_leases", "id":"lease", "sessionId":id, "params":{"attachments":[{"blobSha":sha}]}}),
        json!({"type":"cache_attachment_thumbnail", "id":"thumbnail", "sessionId":id, "thumbnail":{"sourceSha":sha,"thumbBase64":png}}),
    ] {
        let request = wire["id"].as_str().unwrap().to_owned();
        handle_command(
            view.clone(),
            serde_json::from_value::<ServeCommand>(wire).unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(
            frame(&output, |f| f["id"] == request).await["success"],
            true
        );
    }
    assert!(!slot.is_interrupted());
    assert_eq!(*slot.controller.lock(), Some(host.connection_id));
    for (connection, output) in [(&view, &output), (&host, &host_output)] {
        flush(connection, output).await;
        assert!(!read_ndjson_lines(output)
            .iter()
            .any(|f| f["type"] == "session_access" && f["sessionId"] == id));
    }
    model.release.notify_one();
    assert_eq!(
        frame(&output, |f| f["type"] == "agent_idle").await["outcome"],
        "completed"
    );
    assert!(requests
        .0
        .lock()
        .get(1)
        .unwrap()
        .messages
        .iter()
        .any(|message| message.text_content() == Some("test first")));
    handle_command(view.clone(), steer("late-steer"))
        .await
        .unwrap();
    assert_eq!(
        frame(&output, |f| f["id"] == "late-steer").await["error"],
        "not_running"
    );
    model.pause_next.store(true, Ordering::SeqCst);
    handle_command(host.clone(), prompt()).await.unwrap();
    model.wait_until_paused().await;
    handle_command(
        view.clone(),
        ServeCommand::Interrupt {
            id: Some("stop".into()),
            session_id: Some(id.clone()),
        },
    )
    .await
    .unwrap();
    frame(&output, |f| {
        f["type"] == "agent_idle" && f["outcome"] == "interrupted"
    })
    .await;
    assert_eq!(*slot.controller.lock(), Some(host.connection_id));
    runtime.stop_all().await.unwrap();
}

#[tokio::test]
#[serial(env_lock)]
async fn background_work_blocks_native_writes_and_acp_load_without_cancelling_it() {
    use serde_json::json;
    let _key = install_test_api_key();
    let (runtime, work) = shared_runtime(3);
    let (owner, _) = connect(&runtime);
    new_shared(&owner, work.path(), "owner").await;
    let id = owner.active_session_id().unwrap();
    let slot = runtime.registry.get(&id).unwrap();
    let (view, output) = connect_in_project(&runtime, work.path()).await;
    let host = connect_acp(&runtime);
    let ticket = slot
        .ctx
        .session_runtime
        .bash_task_registry
        .spawn("sleep 30".into(), Some(work.path().to_owned()))
        .await
        .unwrap();
    for wire in [
        json!({"type":"prompt", "id":"prompt", "sessionId":id, "text":"do not take over"}),
        json!({"type":"set_model", "id":"model", "sessionId":id, "model":"gpt-5.4"}),
        json!({"type":"steer", "id":"plain-steer", "sessionId":id, "text":"may start a turn"}),
    ] {
        let request = wire["id"].as_str().unwrap().to_owned();
        handle_command(
            view.clone(),
            serde_json::from_value::<ServeCommand>(wire).unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(
            frame(&output, |f| f["id"] == request).await["error"],
            "busy"
        );
    }
    let error = handle_command(
        host,
        ServeCommand::LoadExternalSession {
            id: None,
            session_id: id.clone(),
            cwd: work.path().to_string_lossy().into_owned(),
        },
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("session_in_use"));
    assert_eq!(*slot.controller.lock(), Some(owner.connection_id));
    assert!(slot
        .ctx
        .session_runtime
        .bash_task_registry
        .list()
        .iter()
        .any(|task| task.task_id == ticket.task_id
            && matches!(
                task.status,
                crate::core::tools::primitive::BashTaskStatus::Running
            )));
    runtime.stop_all().await.unwrap();
}

#[tokio::test]
#[serial(env_lock)]
async fn late_acp_cancel_cleanup_does_not_touch_the_new_controller() {
    let _key = install_test_api_key();
    let (runtime, work) = shared_runtime(3);
    let host = connect_acp(&runtime);
    let (model, _) = PausedModel::new();
    let slot = shared_slot_with_provider(&host, work.path(), model.clone()).await;
    let id = slot.session_id.clone();
    let (native, output) = connect_in_project(&runtime, work.path()).await;
    handle_command(
        native.clone(),
        ServeCommand::Prompt {
            id: Some("takeover".into()),
            session_id: Some(id.clone()),
            text: "continue".into(),
            params: Default::default(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        frame(&output, |f| f["id"] == "takeover").await["success"],
        true
    );
    model.wait_until_paused().await;
    host.finish_cancelled_turn(&id).await.unwrap();
    assert!(!slot.is_interrupted());
    assert!(slot.is_busy());
    assert_eq!(*slot.controller.lock(), Some(native.connection_id));
    model.release.notify_one();
    assert_eq!(
        frame(&output, |f| f["type"] == "agent_idle").await["outcome"],
        "completed"
    );
    runtime.stop_all().await.unwrap();
}

async fn shared_slot_with_provider(
    state: &Arc<ServeState>,
    cwd: &std::path::Path,
    provider: Arc<dyn crate::core::llm::LlmProvider>,
) -> Arc<registry::SessionSlot> {
    let mut slot = create_session_slot(
        state.clone(),
        NewSessionParams {
            cwd: Some(cwd.to_string_lossy().into_owned()),
            ..Default::default()
        },
        true,
    )
    .await
    .unwrap();
    let ctx = Arc::get_mut(&mut Arc::get_mut(&mut slot).unwrap().ctx).unwrap();
    ctx.global_services.llm_resolver = Arc::new(FixedResolver::new(
        provider,
        "gpt-5.4",
        ctx.global_services.model_catalog.snapshot(),
    ));
    *slot.controller.lock() = Some(state.connection_id);
    state.registry.insert(slot.clone()).unwrap();
    register_slot_hooks(state, &slot);
    state.set_active_session(&slot.session_id).unwrap();
    slot
}

/// Pause the provider at a tool-call response without sleep-based races.
struct PausedModel {
    inner: RecordingMockLlm,
    pause_next: std::sync::atomic::AtomicBool,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
impl PausedModel {
    fn new() -> (Arc<Self>, SharedRequests) {
        use crate::core::llm::StreamEvent;
        let done = || {
            vec![
                Ok(StreamEvent::ContentDelta {
                    delta: "done".into(),
                }),
                Ok(StreamEvent::FinishReason {
                    reason: "stop".into(),
                }),
            ]
        };
        let (inner, requests) = RecordingMockLlm::new(vec![
            vec![
                Ok(StreamEvent::ToolCallDelta {
                    index: 0,
                    id: Some("todo".into()),
                    name: Some("todos".into()),
                    arguments_delta: Some(
                        r#"{"ops":[{"kind":"upsert","id":"check","content":"Check input"}]}"#
                            .into(),
                    ),
                }),
                Ok(StreamEvent::FinishReason {
                    reason: "tool_calls".into(),
                }),
            ],
            done(),
            done(),
        ]);
        (
            Arc::new(Self {
                inner,
                pause_next: true.into(),
                entered: Default::default(),
                release: Default::default(),
            }),
            requests,
        )
    }
    async fn wait_until_paused(&self) {
        tokio::time::timeout(std::time::Duration::from_secs(3), self.entered.notified())
            .await
            .unwrap();
    }
}
#[async_trait::async_trait]
impl crate::core::llm::LlmProvider for PausedModel {
    fn provider_name(&self) -> &str {
        "paused-mock"
    }
    async fn chat(
        &self,
        _: crate::core::llm::ChatRequest,
    ) -> Result<crate::core::llm::ChatResponse, crate::AppError> {
        unreachable!()
    }
    async fn chat_stream(
        &self,
        request: crate::core::llm::ChatRequest,
    ) -> Result<
        Box<
            dyn tokio_stream::Stream<Item = Result<crate::core::llm::StreamEvent, crate::AppError>>
                + Send
                + Unpin,
        >,
        crate::AppError,
    > {
        let stream = crate::core::llm::LlmProvider::chat_stream(&self.inner, request).await?;
        if self.pause_next.swap(false, Ordering::SeqCst) {
            self.entered.notify_one();
            self.release.notified().await;
        }
        Ok(stream)
    }
    fn count_tokens(&self, _: &[crate::core::llm::ChatMessage]) -> Result<u32, crate::AppError> {
        Ok(0)
    }
}

#[tokio::test]
#[serial(env_lock)]
async fn native_plan_revision_keeps_plan_identity_and_restores_human_policy() {
    use crate::core::llm::StreamEvent;
    use crate::core::plan_runtime::{file_store, InteractionPolicy, PlanStartPolicy};
    let _key = install_test_api_key();
    let (runtime, work) = shared_runtime(3);
    let host = connect_acp(&runtime);
    let (provider, requests) = RecordingMockLlm::new(vec![vec![
        Ok(StreamEvent::ContentDelta {
            delta: "Revise tests before implementation".into(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".into(),
        }),
    ]]);
    let slot = shared_slot_with_provider(&host, work.path(), Arc::new(provider)).await;
    let plan_path = work.path().join("same-plan.plan.md");
    let plan = file_store::PlanFile {
        frontmatter: serde_json::from_value(serde_json::json!({
            "plan_id":"shared-plan", "goal":"revise this plan", "state":"pending",
            "created_at":chrono::Utc::now().to_rfc3339(), "schema_version":1,
            "todos":[{"id":"implement","content":"Implement after tests","status":"pending"}]
        }))
        .unwrap(),
        body: "## Plan\nImplement before testing\n".into(),
    };
    file_store::write_plan(&plan_path, &plan, 2_000).unwrap();
    let plans = &slot.ctx.session_runtime.plan_runtime;
    plans.enter_plan().unwrap();
    plans.bind_plan_file_for_test(plan_path.clone());
    plans.set_interaction_policy(InteractionPolicy {
        start: PlanStartPolicy::Auto,
        ..Default::default()
    });
    host.disconnect().await.unwrap();
    let (native, output) = connect_in_project(&runtime, work.path()).await;
    handle_command(
        native.clone(),
        ServeCommand::Prompt {
            id: Some("revise".into()),
            session_id: Some(slot.session_id.clone()),
            text: "Quote: Implement before testing. Change it to testing first.".into(),
            params: Default::default(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        frame(&output, |f| f["type"] == "agent_idle").await["outcome"],
        "completed"
    );
    assert_eq!(plans.active_plan().unwrap().id, "shared-plan");
    assert_eq!(plans.active_plan_path(), Some(plan_path.clone()));
    assert_eq!(plans.mode(), crate::core::session::AgentMode::Plan);
    assert_eq!(plans.interaction_policy(), InteractionPolicy::default());
    assert_eq!(
        file_store::read_plan(&plan_path).unwrap().frontmatter.state,
        file_store::PlanFileState::Pending
    );
    assert!(requests.0.lock()[0].messages.iter().any(|m| m
        .text_content()
        .is_some_and(|text| text.contains("Change it to testing first"))));
    runtime.stop_all().await.unwrap();
}

#[tokio::test]
#[serial(env_lock)]
async fn duplicate_registry_insertion_never_replaces_a_live_slot() {
    let _key = install_test_api_key();
    let (state, _, _temp, slot) = build_initialized_state_with_streams(vec![]).await;
    let registry = Arc::new(registry::ChatContextRegistry::new(1));
    let accepted = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let registry = registry.clone();
                let slot = slot.clone();
                scope.spawn(move || registry.insert(slot).is_ok())
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| usize::from(handle.join().unwrap()))
            .sum::<usize>()
    });
    assert_eq!(accepted, 1);
    assert_eq!(registry.len(), 1);
    assert_eq!(registry.list().len(), 1);
    assert!(Arc::ptr_eq(&registry.get(&slot.session_id).unwrap(), &slot));
    cleanup_session_slot(&state, &slot, SlotCleanup::Close)
        .await
        .unwrap();
}
