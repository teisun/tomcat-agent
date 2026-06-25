use super::*;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use serial_test::serial;

use crate::core::llm::{
    ChatMessageContent, ChatMessageContentPart, LlmProvider, MessageKind, StreamEvent,
};

async fn wait_for_line(
    buffer: &crate::api::serve::test_support::SharedWriterBuffer,
    predicate: impl Fn(&serde_json::Value) -> bool,
) -> Vec<serde_json::Value> {
    for _ in 0..50 {
        let lines = read_ndjson_lines(buffer);
        if lines.iter().any(&predicate) {
            return lines;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    read_ndjson_lines(buffer)
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_command_routes_by_session_id() {
    let _api_key = install_test_api_key();
    let (state, _buffer, _temp, _slot) = build_initialized_state_with_streams(vec![]).await;
    let first = state.registry.active_session_id().unwrap();

    handle_command(
        Arc::clone(&state),
        ServeCommand::NewSession {
            id: Some("n1".to_string()),
            params: NewSessionParams::default(),
        },
    )
    .await
    .unwrap();
    let sessions = state.registry.list();
    assert_eq!(sessions.len(), 2);
    let second = sessions
        .iter()
        .find(|session| session.session_id != first)
        .unwrap()
        .session_id
        .clone();

    handle_command(
        Arc::clone(&state),
        ServeCommand::SwitchSession {
            id: Some("sw1".to_string()),
            session_id: second.clone(),
        },
    )
    .await
    .unwrap();

    assert_eq!(
        state.registry.active_session_id().as_deref(),
        Some(second.as_str())
    );
    let first_slot = state.registry.get(&first).expect("first session slot");
    let second_slot = state.registry.get(&second).expect("second session slot");
    assert!(Arc::ptr_eq(
        &first_slot.ctx.agent_registry,
        &second_slot.ctx.agent_registry
    ));
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_same_session_second_prompt_is_busy() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, _slot) = build_initialized_state_with_streams(vec![]).await;
    let session_id = state.registry.active_session_id().unwrap();
    let slot = state.registry.get(&session_id).unwrap();
    slot.busy.store(true, Ordering::SeqCst);

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("p2".to_string()),
            session_id: Some(session_id.clone()),
            text: "second".to_string(),
            params: ServeMessageParams::default(),
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("p2")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("p2"))
        .unwrap();
    assert_eq!(
        response.get("success").and_then(serde_json::Value::as_bool),
        Some(false)
    );
    assert_eq!(
        response.get("error").and_then(serde_json::Value::as_str),
        Some("busy")
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_prompt_unknown_session_returns_error() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, _slot) = build_initialized_state_with_streams(vec![]).await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("unknown-1".to_string()),
            session_id: Some("missing-session".to_string()),
            text: "hello".to_string(),
            params: ServeMessageParams::default(),
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("unknown-1")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("unknown-1"))
        .unwrap();
    assert_eq!(
        response.get("error").and_then(serde_json::Value::as_str),
        Some("unknown_session")
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_new_session_rejects_when_registry_is_full() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) =
        build_initialized_state_with_streams_and_max_sessions(1, vec![]).await;
    let before_ids = slot
        .ctx
        .session_runtime
        .session
        .list_session_ids()
        .expect("list session ids before rejection");
    let before_current = slot
        .ctx
        .session_runtime
        .session
        .current_session_entry()
        .expect("read current session before rejection")
        .expect("current session should exist");

    handle_command(
        Arc::clone(&state),
        ServeCommand::NewSession {
            id: Some("full-1".to_string()),
            params: NewSessionParams::default(),
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("full-1")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("full-1"))
        .unwrap();
    assert_eq!(
        response.get("error").and_then(serde_json::Value::as_str),
        Some("too_many_sessions")
    );
    let after_ids = slot
        .ctx
        .session_runtime
        .session
        .list_session_ids()
        .expect("list session ids after rejection");
    let after_current = slot
        .ctx
        .session_runtime
        .session
        .current_session_entry()
        .expect("read current session after rejection")
        .expect("current session should still exist");
    assert_eq!(state.registry.len(), 1, "registry should remain unchanged");
    assert_eq!(after_ids, before_ids, "rejected new_session must not create transcript files");
    assert_eq!(
        after_current.session_id, before_current.session_id,
        "rejected new_session must not repoint current session"
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_prompt_drives_agent_run() {
    let _api_key = install_test_api_key();
    let stream = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "hello".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
        Ok(StreamEvent::Usage {
            prompt_tokens: 1,
            completion_tokens: 1,
            total_tokens: Some(2),
        }),
    ];
    let (state, buffer, _temp, _slot) = build_initialized_state_with_streams(vec![stream]).await;
    let session_id = state.registry.active_session_id().unwrap();

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("p1".to_string()),
            session_id: Some(session_id.clone()),
            text: "say hello".to_string(),
            params: ServeMessageParams::default(),
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
    })
    .await;
    assert!(
        lines.iter().any(|line| {
            line.get("id").and_then(serde_json::Value::as_str) == Some("p1")
                && line.get("success").and_then(serde_json::Value::as_bool) == Some(true)
        }),
        "expected prompt acceptance response, got {lines:?}"
    );
    assert!(
        lines.iter().any(|line| {
            line.get("type").and_then(serde_json::Value::as_str) == Some("agent_start")
                && line.get("sessionId").and_then(serde_json::Value::as_str)
                    == Some(session_id.as_str())
        }),
        "expected agent_start, got {lines:?}"
    );
    assert!(
        lines.iter().any(|line| {
            line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
                && line.get("sessionId").and_then(serde_json::Value::as_str)
                    == Some(session_id.as_str())
        }),
        "expected agent_end, got {lines:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
#[serial(env_lock)]
async fn serve_prompt_installs_fresh_cancel_token_before_spawned_turn_runs() {
    let _api_key = install_test_api_key();
    let (state, _buffer, _temp, slot) = build_initialized_state_with_streams(vec![vec![]]).await;
    let previous = slot.ctx.session_runtime.cancel_token.lock().clone();

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("replace-token".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "hello".to_string(),
            params: ServeMessageParams::default(),
        },
    )
    .await
    .unwrap();

    let current = slot.ctx.session_runtime.cancel_token.lock().clone();
    previous.cancel();
    assert!(
        !current.is_cancelled(),
        "accepted prompt should replace the prior cancel token before the spawned turn observes it"
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_prompt_with_image_attachment_builds_multimodal_message() {
    let _api_key = install_test_api_key();
    let stream = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "vision ok".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let (state, buffer, _temp, slot, requests) =
        build_initialized_state_with_recorded_streams(vec![stream]).await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("img-1".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "describe this".to_string(),
            params: ServeMessageParams {
                attachments: vec![ServeAttachment {
                    kind: ServeAttachmentKind::Image,
                    mime_type: None,
                    data_base64: None,
                    file_id: Some("file-vision".to_string()),
                }],
            },
        },
    )
    .await
    .unwrap();

    let _ = wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
    })
    .await;

    let captured = requests.0.lock();
    assert_eq!(captured.len(), 1, "expected exactly one LLM request");
    let user_message = captured[0]
        .messages
        .iter()
        .rev()
        .find(|message| matches!(message.role, crate::core::llm::ChatMessageRole::User))
        .expect("user message");
    let Some(ChatMessageContent::Parts(parts)) = &user_message.content else {
        panic!(
            "expected multimodal parts user message, got {:?}",
            user_message.content
        );
    };
    assert_eq!(parts.len(), 2, "expected text + image parts");
    assert!(matches!(
        &parts[0],
        ChatMessageContentPart::InputText { text } if text == "describe this"
    ));
    assert!(matches!(
        &parts[1],
        ChatMessageContentPart::InputImage {
            file_id: Some(file_id),
            data: None,
            ..
        } if file_id == "file-vision"
    ));
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_follow_up_with_attachment_queues_multimodal_message_when_busy() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;
    slot.busy.store(true, Ordering::SeqCst);

    handle_command(
        Arc::clone(&state),
        ServeCommand::FollowUp {
            id: Some("fu-1".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "look at this too".to_string(),
            params: ServeMessageParams {
                attachments: vec![ServeAttachment {
                    kind: ServeAttachmentKind::Image,
                    mime_type: None,
                    data_base64: None,
                    file_id: Some("file-follow-up".to_string()),
                }],
            },
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("fu-1")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("fu-1"))
        .expect("queued follow_up response");
    assert_eq!(response["payload"]["queued"].as_bool(), Some(true));

    let queue = slot.ctx.session_runtime.follow_up_queue.lock();
    assert_eq!(queue.len(), 1, "expected one queued follow_up");
    let Some(ChatMessageContent::Parts(parts)) = &queue[0].content else {
        panic!(
            "expected queued multimodal follow_up, got {:?}",
            queue[0].content
        );
    };
    assert!(matches!(
        &parts[1],
        ChatMessageContentPart::InputImage {
            file_id: Some(file_id),
            data: None,
            ..
        } if file_id == "file-follow-up"
    ));
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_prompt_invalid_attachment_returns_error() {
    let _api_key = install_test_api_key();
    let stream = vec![Ok(StreamEvent::FinishReason {
        reason: "stop".to_string(),
    })];
    let (state, buffer, _temp, slot, requests) =
        build_initialized_state_with_recorded_streams(vec![stream]).await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("bad-attachment".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "bad".to_string(),
            params: ServeMessageParams {
                attachments: vec![ServeAttachment {
                    kind: ServeAttachmentKind::Image,
                    mime_type: None,
                    data_base64: Some("Zm9v".to_string()),
                    file_id: None,
                }],
            },
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("bad-attachment")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| {
            line.get("id").and_then(serde_json::Value::as_str) == Some("bad-attachment")
        })
        .expect("invalid attachment response");
    assert_eq!(response["success"].as_bool(), Some(false));
    assert_eq!(
        response["error"].as_str(),
        Some("invalid_attachment: image attachment requires mimeType")
    );
    assert!(
        requests.0.lock().is_empty(),
        "invalid attachment should not reach LLM"
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_prompt_without_attachments_falls_back_to_user_text() {
    let _api_key = install_test_api_key();
    let stream = vec![Ok(StreamEvent::FinishReason {
        reason: "stop".to_string(),
    })];
    let (state, buffer, _temp, slot, requests) =
        build_initialized_state_with_recorded_streams(vec![stream]).await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("plain-1".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "plain text".to_string(),
            params: ServeMessageParams::default(),
        },
    )
    .await
    .unwrap();

    let _ = wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
    })
    .await;

    let captured = requests.0.lock();
    let user_message = captured[0]
        .messages
        .iter()
        .rev()
        .find(|message| matches!(message.role, crate::core::llm::ChatMessageRole::User))
        .expect("user message");
    assert!(matches!(
        &user_message.content,
        Some(ChatMessageContent::Text(text)) if text == "plain text"
    ));
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_steer_ignores_attachments() {
    let _api_key = install_test_api_key();
    let stream = vec![Ok(StreamEvent::FinishReason {
        reason: "stop".to_string(),
    })];
    let (state, buffer, _temp, slot, requests) =
        build_initialized_state_with_recorded_streams(vec![stream]).await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::Steer {
            id: Some("steer-1".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "just steer".to_string(),
            params: ServeMessageParams {
                attachments: vec![ServeAttachment {
                    kind: ServeAttachmentKind::Image,
                    mime_type: None,
                    data_base64: None,
                    file_id: Some("ignored-file".to_string()),
                }],
            },
        },
    )
    .await
    .unwrap();

    let _ = wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
    })
    .await;

    let captured = requests.0.lock();
    let steering_message = captured[0]
        .messages
        .iter()
        .rev()
        .find(|message| message.kind == MessageKind::Steering)
        .expect("steering message");
    assert!(matches!(
        &steering_message.content,
        Some(ChatMessageContent::Text(text)) if text == "just steer"
    ));
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_get_messages_uptoseq_is_null_placeholder() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::GetMessages {
            id: Some("gm-1".to_string()),
            session_id: Some(slot.session_id.clone()),
            params: GetMessagesParams::default(),
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("gm-1")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("gm-1"))
        .expect("get_messages response");
    assert!(response["payload"].get("upToSeq").is_some());
    assert!(response["payload"]["upToSeq"].is_null());
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_prompt_panic_isolation_emits_agent_end_error() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_panicking_provider().await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("panic-1".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "panic".to_string(),
            params: ServeMessageParams::default(),
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
            && line.get("error").and_then(serde_json::Value::as_str)
                == Some("serve session task panicked")
    })
    .await;
    assert!(
        lines.iter().any(|line| {
            line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
                && line.get("error").and_then(serde_json::Value::as_str)
                    == Some("serve session task panicked")
                && line.get("sessionId").and_then(serde_json::Value::as_str)
                    == Some(slot.session_id.as_str())
        }),
        "expected panic-isolated agent_end, got {lines:?}"
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_prompt_with_stale_invalid_model_override_emits_single_agent_end_and_recovers() {
    let _api_key = install_test_api_key();
    let stream = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "recovered".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![stream]).await;

    slot.ctx
        .session_runtime
        .session
        .switch_current_model(None, Some("totally-missing-model"))
        .expect("seed stale model override");

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("invalid-override-1".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "hello".to_string(),
            params: ServeMessageParams::default(),
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
            && line.get("error").and_then(serde_json::Value::as_str).is_some()
    })
    .await;
    let error_ends = lines
        .iter()
        .filter(|line| {
            line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
                && line.get("error").and_then(serde_json::Value::as_str).is_some()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        error_ends.len(),
        1,
        "invalid stale model override should emit exactly one terminal error event: {lines:?}"
    );
    assert!(
        error_ends[0]
            .get("error")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|message| message.contains("totally-missing-model")),
        "expected stale model error to mention the invalid override: {lines:?}"
    );
    assert!(
        slot.turn_state.lock().is_some(),
        "turn_state should be restored after pre-loop resolve failure"
    );
    for _ in 0..50 {
        if !slot.is_busy() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    slot.ctx
        .session_runtime
        .session
        .switch_current_model(None, Some("gpt-5.4"))
        .expect("restore valid model");
    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("invalid-override-2".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "recover".to_string(),
            params: ServeMessageParams::default(),
        },
    )
    .await
    .unwrap();
    let after_recovery_prompt = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("invalid-override-2")
    })
    .await;
    let recovery_response = after_recovery_prompt
        .iter()
        .find(|line| {
            line.get("id").and_then(serde_json::Value::as_str) == Some("invalid-override-2")
        })
        .expect("recovery prompt response");
    assert_eq!(
        recovery_response["success"].as_bool(),
        Some(true),
        "recovery prompt should still be accepted: {after_recovery_prompt:?}"
    );
    let recovered = {
        let mut lines = read_ndjson_lines(&buffer);
        for _ in 0..50 {
            lines = read_ndjson_lines(&buffer);
            if lines
                .iter()
                .filter(|line| {
                    line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
                })
                .count()
                >= 2
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        lines
    };
    let all_agent_ends = recovered
        .iter()
        .filter(|line| line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end"))
        .count();
    assert_eq!(
        all_agent_ends, 2,
        "expected one failed + one recovered terminal event: {recovered:?}"
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_prompt_with_attachment_history_then_deepseek_emits_single_agent_end_and_recovers() {
    let _api_key = install_test_api_key();
    let first_stream = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "vision ok".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let second_stream = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "back on gpt".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let (state, buffer, _temp, slot) =
        build_initialized_state_with_streams(vec![first_stream, second_stream]).await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("attachment-history-1".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "describe image".to_string(),
            params: ServeMessageParams {
                attachments: vec![ServeAttachment {
                    kind: ServeAttachmentKind::Image,
                    mime_type: None,
                    data_base64: None,
                    file_id: Some("file-vision".to_string()),
                }],
            },
        },
    )
    .await
    .unwrap();
    let after_first = wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
            && line.get("error").and_then(serde_json::Value::as_str).is_none()
    })
    .await;
    assert_eq!(
        after_first
            .iter()
            .filter(|line| line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end"))
            .count(),
        1
    );

    handle_command(
        Arc::clone(&state),
        ServeCommand::SetModel {
            id: Some("set-deepseek".to_string()),
            session_id: Some(slot.session_id.clone()),
            model: "deepseek-v4-pro".to_string(),
        },
    )
    .await
    .unwrap();
    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("attachment-history-2".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "follow up".to_string(),
            params: ServeMessageParams::default(),
        },
    )
    .await
    .unwrap();

    let deepseek_failure = wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
            && line.get("error").and_then(serde_json::Value::as_str).is_some()
    })
    .await;
    let deepseek_errors = deepseek_failure
        .iter()
        .filter(|line| {
            line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
                && line.get("error").and_then(serde_json::Value::as_str).is_some()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        deepseek_errors.len(),
        1,
        "capability mismatch should emit exactly one terminal error event: {deepseek_failure:?}"
    );
    let error_text = deepseek_errors[0]
        .get("error")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    assert!(
        error_text.contains("provider/model 不支持"),
        "expected capability mismatch error, got {error_text:?}"
    );
    assert!(
        slot.turn_state.lock().is_some(),
        "turn_state should be restored after capability validation failure"
    );
    for _ in 0..50 {
        if !slot.is_busy() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    handle_command(
        Arc::clone(&state),
        ServeCommand::SetModel {
            id: Some("set-back-gpt".to_string()),
            session_id: Some(slot.session_id.clone()),
            model: "gpt-5.4".to_string(),
        },
    )
    .await
    .unwrap();
    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("attachment-history-3".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "recover".to_string(),
            params: ServeMessageParams::default(),
        },
    )
    .await
    .unwrap();
    let after_recovery_prompt = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("attachment-history-3")
    })
    .await;
    let recovery_response = after_recovery_prompt
        .iter()
        .find(|line| {
            line.get("id").and_then(serde_json::Value::as_str) == Some("attachment-history-3")
        })
        .expect("recovery prompt response");
    assert_eq!(
        recovery_response["success"].as_bool(),
        Some(true),
        "recovery prompt should still be accepted: {after_recovery_prompt:?}"
    );
    let recovered = {
        let mut lines = read_ndjson_lines(&buffer);
        for _ in 0..50 {
            lines = read_ndjson_lines(&buffer);
            if lines
                .iter()
                .filter(|line| {
                    line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
                })
                .count()
                >= 3
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        lines
    };
    let agent_end_total = recovered
        .iter()
        .filter(|line| line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end"))
        .count();
    let agent_end_error_total = recovered
        .iter()
        .filter(|line| {
            line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
                && line.get("error").and_then(serde_json::Value::as_str).is_some()
        })
        .count();
    assert_eq!(
        agent_end_total, 3,
        "expected success + failure + recovery agent_end events: {recovered:?}"
    );
    assert_eq!(
        agent_end_error_total, 1,
        "only the deepseek capability mismatch turn should fail"
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_set_model_rejects_invalid_id_without_mutating_session_override() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::SetModel {
            id: Some("bad-model".to_string()),
            session_id: Some(slot.session_id.clone()),
            model: "deepseek".to_string(),
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("bad-model")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("bad-model"))
        .expect("invalid model response");
    assert_eq!(response["success"].as_bool(), Some(false));
    assert!(
        response["error"]
            .as_str()
            .is_some_and(|message| message.contains("deepseek")),
        "expected invalid model error to mention requested id: {response:?}"
    );

    let current = slot
        .ctx
        .session_runtime
        .session
        .current_session_entry()
        .expect("read current session")
        .expect("current session entry");
    assert_eq!(
        current.model_override, None,
        "invalid set_model must not persist a bad model_override"
    );

    handle_command(
        Arc::clone(&state),
        ServeCommand::ListModels {
            id: Some("models-after-bad-set".to_string()),
        },
    )
    .await
    .unwrap();
    let after = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("models-after-bad-set")
    })
    .await;
    let list_models = after
        .iter()
        .find(|line| {
            line.get("id").and_then(serde_json::Value::as_str) == Some("models-after-bad-set")
        })
        .expect("list_models response after invalid set_model");
    assert_eq!(list_models["success"].as_bool(), Some(true));
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_set_thinking_level_roundtrips_in_get_state() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::SetThinkingLevel {
            id: Some("effort-1".to_string()),
            session_id: Some(slot.session_id.clone()),
            model: "gpt-5.4".to_string(),
            level: "xhigh".to_string(),
        },
    )
    .await
    .unwrap();
    handle_command(
        Arc::clone(&state),
        ServeCommand::GetState {
            id: Some("state-effort-1".to_string()),
            session_id: Some(slot.session_id.clone()),
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("state-effort-1")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("state-effort-1"))
        .expect("get_state response");
    let payload = response["payload"].clone();
    assert_eq!(response["success"].as_bool(), Some(true));
    assert_eq!(payload["model"].as_str(), Some("gpt-5.4"));
    assert_eq!(payload["thinkingLevel"].as_str(), Some("xhigh"));
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_get_state_tracks_per_model_thinking_level_after_switching_models() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;

    for command in [
        ServeCommand::SetThinkingLevel {
            id: Some("effort-gpt".to_string()),
            session_id: Some(slot.session_id.clone()),
            model: "gpt-5.4".to_string(),
            level: "low".to_string(),
        },
        ServeCommand::SetThinkingLevel {
            id: Some("effort-deepseek".to_string()),
            session_id: Some(slot.session_id.clone()),
            model: "deepseek-v4-pro".to_string(),
            level: "xhigh".to_string(),
        },
        ServeCommand::SetModel {
            id: Some("switch-deepseek".to_string()),
            session_id: Some(slot.session_id.clone()),
            model: "deepseek-v4-pro".to_string(),
        },
        ServeCommand::GetState {
            id: Some("state-deepseek".to_string()),
            session_id: Some(slot.session_id.clone()),
        },
        ServeCommand::SetModel {
            id: Some("switch-gpt".to_string()),
            session_id: Some(slot.session_id.clone()),
            model: "gpt-5.4".to_string(),
        },
        ServeCommand::GetState {
            id: Some("state-gpt".to_string()),
            session_id: Some(slot.session_id.clone()),
        },
    ] {
        handle_command(Arc::clone(&state), command).await.unwrap();
    }

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("state-gpt")
    })
    .await;
    let deepseek = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("state-deepseek"))
        .expect("deepseek get_state");
    let gpt = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("state-gpt"))
        .expect("gpt get_state");

    assert_eq!(deepseek["payload"]["model"].as_str(), Some("deepseek-v4-pro"));
    assert_eq!(deepseek["payload"]["thinkingLevel"].as_str(), Some("xhigh"));
    assert_eq!(gpt["payload"]["model"].as_str(), Some("gpt-5.4"));
    assert_eq!(gpt["payload"]["thinkingLevel"].as_str(), Some("low"));
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_set_thinking_level_invalid_value_returns_error_without_breaking_loop() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::SetThinkingLevel {
            id: Some("bad-effort".to_string()),
            session_id: Some(slot.session_id.clone()),
            model: "gpt-5.4".to_string(),
            level: "turbo".to_string(),
        },
    )
    .await
    .unwrap();
    handle_command(
        Arc::clone(&state),
        ServeCommand::GetState {
            id: Some("state-after-bad-effort".to_string()),
            session_id: Some(slot.session_id.clone()),
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("state-after-bad-effort")
    })
    .await;
    let error = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("bad-effort"))
        .expect("bad effort response");
    let state_after = lines
        .iter()
        .find(|line| {
            line.get("id").and_then(serde_json::Value::as_str) == Some("state-after-bad-effort")
        })
        .expect("follow-up get_state response");

    assert_eq!(error["success"].as_bool(), Some(false));
    assert_eq!(error["error"].as_str(), Some("invalid_thinking_level"));
    assert_eq!(state_after["success"].as_bool(), Some(true));
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_set_thinking_level_persists_across_restart() {
    let _api_key = install_test_api_key();
    let temp = tempfile::tempdir().expect("tempdir");
    let cfg = serve_test_config(temp.path(), "http://127.0.0.1:1");
    let provider: Arc<dyn LlmProvider> = Arc::new(DeterministicMockLlm::new(vec![]));
    let (state, _buffer, temp, slot) = build_initialized_state_with_provider(temp, cfg.clone(), provider).await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::SetThinkingLevel {
            id: Some("persist-effort".to_string()),
            session_id: Some(slot.session_id.clone()),
            model: "gpt-5.4".to_string(),
            level: "xhigh".to_string(),
        },
    )
    .await
    .unwrap();

    drop(slot);
    drop(state);

    let provider: Arc<dyn LlmProvider> = Arc::new(DeterministicMockLlm::new(vec![]));
    let (state, buffer, _temp, slot) =
        build_initialized_state_with_provider(temp, cfg, provider).await;
    handle_command(
        Arc::clone(&state),
        ServeCommand::GetState {
            id: Some("state-after-restart".to_string()),
            session_id: Some(slot.session_id.clone()),
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("state-after-restart")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| {
            line.get("id").and_then(serde_json::Value::as_str) == Some("state-after-restart")
        })
        .expect("get_state after restart");
    assert_eq!(response["payload"]["thinkingLevel"].as_str(), Some("xhigh"));
}

#[test]
fn build_shared_model_thinking_uses_global_store_path() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut cfg = serve_test_config(temp.path(), "http://127.0.0.1:1");
    cfg.llm.thinking.level = "medium".to_string();
    ensure_work_dir_structure(&cfg).expect("work dir");

    let store = build_shared_model_thinking(&cfg).expect("shared model thinking");
    let global_path = crate::resolve_model_thinking_path(&cfg).expect("global model thinking path");

    assert_eq!(store.get("gpt-5.4"), crate::core::llm::ThinkingLevel::Medium);
    assert!(global_path.exists(), "global model thinking store should be created");

    let persisted = std::fs::read_to_string(&global_path).expect("read global model thinking store");
    let parsed: serde_json::Value =
        serde_json::from_str(&persisted).expect("parse global model thinking store");
    assert_eq!(parsed["models"].as_object().map(|models| models.len()), Some(0));
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_prompt_passes_per_model_thinking_level_to_main_loop_request() {
    let _api_key = install_test_api_key();
    let stream = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "hello".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let (state, buffer, _temp, slot, requests) =
        build_initialized_state_with_recorded_streams(vec![stream]).await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::SetThinkingLevel {
            id: Some("prompt-effort".to_string()),
            session_id: Some(slot.session_id.clone()),
            model: "gpt-5.4".to_string(),
            level: "xhigh".to_string(),
        },
    )
    .await
    .unwrap();
    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("prompt-with-effort".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "say hello".to_string(),
            params: ServeMessageParams::default(),
        },
    )
    .await
    .unwrap();
    let _ = wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
    })
    .await;

    let captured = requests.0.lock();
    assert_eq!(captured.len(), 1, "expected exactly one recorded LLM request");
    assert_eq!(captured[0].thinking_level, Some(crate::core::llm::ThinkingLevel::Xhigh));
}
