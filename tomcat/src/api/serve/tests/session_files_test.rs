use super::*;
use serial_test::serial;

async fn file_response(buffer: &SharedWriterBuffer, id: &str) -> serde_json::Value {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let Some(frame) = read_ndjson_lines(buffer)
                .into_iter()
                .find(|v| v["id"] == id)
            {
                return frame;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
#[serial(env_lock)]
async fn session_file_rpc_busy_read_restore_and_chat_preservation() {
    let _key = EnvGuard::set(TEST_API_KEY_ENV, "stub");
    let (state, buffer, temp, slot) = build_initialized_state_with_streams(vec![]).await;
    let session = &slot.ctx.session_runtime.session;
    let owner = session
        .append_message(serde_json::json!({"role":"user","content":"edit"}))
        .unwrap();
    let transcript = session.transcript_path(&slot.session_id);
    let path = temp.path().canonicalize().unwrap().join("target.txt");
    std::fs::write(&path, "before").unwrap();
    let cwd = crate::core::checkpoint::file_baselines::session_cwd(&transcript).unwrap();
    let tracker =
        crate::core::checkpoint::file_baselines::TurnFileBaselines::new(&transcript, &owner, cwd)
            .unwrap();
    let pending = tracker.prepare(path.to_str().unwrap()).unwrap();
    std::fs::write(&path, "after").unwrap();
    pending.commit();
    let before = std::fs::read(&transcript).unwrap();
    slot.mark_busy();
    handle_command(
        state.clone(),
        ServeCommand::GetSessionFiles {
            id: Some("list".into()),
            session_id: slot.session_id.clone(),
        },
    )
    .await
    .unwrap();
    let response = file_response(&buffer, "list").await;
    assert_eq!(response["success"], true);
    assert_eq!(response["payload"]["sourceTurnId"], owner);
    let paths = vec![path.to_string_lossy().into_owned()];
    let command = |id: &str| ServeCommand::RestoreSessionFiles {
        id: Some(id.into()),
        session_id: slot.session_id.clone(),
        source_turn_id: owner.clone(),
        paths: paths.clone(),
    };
    handle_command(state.clone(), command("busy"))
        .await
        .unwrap();
    assert_eq!(file_response(&buffer, "busy").await["error"], "busy");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "after");
    slot.mark_idle();
    handle_command(
        state.clone(),
        ServeCommand::GetSessionFileBaseline {
            id: Some("base".into()),
            session_id: slot.session_id.clone(),
            source_turn_id: owner.clone(),
            path: paths[0].clone(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        file_response(&buffer, "base").await["payload"]["text"],
        "before"
    );
    handle_command(state, command("restore")).await.unwrap();
    let response = file_response(&buffer, "restore").await;
    assert_eq!(response["success"], true, "{response}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "before");
    assert_eq!(std::fs::read(&transcript).unwrap(), before);
    assert!(read_ndjson_lines(&buffer)
        .iter()
        .all(|v| v["type"] != "agent_start"));
}

#[tokio::test]
#[serial(env_lock)]
async fn session_file_real_history_rewrite_reselects_source() {
    use crate::core::checkpoint::file_baselines::{session_cwd, session_dir, TurnFileBaselines};
    let _key = EnvGuard::set(TEST_API_KEY_ENV, "stub");
    let (state, buffer, temp, slot) = build_initialized_state_with_streams(vec![]).await;
    let session = &slot.ctx.session_runtime.session;
    let transcript = session.transcript_path(&slot.session_id);
    let root = session_dir(&transcript);
    let publish = |owner: &str, name: &str, tick: u64| {
        let path = temp.path().canonicalize().unwrap().join(name);
        let tracker =
            TurnFileBaselines::new(&transcript, owner, session_cwd(&transcript).unwrap()).unwrap();
        let pending = tracker.prepare(path.to_str().unwrap()).unwrap();
        std::fs::write(&path, "changed").unwrap();
        pending.commit();
        std::fs::File::open(root.join(owner).join("baselines.jsonl"))
            .unwrap()
            .set_modified(
                std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000 + tick),
            )
            .unwrap();
        path.to_string_lossy().into_owned()
    };
    let u1 = session
        .append_message(serde_json::json!({"role":"user","content":"change a"}))
        .unwrap();
    let a = publish(&u1, "a.txt", 1);
    let anchor = session
        .append_message(serde_json::json!({"role":"assistant","content":"a changed"}))
        .unwrap();
    let u2 = session
        .append_message(serde_json::json!({"role":"user","content":"change b"}))
        .unwrap();
    let b = publish(&u2, "b.txt", 2);
    session
        .append_message(
            serde_json::json!({"role":"assistant","content":"partial b result","turn_failed":true}),
        )
        .unwrap();

    handle_command(
        state.clone(),
        ServeCommand::GetSessionFiles {
            id: Some("before-retry".into()),
            session_id: slot.session_id.clone(),
        },
    )
    .await
    .unwrap();
    let before = file_response(&buffer, "before-retry").await;
    assert_eq!(before["payload"]["sourceTurnId"], u2);
    assert_eq!(before["payload"]["files"][0]["path"], b);

    // The production Retry implementation: copy forward archives u2 and cleans its backups.
    let retry = session.copy_user_message_forward(&u2).unwrap();
    assert!(!root.join(&u2).exists());
    assert!(root.join(&u1).exists());
    assert!(!root.join(&retry).join("baselines.jsonl").exists());
    handle_command(
        state.clone(),
        ServeCommand::GetSessionFiles {
            id: Some("after-retry".into()),
            session_id: slot.session_id.clone(),
        },
    )
    .await
    .unwrap();
    let after = file_response(&buffer, "after-retry").await;
    assert_eq!(after["payload"]["sourceTurnId"], u1);
    assert_eq!(after["payload"]["files"].as_array().unwrap().len(), 1);
    assert_eq!(after["payload"]["files"][0]["path"], a);

    session
        .append_message(serde_json::json!({"role":"assistant","content":"retried without edits"}))
        .unwrap();
    let u3 = session
        .append_message(serde_json::json!({"role":"user","content":"change c"}))
        .unwrap();
    let c = publish(&u3, "c.txt", 3);
    session
        .append_message(serde_json::json!({"role":"assistant","content":"c changed"}))
        .unwrap();
    handle_command(
        state.clone(),
        ServeCommand::GetSessionFiles {
            id: Some("before-checkpoint".into()),
            session_id: slot.session_id.clone(),
        },
    )
    .await
    .unwrap();
    let before = file_response(&buffer, "before-checkpoint").await;
    assert_eq!(before["payload"]["sourceTurnId"], u3);
    assert_eq!(before["payload"]["files"][0]["path"], c);

    // These are the real checkpoint finalization operations, not handmade superseded JSON.
    session
        .mark_messages_after_anchor_superseded(&anchor)
        .unwrap();
    session.cleanup_superseded_file_baselines();
    assert!(!root.join(&u3).exists());
    assert!(root.join(&u1).exists());
    handle_command(
        state.clone(),
        ServeCommand::GetSessionFiles {
            id: Some("after-checkpoint".into()),
            session_id: slot.session_id.clone(),
        },
    )
    .await
    .unwrap();
    let after = file_response(&buffer, "after-checkpoint").await;
    assert_eq!(after["payload"]["sourceTurnId"], u1);
    assert_eq!(after["payload"]["files"][0]["path"], a);

    // Editing the first input must discard its directory and all remaining live suffix turns.
    session
        .rewind_user_message(
            &u1,
            serde_json::json!({"role":"user","content":"edited request"}),
        )
        .unwrap();
    assert!(!root.join(&u1).exists());
    handle_command(
        state,
        ServeCommand::GetSessionFiles {
            id: Some("after-edit".into()),
            session_id: slot.session_id.clone(),
        },
    )
    .await
    .unwrap();
    let after = file_response(&buffer, "after-edit").await;
    assert_eq!(after["payload"]["sourceTurnId"], serde_json::Value::Null);
    assert_eq!(after["payload"]["files"], serde_json::json!([]));
    // History rewrites, not Files queries, changed history; none of these metadata reads wrote files.
    assert_eq!(std::fs::read_to_string(a).unwrap(), "changed");
    assert_eq!(std::fs::read_to_string(b).unwrap(), "changed");
    assert_eq!(std::fs::read_to_string(c).unwrap(), "changed");
    assert!(read_ndjson_lines(&buffer)
        .iter()
        .all(|v| v["type"] != "agent_start"));
}

#[test]
fn session_file_schema_and_command_ids_are_exported() {
    let dts = serve_dts();
    for name in [
        "SessionFilesResponse",
        "SessionFileBaselineResponse",
        "SessionFilesRestoreResponse",
    ] {
        assert!(dts.contains(name));
    }
    let command: ServeCommand = serde_json::from_value(
        serde_json::json!({"type":"get_session_files","id":"f","sessionId":"s"}),
    )
    .unwrap();
    assert_eq!(command.command_id(), Some("f"));
    assert_eq!(command.session_id(), Some("s"));
    assert_eq!(command.wire_type(), "get_session_files");
}
