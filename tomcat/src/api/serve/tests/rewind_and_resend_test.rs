use super::*;
use crate::core::llm::StreamEvent;
use serde_json::json;
use serial_test::serial;
use std::sync::Arc;

async fn wait_for_line(
    buffer: &crate::api::serve::test_support::SharedWriterBuffer,
    predicate: impl Fn(&serde_json::Value) -> bool,
) -> Vec<serde_json::Value> {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let lines = read_ndjson_lines(buffer);
            if lines.iter().any(&predicate) {
                return lines;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("expected frame must arrive")
}

#[tokio::test]
#[serial(env_lock)]
async fn rewind_only_reverses_keep_when_disk_returns_before_it() {
    use crate::core::checkpoint::{file_baselines, session_files};
    let _key = install_test_api_key();
    for (files, target_before_keep) in [
        (RewindFiles::Revert, true),
        (RewindFiles::Revert, false),
        (RewindFiles::Keep, true),
    ] {
        let (state, buffer, temp, slot) =
            build_initialized_state_with_streams(vec![vec![Ok(StreamEvent::FinishReason {
                reason: "stop".into(),
            })]])
            .await;
        let session = &slot.ctx.session_runtime.session;
        let first = session
            .append_message(json!({"role":"user","content":"change a"}))
            .unwrap();
        let transcript = session.transcript_path(&slot.session_id);
        let cwd = file_baselines::session_cwd(&transcript).unwrap();
        let a = temp.path().canonicalize().unwrap().join("a");
        std::fs::write(&a, "original").unwrap();
        let tracker =
            file_baselines::TurnFileBaselines::new(&transcript, &first, cwd.clone()).unwrap();
        let pending = tracker.prepare(a.to_str().unwrap()).unwrap();
        std::fs::write(&a, "accepted").unwrap();
        pending.commit();
        let first_at = chrono::DateTime::parse_from_rfc3339(
            &session.rewind_target(&first).unwrap().0.timestamp,
        )
        .unwrap()
        .timestamp_millis();
        file_baselines::timestamp_after(Some(first_at)).unwrap();
        let kept = session_files::keep(&transcript, &slot.session_id, &first).unwrap();
        let target = if target_before_keep {
            first.clone()
        } else {
            let kept_at = kept
                .source_turn_id
                .strip_prefix("keep-")
                .unwrap()
                .parse::<i64>()
                .unwrap();
            file_baselines::timestamp_after(Some(kept_at)).unwrap();
            let target = session
                .append_message(json!({"role":"user","content":"change b"}))
                .unwrap();
            let b = temp.path().canonicalize().unwrap().join("b");
            let tracker =
                file_baselines::TurnFileBaselines::new(&transcript, &target, cwd).unwrap();
            let pending = tracker.prepare(b.to_str().unwrap()).unwrap();
            std::fs::write(b, "created").unwrap();
            pending.commit();
            target
        };
        let timestamp = chrono::DateTime::parse_from_rfc3339(
            &session.rewind_target(&target).unwrap().0.timestamp,
        )
        .unwrap()
        .timestamp_millis();
        let root = file_baselines::session_dir(&transcript);
        let keep_dir = root.join(&kept.source_turn_id);
        let kept_at = kept
            .source_turn_id
            .strip_prefix("keep-")
            .unwrap()
            .parse::<i64>()
            .unwrap();
        assert_eq!(timestamp < kept_at, target_before_keep);
        handle_command(
            state,
            ServeCommand::RewindAndResend {
                id: Some("rewind-keep".into()),
                session_id: slot.session_id.clone(),
                message_id: target,
                files,
                message: RewindMessage {
                    text: "replacement".into(),
                    params: Default::default(),
                },
            },
        )
        .await
        .unwrap();
        let frames = wait_for_line(&buffer, |v| v["type"] == "agent_idle").await;
        assert!(
            frames
                .iter()
                .any(|v| v["id"] == "rewind-keep" && v["success"] == true),
            "{frames:?}"
        );
        let reverted_before = files == RewindFiles::Revert && target_before_keep;
        assert_eq!(keep_dir.exists(), !reverted_before);
        assert_eq!(
            std::fs::read_to_string(&a).unwrap(),
            if reverted_before {
                "original"
            } else {
                "accepted"
            }
        );
        assert!(session_files::list(&transcript, &slot.session_id)
            .unwrap()
            .files
            .is_empty());
        if files == RewindFiles::Keep {
            assert!(root.join(&first).exists());
        }
    }
}

#[tokio::test]
#[serial(env_lock)]
async fn rewind_and_resend_keeps_prefix_once_and_rejects_duplicate() {
    let _key = install_test_api_key();
    let (state, buffer, _temp, slot, requests) =
        build_initialized_state_with_recorded_streams(vec![vec![
            Ok(StreamEvent::ContentDelta {
                delta: "new answer".into(),
            }),
            Ok(StreamEvent::FinishReason {
                reason: "stop".into(),
            }),
        ]])
        .await;
    let session = &slot.ctx.session_runtime.session;
    session
        .append_message(json!({"role":"user","content":"prefix"}))
        .unwrap();
    session
        .append_message(json!({"role":"assistant","content":"prefix answer"}))
        .unwrap();
    let target = session
        .append_message(json!({"role":"user","content":"DISCARDED_USER"}))
        .unwrap();
    session
        .append_message(json!({"role":"assistant","content":"DISCARDED_ANSWER"}))
        .unwrap();
    session
        .append_compaction_boundary("DISCARDED_SUMMARY", None, None, 4)
        .unwrap();
    let command = ServeCommand::RewindAndResend {
        id: Some("edit".into()),
        session_id: slot.session_id.clone(),
        message_id: target,
        files: RewindFiles::Keep,
        message: RewindMessage {
            text: "replacement".into(),
            params: ServeMessageParams::default(),
        },
    };
    handle_command(state.clone(), command.clone())
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while slot.is_busy() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    handle_command(state, command).await.unwrap();
    tokio::task::yield_now().await;
    let requests = requests.0.lock();
    let texts = requests[0]
        .messages
        .iter()
        .filter_map(|m| m.text_content())
        .collect::<Vec<_>>();
    assert!(texts.contains(&"prefix"));
    assert_eq!(texts.iter().filter(|t| **t == "replacement").count(), 1);
    assert!(!texts.iter().any(|t| t.contains("DISCARDED")));
    assert!(read_ndjson_lines(&buffer)
        .iter()
        .any(|v| v["error"] == "rewind_target_stale"));
}

#[tokio::test]
#[serial(env_lock)]
async fn failed_revert_retains_queued_follow_up_until_successful_keep_supersedes_it() {
    let _key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![vec![
        Ok(StreamEvent::ContentDelta {
            delta: "replacement answer".into(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".into(),
        }),
    ]])
    .await;
    let session = &slot.ctx.session_runtime.session;
    let target = session
        .append_message(json!({"role":"user","content":"old"}))
        .unwrap();
    session
        .append_message(json!({"role":"assistant","content":"working"}))
        .unwrap();
    let queued_id = session
        .append_message(json!({"role":"user","content":"queued follow-up"}))
        .unwrap();
    let mut queued = crate::core::llm::ChatMessage::user("queued follow-up");
    queued.msg_id = Some(queued_id.clone());
    slot.ctx.session_runtime.follow_up_queue.lock().push(queued);
    slot.mark_busy();
    let worker_slot = slot.clone();
    let cancel = slot.ctx.session_runtime.cancel_token.lock().clone();
    *slot.run_task.lock() = Some(tokio::spawn(async move {
        cancel.cancelled().await;
        worker_slot.mark_idle();
    }));
    let path = session.transcript_path(&slot.session_id);
    let before = std::fs::read(&path).unwrap();
    let command = |files, id: &str| ServeCommand::RewindAndResend {
        id: Some(id.into()),
        session_id: slot.session_id.clone(),
        message_id: target.clone(),
        files,
        message: RewindMessage {
            text: "replacement".into(),
            params: ServeMessageParams::default(),
        },
    };
    handle_command(state.clone(), command(RewindFiles::Revert, "failed-revert"))
        .await
        .unwrap();
    let lines = wait_for_line(&buffer, |v| v["id"] == "failed-revert").await;
    assert!(lines.iter().any(|v| v["error"] == "revert_unavailable"));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(!slot.is_busy());
    assert_eq!(
        slot.ctx.session_runtime.follow_up_queue.lock()[0]
            .msg_id
            .as_deref(),
        Some(queued_id.as_str())
    );
    handle_command(state, command(RewindFiles::Keep, "keep-after-failure"))
        .await
        .unwrap();
    wait_for_line(&buffer, |v| v["type"] == "agent_idle").await;
    assert!(slot.ctx.session_runtime.follow_up_queue.lock().is_empty());
    assert!(slot.ctx.session_runtime.steering_queue.lock().is_empty());
    let queued_entry = std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|entry| entry["id"] == queued_id)
        .unwrap();
    assert_eq!(queued_entry["message"]["superseded"], true);
}

#[tokio::test]
#[serial(env_lock)]
async fn tool_media_rejects_retry_and_rewind_but_does_not_block_real_retry_or_remove_keep() {
    let _key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;
    let session = &slot.ctx.session_runtime.session;
    let user = session
        .append_message(json!({"role":"user","content":"real input"}))
        .unwrap();
    let carrier = session
        .append_message(json!({"role":"user","kind":"tool_media","content":[]}))
        .unwrap();
    let transcript = session.transcript_path(&slot.session_id);
    let keep = crate::core::checkpoint::file_baselines::session_dir(&transcript).join("keep-100");
    std::fs::create_dir_all(&keep).unwrap();
    handle_command(
        state.clone(),
        ServeCommand::Retry {
            id: Some("media-retry".into()),
            session_id: Some(slot.session_id.clone()),
            message_id: carrier.clone(),
        },
    )
    .await
    .unwrap();
    assert!(wait_for_line(&buffer, |v| v["id"] == "media-retry")
        .await
        .iter()
        .any(|v| v["error"] == "retry_target_stale"));
    handle_command(
        state,
        ServeCommand::RewindAndResend {
            id: Some("media-rewind".into()),
            session_id: slot.session_id.clone(),
            message_id: carrier,
            files: RewindFiles::Keep,
            message: RewindMessage {
                text: "replacement".into(),
                params: Default::default(),
            },
        },
    )
    .await
    .unwrap();
    assert!(wait_for_line(&buffer, |v| v["id"] == "media-rewind")
        .await
        .iter()
        .any(|v| v["error"] == "rewind_target_ineligible"));
    assert!(session.copy_user_message_forward(&user).is_ok());
    assert!(keep.exists(), "Retry never reverts acceptance or disk");
}

#[tokio::test]
#[serial(env_lock)]
async fn rewind_eligibility_matches_preview_for_all_message_kinds_and_queues() {
    let _key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;
    let session = &slot.ctx.session_runtime.session;
    let mut ids = Vec::new();
    for (kind, superseded, queued) in [
        ("normal", false, false),
        ("normal", false, true),
        ("steering", false, false),
        ("signal", false, false),
        ("tool_media", false, false),
        ("normal", true, false),
    ] {
        let id = session
            .append_message(
                json!({"role":"user","kind":kind,"content":"candidate","superseded":superseded}),
            )
            .unwrap();
        if queued {
            let mut input = crate::core::llm::ChatMessage::user("candidate");
            input.msg_id = Some(id.clone());
            slot.ctx.session_runtime.follow_up_queue.lock().push(input);
        }
        ids.push(id);
    }
    handle_command(
        state.clone(),
        ServeCommand::GetMessages {
            id: Some("eligibility-history".into()),
            session_id: Some(slot.session_id.clone()),
            params: GetMessagesParams {
                limit: Some(100),
                ..Default::default()
            },
        },
    )
    .await
    .unwrap();
    let history = wait_for_line(&buffer, |v| v["id"] == "eligibility-history").await;
    let payload = &history
        .iter()
        .find(|v| v["id"] == "eligibility-history")
        .unwrap()["payload"];
    for (index, id) in ids.into_iter().enumerate() {
        let eligible = payload["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["id"] == id)
            .unwrap()["message"]["rewindEligible"]
            .as_bool()
            .unwrap();
        let request_id = format!("preview-kind-{index}");
        handle_command(
            state.clone(),
            ServeCommand::PreviewRewind {
                id: Some(request_id.clone()),
                session_id: slot.session_id.clone(),
                message_id: id,
            },
        )
        .await
        .unwrap();
        let frames = wait_for_line(&buffer, |v| v["id"] == request_id).await;
        let reply = frames.iter().find(|v| v["id"] == request_id).unwrap();
        assert_eq!(reply["success"].as_bool().unwrap(), eligible, "{reply}");
    }
}

#[tokio::test]
#[serial(env_lock)]
async fn preview_rewind_without_header_cwd_has_no_baselines_not_process_fallback() {
    let _key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;
    let path = slot
        .ctx
        .session_runtime
        .session
        .transcript_path(&slot.session_id);
    let mut header = crate::core::session::transcript::read_header(&path).unwrap();
    header.cwd = None;
    let mut lines = std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    lines[0] = serde_json::to_string(&header).unwrap();
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();
    let target = slot
        .ctx
        .session_runtime
        .session
        .append_message(json!({"role":"user","content":"old"}))
        .unwrap();
    assert!(crate::core::checkpoint::file_baselines::session_cwd(&path).is_none());
    handle_command(
        state,
        ServeCommand::PreviewRewind {
            id: Some("missing-cwd".into()),
            session_id: slot.session_id.clone(),
            message_id: target,
        },
    )
    .await
    .unwrap();
    assert!(wait_for_line(&buffer, |v| v["id"] == "missing-cwd")
        .await
        .iter()
        .any(|v| v["payload"]["revertReason"] == "no_baselines"));
}

#[tokio::test]
#[serial(env_lock)]
async fn preview_rewind_is_read_only_and_rejects_queued_input() {
    let _key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;
    let target = slot
        .ctx
        .session_runtime
        .session
        .append_message(json!({"role":"user","content":"old"}))
        .unwrap();
    let path = slot
        .ctx
        .session_runtime
        .session
        .current_transcript_path()
        .unwrap()
        .unwrap();
    let before = std::fs::read(&path).unwrap();
    handle_command(
        state.clone(),
        ServeCommand::PreviewRewind {
            id: Some("preview".into()),
            session_id: slot.session_id.clone(),
            message_id: target.clone(),
        },
    )
    .await
    .unwrap();
    tokio::task::yield_now().await;
    assert_eq!(std::fs::read(path).unwrap(), before);
    assert!(!slot.ctx.session_runtime.cancel_token.lock().is_cancelled());
    assert!(read_ndjson_lines(&buffer)
        .iter()
        .any(|v| v["payload"]["revertReason"] == "no_baselines"));
    let mut queued = crate::core::llm::ChatMessage::user("queued");
    queued.msg_id = Some(target.clone());
    slot.ctx.session_runtime.follow_up_queue.lock().push(queued);
    handle_command(
        state,
        ServeCommand::PreviewRewind {
            id: Some("queued".into()),
            session_id: slot.session_id.clone(),
            message_id: target,
        },
    )
    .await
    .unwrap();
    tokio::task::yield_now().await;
    assert!(read_ndjson_lines(&buffer)
        .iter()
        .any(|v| v["error"] == "rewind_target_ineligible"));
}

#[tokio::test]
#[serial(env_lock)]
async fn rewind_command_timeout_preserves_busy_handle_history_and_files() {
    let _key = install_test_api_key();
    let (state, buffer, temp, slot) = build_initialized_state_with_streams(vec![]).await;
    let session = &slot.ctx.session_runtime.session;
    let target = session
        .append_message(json!({"role":"user","content":"old"}))
        .unwrap();
    let transcript = session.transcript_path(&slot.session_id);
    let file = temp.path().join("target.txt");
    std::fs::write(&file, "unchanged").unwrap();
    let before = std::fs::read(&transcript).unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel();
    slot.mark_busy();
    let worker_slot = slot.clone();
    *slot.run_task.lock() = Some(tokio::spawn(async move {
        rx.await.unwrap();
        worker_slot.mark_idle();
    }));
    handle_command(
        state,
        ServeCommand::RewindAndResend {
            id: Some("timeout".into()),
            session_id: slot.session_id.clone(),
            message_id: target,
            files: RewindFiles::Keep,
            message: RewindMessage {
                text: "replacement".into(),
                params: Default::default(),
            },
        },
    )
    .await
    .unwrap();
    let frames = wait_for_line(&buffer, |v| v["id"] == "timeout").await;
    assert!(frames.iter().any(|v| v["error"] == "stop_timeout"));
    assert!(slot.is_busy() && slot.run_task.lock().is_some());
    assert_eq!(std::fs::read(&transcript).unwrap(), before);
    assert_eq!(std::fs::read_to_string(file).unwrap(), "unchanged");
    tx.send(()).unwrap();
    let handle = slot.run_task.lock().take().unwrap();
    handle.await.unwrap();
}

#[tokio::test]
#[serial(env_lock)]
async fn rewind_command_barrier_restores_only_after_join_and_starts_once() {
    let _key = install_test_api_key();
    let (state, buffer, temp, slot, requests) =
        build_initialized_state_with_recorded_streams(vec![vec![
            Ok(StreamEvent::ContentDelta {
                delta: "new answer".into(),
            }),
            Ok(StreamEvent::FinishReason {
                reason: "stop".into(),
            }),
        ]])
        .await;
    let session = &slot.ctx.session_runtime.session;
    let target = session
        .append_message(json!({"role":"user","content":"old"}))
        .unwrap();
    let transcript = session.transcript_path(&slot.session_id);
    let file = temp.path().join("target.txt");
    std::fs::write(&file, "original").unwrap();
    let cwd = crate::core::checkpoint::file_baselines::session_cwd(&transcript).unwrap();
    let baseline =
        crate::core::checkpoint::file_baselines::TurnFileBaselines::new(&transcript, &target, cwd)
            .unwrap();
    let pending = baseline.prepare(file.to_str().unwrap()).unwrap();
    std::fs::write(&file, "AI change").unwrap();
    pending.commit();
    let before = std::fs::read(&transcript).unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel();
    slot.mark_busy();
    let worker = slot.clone();
    *slot.run_task.lock() = Some(tokio::spawn(async move {
        rx.await.unwrap();
        worker.mark_idle();
    }));
    let command = handle_command(
        state,
        ServeCommand::RewindAndResend {
            id: Some("barrier".into()),
            session_id: slot.session_id.clone(),
            message_id: target,
            files: RewindFiles::Revert,
            message: RewindMessage {
                text: "replacement".into(),
                params: Default::default(),
            },
        },
    );
    tokio::pin!(command);
    assert!(matches!(
        futures_util::poll!(&mut command),
        std::task::Poll::Pending
    ));
    assert_eq!(std::fs::read(&transcript).unwrap(), before);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "AI change");
    assert!(requests.0.lock().is_empty());
    tx.send(()).unwrap();
    command.await.unwrap();
    wait_for_line(&buffer, |v| v["type"] == "agent_idle").await;
    assert_eq!(std::fs::read_to_string(file).unwrap(), "original");
    assert_eq!(requests.0.lock().len(), 1);
    let frames = read_ndjson_lines(&buffer);
    assert_eq!(
        frames.iter().filter(|v| v["type"] == "agent_start").count(),
        1
    );
    let rows = std::fs::read_to_string(transcript).unwrap();
    assert_eq!(
        rows.lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .filter(|v| v["message"]["role"] == "user" && v["message"]["superseded"] != true)
            .count(),
        1
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn rewind_partial_restore_failure_keeps_history_and_can_retry() {
    let _key = install_test_api_key();
    let (state, buffer, temp, slot) = build_initialized_state_with_streams(vec![vec![
        Ok(StreamEvent::ContentDelta {
            delta: "done".into(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".into(),
        }),
    ]])
    .await;
    let session = &slot.ctx.session_runtime.session;
    let target = session
        .append_message(json!({"role":"user","content":"old"}))
        .unwrap();
    let transcript = session.transcript_path(&slot.session_id);
    let a = temp.path().join("a.txt");
    let b = temp.path().join("b.txt");
    std::fs::write(&a, "original").unwrap();
    let tracker = crate::core::checkpoint::file_baselines::TurnFileBaselines::new(
        &transcript,
        &target,
        crate::core::checkpoint::file_baselines::session_cwd(&transcript).unwrap(),
    )
    .unwrap();
    let a_pending = tracker.prepare(a.to_str().unwrap()).unwrap();
    let b_pending = tracker.prepare(b.to_str().unwrap()).unwrap();
    std::fs::write(&a, "AI change").unwrap();
    std::fs::write(&b, "created").unwrap();
    a_pending.commit();
    b_pending.commit();
    std::fs::remove_file(&b).unwrap();
    std::fs::create_dir(&b).unwrap();
    let before = std::fs::read(&transcript).unwrap();
    let make = |id: &str| ServeCommand::RewindAndResend {
        id: Some(id.into()),
        session_id: slot.session_id.clone(),
        message_id: target.clone(),
        files: RewindFiles::Revert,
        message: RewindMessage {
            text: "replacement".into(),
            params: Default::default(),
        },
    };
    handle_command(state.clone(), make("partial"))
        .await
        .unwrap();
    let frames = wait_for_line(&buffer, |v| v["id"] == "partial").await;
    let reply = frames.iter().find(|v| v["id"] == "partial").unwrap();
    assert_eq!(reply["error"], "revert_failed");
    assert_eq!(
        reply["payload"]["path"],
        b.canonicalize().unwrap().to_string_lossy().as_ref()
    );
    assert_eq!(std::fs::read(&transcript).unwrap(), before);
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "original");
    assert!(b.is_dir() && !slot.is_busy());
    std::fs::remove_dir(&b).unwrap();
    handle_command(state, make("retry-partial")).await.unwrap();
    let frames = wait_for_line(&buffer, |v| v["type"] == "agent_idle").await;
    assert!(frames
        .iter()
        .any(|v| v["id"] == "retry-partial" && v["success"] == true));
}

#[tokio::test]
#[serial(env_lock)]
async fn rewind_and_resend_waits_for_foreground_barrier_before_modifying_history() {
    let _key = install_test_api_key();
    let (state, _buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;
    let (tx, rx) = tokio::sync::oneshot::channel();
    slot.mark_busy();
    let worker_slot = Arc::clone(&slot);
    *slot.run_task.lock() = Some(tokio::spawn(async move {
        rx.await.unwrap();
        worker_slot.mark_idle();
    }));
    let stopping = super::super::cancel_and_wait(&state, &slot);
    tokio::pin!(stopping);
    assert!(matches!(
        futures_util::poll!(&mut stopping),
        std::task::Poll::Pending
    ));
    assert!(slot.is_busy());
    tx.send(()).unwrap();
    stopping.await.unwrap();
    assert!(!slot.is_busy());
}
