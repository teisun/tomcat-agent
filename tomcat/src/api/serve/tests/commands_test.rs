use super::*;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use serial_test::serial;

use crate::core::llm::{ChatMessageContent, ChatMessageContentPart, MessageKind, StreamEvent};

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
