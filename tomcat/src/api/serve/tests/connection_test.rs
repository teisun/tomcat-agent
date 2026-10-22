use super::*;
use serial_test::serial;
use std::sync::atomic::Ordering;
use std::sync::Arc;

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
    let error = handle_command(rival, load(&cwd)).await.unwrap_err();
    assert!(error.to_string().contains("session_in_use"));
    assert_eq!(*loaded.controller.lock(), Some(next.connection_id));

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
        assert_eq!(row(&host_id)["access"], "observe");
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
async fn acp_origin_survives_disconnect_and_reload_without_observer_mutations() {
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
        assert_eq!(response["payload"]["access"], "observe");
        assert_eq!(
            response["payload"]["origin"],
            serde_json::json!({"kind":"acp","clientName":"paperclip"})
        );
        assert_eq!(
            manager.current_session_entry().unwrap().unwrap().session_id,
            native
        );
        assert_eq!(std::fs::read(&history).unwrap(), original_bytes);
    }
    cleanup_session_slot(&host, &external, SlotCleanup::Close)
        .await
        .unwrap();
    handle_command(
        viewer.clone(),
        ServeCommand::DeleteSession {
            id: Some("deny-delete".into()),
            session_id: id.clone(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        frame(&output, |f| f["id"] == "deny-delete").await["error"],
        "read_only_session"
    );
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
        "observe"
    );
    let reloaded = runtime.registry.get(&id).unwrap();
    assert_eq!(reloaded.origin, origin);
    assert_eq!(*reloaded.controller.lock(), None);
    assert_eq!(
        manager.current_session_entry().unwrap().unwrap().session_id,
        native
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
    let error = handle_command(
        rival,
        ServeCommand::SwitchSession {
            id: None,
            session_id: id.clone(),
        },
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("session_in_use"));
    assert_eq!(
        crate::core::session::read_header(&history).unwrap().origin,
        origin
    );
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
async fn ask_question_panel_follows_the_controlling_protocol() {
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

    let _key = install_test_api_key();
    let (runtime, work) = shared_runtime(4);
    let host = connect_acp(&runtime);
    new_shared(&host, work.path(), "host-new").await;
    let host_slot = runtime
        .registry
        .get(&host.active_session_id().unwrap())
        .unwrap();
    assert_support(&host_slot, false).await;

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
    assert_support(&slot, false).await;

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
        assert_support(&slot, takes_control).await;
    }

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
    next_native.disconnect().await.unwrap();
    cleanup_session_slot(&next_native, &slot, SlotCleanup::Close)
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
    assert_support(&loaded, false).await;
    assert!(!loaded.resume_pending_ask_question.load(Ordering::SeqCst),
        "an initialized non-interactive controller must not leave a deferred UI flag blocking deletion");
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
async fn observer_writes_and_forged_approvals_are_rejected_without_affecting_controller() {
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
            "read_only_session"
        );
        let request = format!("stop-{index}");
        handle_command(
            b.clone(),
            ServeCommand::Interrupt {
                id: Some(request.clone()),
                session_id: requested,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            frame(&b_buffer, |f| f["id"] == request).await["error"],
            "read_only_session"
        );
    }
    assert_eq!(
        std::fs::read(slot.ctx.session_runtime.session.transcript_path(&id)).unwrap(),
        before
    );
    assert!(!slot.is_interrupted());
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
    // Exercise real EOF on the shared connection loop, not only its cleanup helper.
    super::super::stdin::run_connection_loop(
        a.clone(),
        tokio::io::BufReader::new(tokio::io::empty()),
    )
    .await
    .unwrap_or_else(|error| {
        panic!(
            "EOF cleanup failed: {error}; bash tasks: {:?}",
            slot.ctx.session_runtime.bash_task_registry.list()
        )
    });
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
            assert_eq!(opened["payload"]["access"], "observe", "{phase}: {opened}");
            assert_eq!(viewer.active_session_id().as_deref(), Some(id.as_str()));
        } else {
            assert_eq!(opened["error"], "unknown_session", "{phase}: {opened}");
            assert!(viewer.active_session_id().is_none());
            assert_eq!(runtime.registry.get(&id).is_some(), phase == "live");
        }
        assert_eq!(viewer.current_scope().unwrap().key, key);
        assert_eq!(manager.current_session_id().unwrap(), durable_current);
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
