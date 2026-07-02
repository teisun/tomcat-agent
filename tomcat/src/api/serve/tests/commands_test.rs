use super::*;
use base64::Engine as _;
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

fn append_history_message(
    slot: &Arc<crate::api::serve::SessionSlot>,
    role: &str,
    content: &str,
) -> String {
    slot.ctx
        .session_runtime
        .session
        .try_append_message_to_session(
            &slot.session_id,
            serde_json::json!({
                "role": role,
                "content": content,
            }),
        )
        .expect("append history message")
}

fn payload_message_ids(response: &serde_json::Value) -> Vec<String> {
    response["payload"]["messages"]
        .as_array()
        .expect("messages array")
        .iter()
        .filter_map(|entry| entry.get("id").and_then(serde_json::Value::as_str))
        .map(ToString::to_string)
        .collect()
}

fn decode_cursor(cursor: &str) -> serde_json::Value {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(cursor.as_bytes())
        .expect("decode cursor");
    serde_json::from_slice(&bytes).expect("parse cursor json")
}

fn session_message_entries(
    slot: &Arc<crate::api::serve::SessionSlot>,
) -> Vec<crate::core::session::transcript::MessageEntry> {
    slot.ctx
        .session_runtime
        .session
        .get_entries_for_session(&slot.session_id, 256)
        .expect("read session entries")
        .into_iter()
        .filter_map(|entry| match entry {
            crate::core::session::transcript::TranscriptEntry::Message(message) => Some(message),
            _ => None,
        })
        .collect()
}

fn count_message_entries_with_id(
    slot: &Arc<crate::api::serve::SessionSlot>,
    message_id: &str,
) -> usize {
    session_message_entries(slot)
        .into_iter()
        .filter(|entry| entry.id.as_deref() == Some(message_id))
        .count()
}

fn latest_user_entry(
    slot: &Arc<crate::api::serve::SessionSlot>,
) -> crate::core::session::transcript::MessageEntry {
    session_message_entries(slot)
        .into_iter()
        .rev()
        .find(|entry| entry.message.get("role").and_then(serde_json::Value::as_str) == Some("user"))
        .expect("latest user entry")
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
async fn serve_prompt_emits_assistant_message_id_on_stream_and_turn_end() {
    let _api_key = install_test_api_key();
    let stream = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "hello stable id".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![stream]).await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("prompt-stable-id".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "hello".to_string(),
            params: ServeMessageParams::default(),
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
    })
    .await;

    let message_start_id = lines
        .iter()
        .find(|line| line.get("type").and_then(serde_json::Value::as_str) == Some("message_start"))
        .and_then(|line| line.get("assistantMessageId"))
        .and_then(serde_json::Value::as_str)
        .expect("message_start assistantMessageId");
    let message_update_id = lines
        .iter()
        .find(|line| line.get("type").and_then(serde_json::Value::as_str) == Some("message_update"))
        .and_then(|line| line.get("assistantMessageId"))
        .and_then(serde_json::Value::as_str)
        .expect("message_update assistantMessageId");
    let message_end_id = lines
        .iter()
        .find(|line| line.get("type").and_then(serde_json::Value::as_str) == Some("message_end"))
        .and_then(|line| line.get("assistantMessageId"))
        .and_then(serde_json::Value::as_str)
        .expect("message_end assistantMessageId");
    let turn_end_id = lines
        .iter()
        .find(|line| line.get("type").and_then(serde_json::Value::as_str) == Some("turn_end"))
        .and_then(|line| line.get("assistantMessageId"))
        .and_then(serde_json::Value::as_str)
        .expect("turn_end assistantMessageId");

    assert_eq!(message_update_id, message_start_id);
    assert_eq!(message_end_id, message_start_id);
    assert_eq!(turn_end_id, message_start_id);
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
                ..ServeMessageParams::default()
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
                user_message_id: Some("follow-up-fixed-id".to_string()),
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
    assert_eq!(queue[0].msg_id.as_deref(), Some("follow-up-fixed-id"));
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
    drop(queue);
    assert_eq!(count_message_entries_with_id(&slot, "follow-up-fixed-id"), 1);
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
                ..ServeMessageParams::default()
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
async fn serve_prompt_uses_requested_user_message_id_for_transcript_and_context() {
    let _api_key = install_test_api_key();
    let stream = vec![Ok(StreamEvent::FinishReason {
        reason: "stop".to_string(),
    })];
    let (state, buffer, _temp, slot, requests) =
        build_initialized_state_with_recorded_streams(vec![stream]).await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("fixed-user-id-1".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "plain text".to_string(),
            params: ServeMessageParams {
                user_message_id: Some("user-fixed-id".to_string()),
                ..ServeMessageParams::default()
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
    let user_message = captured[0]
        .messages
        .iter()
        .rev()
        .find(|message| matches!(message.role, crate::core::llm::ChatMessageRole::User))
        .expect("user message");
    assert_eq!(user_message.msg_id.as_deref(), Some("user-fixed-id"));
    drop(captured);
    assert_eq!(count_message_entries_with_id(&slot, "user-fixed-id"), 1);
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
    assert!(
        user_message
            .msg_id
            .as_deref()
            .is_some_and(|message_id| !message_id.is_empty())
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_prompt_blank_user_message_id_falls_back_to_generated_entry_id() {
    let _api_key = install_test_api_key();
    let stream = vec![Ok(StreamEvent::FinishReason {
        reason: "stop".to_string(),
    })];
    let (state, _buffer, _temp, slot) = build_initialized_state_with_streams(vec![stream]).await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("blank-user-id".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "blank id".to_string(),
            params: ServeMessageParams {
                user_message_id: Some("   ".to_string()),
                ..ServeMessageParams::default()
            },
        },
    )
    .await
    .unwrap();

    let entry = latest_user_entry(&slot);
    assert_ne!(entry.id.as_deref(), Some("   "));
    assert!(entry.id.as_deref().is_some_and(|message_id| !message_id.trim().is_empty()));
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_prompt_duplicate_user_message_id_falls_back_to_generated_entry_id() {
    let _api_key = install_test_api_key();
    let stream = vec![Ok(StreamEvent::FinishReason {
        reason: "stop".to_string(),
    })];
    let (state, _buffer, _temp, slot) = build_initialized_state_with_streams(vec![stream]).await;
    slot.ctx
        .session_runtime
        .session
        .append_message_with_id(
            serde_json::json!({
                "role": "user",
                "content": "existing",
            }),
            "dup-user-id",
        )
        .expect("seed duplicate id");

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("duplicate-user-id".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "new text".to_string(),
            params: ServeMessageParams {
                user_message_id: Some("dup-user-id".to_string()),
                ..ServeMessageParams::default()
            },
        },
    )
    .await
    .unwrap();

    let entry = latest_user_entry(&slot);
    assert_ne!(entry.id.as_deref(), Some("dup-user-id"));
    assert_eq!(count_message_entries_with_id(&slot, "dup-user-id"), 1);
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
                user_message_id: Some("steer-fixed-id".to_string()),
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
    assert_eq!(steering_message.msg_id.as_deref(), Some("steer-fixed-id"));
    drop(captured);
    assert_eq!(count_message_entries_with_id(&slot, "steer-fixed-id"), 1);
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_busy_steer_queues_and_persists_requested_user_message_id() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;
    slot.busy.store(true, Ordering::SeqCst);

    handle_command(
        Arc::clone(&state),
        ServeCommand::Steer {
            id: Some("steer-busy".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "redirect".to_string(),
            params: ServeMessageParams {
                user_message_id: Some("steer-busy-fixed-id".to_string()),
                ..ServeMessageParams::default()
            },
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("steer-busy")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("steer-busy"))
        .expect("queued steer response");
    assert_eq!(response["payload"]["queued"].as_bool(), Some(true));

    let queue = slot.ctx.session_runtime.steering_queue.lock();
    assert_eq!(queue.len(), 1, "expected one queued steering message");
    assert_eq!(queue[0].msg_id.as_deref(), Some("steer-busy-fixed-id"));
    drop(queue);
    assert_eq!(count_message_entries_with_id(&slot, "steer-busy-fixed-id"), 1);
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
async fn serve_get_messages_returns_cursor_metadata_and_continuous_pages() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;
    append_history_message(&slot, "user", "first");
    append_history_message(&slot, "assistant", "second");
    append_history_message(&slot, "user", "third");
    append_history_message(&slot, "assistant", "fourth");

    handle_command(
        Arc::clone(&state),
        ServeCommand::GetMessages {
            id: Some("gm-page-1".to_string()),
            session_id: Some(slot.session_id.clone()),
            params: GetMessagesParams {
                limit: Some(2),
                ..GetMessagesParams::default()
            },
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("gm-page-1")
    })
    .await;
    let first = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("gm-page-1"))
        .expect("first get_messages response");
    let first_page_ids = payload_message_ids(first);
    assert_eq!(first_page_ids.len(), 2);
    assert_eq!(first["payload"]["hasMore"].as_bool(), Some(true));
    let next_cursor = first["payload"]["nextCursor"]
        .as_str()
        .expect("next cursor");
    let decoded = decode_cursor(next_cursor);
    assert_eq!(decoded["boundaryId"].as_str(), Some(first_page_ids[0].as_str()));

    handle_command(
        Arc::clone(&state),
        ServeCommand::GetMessages {
            id: Some("gm-page-2".to_string()),
            session_id: Some(slot.session_id.clone()),
            params: GetMessagesParams {
                cursor: Some(next_cursor.to_string()),
                limit: Some(2),
                ..GetMessagesParams::default()
            },
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("gm-page-2")
    })
    .await;
    let second = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("gm-page-2"))
        .expect("second get_messages response");
    assert_eq!(payload_message_ids(second).len(), 2);
    assert_eq!(second["payload"]["hasMore"].as_bool(), Some(false));
    assert!(second["payload"]["nextCursor"].is_null());
    let all_ids = [payload_message_ids(second), first_page_ids].concat();
    assert_eq!(all_ids.len(), 4);
    assert_eq!(all_ids.iter().collect::<std::collections::HashSet<_>>().len(), 4);
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_get_messages_relocates_stale_cursor_by_boundary_id() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;
    let first_id = append_history_message(&slot, "user", "first");
    let second_id = append_history_message(&slot, "assistant", "second");
    let third_id = append_history_message(&slot, "user", "third");
    append_history_message(&slot, "assistant", "fourth");

    handle_command(
        Arc::clone(&state),
        ServeCommand::GetMessages {
            id: Some("gm-relocate-1".to_string()),
            session_id: Some(slot.session_id.clone()),
            params: GetMessagesParams {
                limit: Some(2),
                ..GetMessagesParams::default()
            },
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("gm-relocate-1")
    })
    .await;
    let first = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("gm-relocate-1"))
        .expect("first get_messages response");
    let next_cursor = first["payload"]["nextCursor"]
        .as_str()
        .expect("next cursor")
        .to_string();

    let transcript_path = slot
        .ctx
        .session_runtime
        .session
        .transcript_path(&slot.session_id);
    crate::core::session::transcript::insert_entry_after_message_id(
        &transcript_path,
        &second_id,
        &crate::core::session::transcript::TranscriptEntry::Custom(
            crate::core::session::transcript::CustomEntry {
                id: Some("inserted-before-third".to_string()),
                parent_id: None,
                timestamp: "2025-01-01T00:00:02.500Z".to_string(),
                extra: serde_json::json!({
                    "event": "history.inserted"
                }),
            },
        ),
    )
    .expect("insert before cursor boundary");

    handle_command(
        Arc::clone(&state),
        ServeCommand::GetMessages {
            id: Some("gm-relocate-2".to_string()),
            session_id: Some(slot.session_id.clone()),
            params: GetMessagesParams {
                cursor: Some(next_cursor),
                limit: Some(2),
                ..GetMessagesParams::default()
            },
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("gm-relocate-2")
    })
    .await;
    let second = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("gm-relocate-2"))
        .expect("second get_messages response");
    let ids = payload_message_ids(second);
    assert_eq!(ids, vec![second_id, "inserted-before-third".to_string()]);
    assert!(!ids.contains(&first_id));
    assert!(!ids.contains(&third_id));
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_get_messages_uses_best_effort_when_boundary_id_disappears() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;
    append_history_message(&slot, "user", "first");
    append_history_message(&slot, "assistant", "second");
    let third_id = append_history_message(&slot, "user", "third");
    append_history_message(&slot, "assistant", "fourth");

    handle_command(
        Arc::clone(&state),
        ServeCommand::GetMessages {
            id: Some("gm-best-effort-1".to_string()),
            session_id: Some(slot.session_id.clone()),
            params: GetMessagesParams {
                limit: Some(2),
                ..GetMessagesParams::default()
            },
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("gm-best-effort-1")
    })
    .await;
    let first = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("gm-best-effort-1"))
        .expect("first get_messages response");
    let next_cursor = first["payload"]["nextCursor"]
        .as_str()
        .expect("next cursor")
        .to_string();

    let transcript_path = slot
        .ctx
        .session_runtime
        .session
        .transcript_path(&slot.session_id);
    let lines = std::fs::read_to_string(&transcript_path)
        .expect("read transcript")
        .lines()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let rewritten = lines
        .into_iter()
        .map(|line| {
            if line.contains(&format!("\"id\":\"{third_id}\"")) {
                serde_json::to_string(&crate::core::session::transcript::TranscriptEntry::Custom(
                    crate::core::session::transcript::CustomEntry {
                        id: Some("replacement-entry".to_string()),
                        parent_id: None,
                        timestamp: "2025-01-01T00:00:03.000Z".to_string(),
                        extra: serde_json::json!({
                            "event": "history.rewritten"
                        }),
                    },
                ))
                .expect("serialize replacement")
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    std::fs::write(&transcript_path, rewritten).expect("rewrite transcript");

    handle_command(
        Arc::clone(&state),
        ServeCommand::GetMessages {
            id: Some("gm-best-effort-2".to_string()),
            session_id: Some(slot.session_id.clone()),
            params: GetMessagesParams {
                cursor: Some(next_cursor),
                limit: Some(2),
                ..GetMessagesParams::default()
            },
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("gm-best-effort-2")
    })
    .await;
    let second = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("gm-best-effort-2"))
        .expect("second get_messages response");
    assert_eq!(second.get("success").and_then(serde_json::Value::as_bool), Some(true));
    let ids = payload_message_ids(second);
    assert!(!ids.is_empty());
    assert!(!ids.contains(&third_id));
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_get_messages_returns_boundary_entries_without_truncation() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;
    append_history_message(&slot, "user", "before boundary");
    let transcript_path = slot
        .ctx
        .session_runtime
        .session
        .transcript_path(&slot.session_id);
    crate::core::session::transcript::append_entry(
        &transcript_path,
        &crate::core::session::transcript::TranscriptEntry::BranchSummary(
            crate::core::session::transcript::BranchSummaryEntry {
                id: Some("boundary-1".to_string()),
                parent_id: None,
                timestamp: "2025-01-01T00:00:02.000Z".to_string(),
                summary: Some("Earlier turns were summarized".to_string()),
                covered_start_id: None,
                covered_end_id: None,
                covered_count: Some(4),
                is_boundary: Some(true),
                preheat_compaction_id: None,
                estimated_covered_tokens_before: None,
                estimated_summary_tokens: None,
                estimated_tokens_saved: None,
                error: None,
                attempts: None,
            },
        ),
    )
    .expect("append boundary entry");
    append_history_message(&slot, "assistant", "after boundary");

    handle_command(
        Arc::clone(&state),
        ServeCommand::GetMessages {
            id: Some("gm-boundary".to_string()),
            session_id: Some(slot.session_id.clone()),
            params: GetMessagesParams {
                limit: Some(4),
                ..GetMessagesParams::default()
            },
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("gm-boundary")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("gm-boundary"))
        .expect("get_messages response");
    let entries = response["payload"]["messages"]
        .as_array()
        .expect("messages array");
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[1]["type"].as_str(), Some("branch_summary"));
    assert_eq!(entries[1]["id"].as_str(), Some("boundary-1"));
    assert_eq!(entries[2]["message"]["content"].as_str(), Some("after boundary"));
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
                ..ServeMessageParams::default()
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
async fn serve_get_state_reports_interrupted_alongside_busy() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;
    slot.busy.store(true, Ordering::SeqCst);
    slot.ctx.session_runtime.cancel_token.lock().cancel();

    handle_command(
        Arc::clone(&state),
        ServeCommand::GetState {
            id: Some("state-interrupted".to_string()),
            session_id: Some(slot.session_id.clone()),
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("state-interrupted")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("state-interrupted"))
        .expect("get_state response");
    let payload = response["payload"].clone();

    assert_eq!(payload["busy"].as_bool(), Some(true));
    assert_eq!(payload["interrupted"].as_bool(), Some(true));
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_list_sessions_live_reports_interrupted_flag() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;
    slot.busy.store(true, Ordering::SeqCst);
    slot.ctx.session_runtime.cancel_token.lock().cancel();

    handle_command(
        Arc::clone(&state),
        ServeCommand::ListSessions {
            id: Some("list-live-interrupted".to_string()),
            scope: Some(ListSessionsScope::Live),
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("list-live-interrupted")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| {
            line.get("id").and_then(serde_json::Value::as_str)
                == Some("list-live-interrupted")
        })
        .expect("list_sessions response");
    let sessions = response["payload"]["sessions"]
        .as_array()
        .expect("sessions array");
    let current = sessions
        .iter()
        .find(|entry| entry["sessionId"].as_str() == Some(slot.session_id.as_str()))
        .expect("current session summary");

    assert_eq!(current["busy"].as_bool(), Some(true));
    assert_eq!(current["interrupted"].as_bool(), Some(true));
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_interrupt_rearms_root_token_before_next_turn_can_spawn_subagents() {
    let _api_key = install_test_api_key();
    let stream = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "after interrupt".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![stream]).await;

    crate::api::serve::control::handle_control_or_interrupt(
        Arc::clone(&state),
        ServeCommand::Interrupt {
            id: Some("interrupt-rearm".to_string()),
            session_id: Some(slot.session_id.clone()),
        },
    )
    .await
    .unwrap();

    let blocked = slot
        .ctx
        .agent_registry
        .spawn_subagent_internal(
            &slot.session_id,
            crate::core::agent_loop::SubagentType::Reviewer,
            |ctx| async move {
                crate::core::agent_registry::SubagentOutcome {
                    child_session_id: ctx.child_session_id,
                    subagent_type: ctx.subagent_type,
                    outcome_label: crate::core::agent_registry::SubagentOutcomeLabel::Completed,
                    error_message: None,
                }
            },
        )
        .await
        .expect_err("interrupt 后、rearm 前应拒绝派生子 Agent");
    assert!(
        matches!(blocked, crate::core::agent_registry::SpawnError::ParentAborted(_)),
        "实际错误 = {blocked:?}"
    );

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("prompt-after-interrupt".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "hello again".to_string(),
            params: ServeMessageParams::default(),
        },
    )
    .await
    .unwrap();

    let _lines = wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
    })
    .await;
    for _ in 0..50 {
        if !slot.is_busy() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        !slot.is_busy(),
        "第二回合结束后 session 应回到 idle，才能验证 rearm 结果"
    );
    assert!(
        !slot.ctx.session_runtime.cancel_token.lock().is_cancelled(),
        "start_turn 应已安装新的 turn token"
    );

    let outcome = slot
        .ctx
        .agent_registry
        .spawn_subagent_internal(
            &slot.session_id,
            crate::core::agent_loop::SubagentType::Reviewer,
            |ctx| async move {
                crate::core::agent_registry::SubagentOutcome {
                    child_session_id: ctx.child_session_id,
                    subagent_type: ctx.subagent_type,
                    outcome_label: crate::core::agent_registry::SubagentOutcomeLabel::Completed,
                    error_message: None,
                }
            },
        )
        .await
        .expect("start_turn rearm 后应可再次派生子 Agent");
    assert_eq!(
        outcome.outcome_label,
        crate::core::agent_registry::SubagentOutcomeLabel::Completed
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_set_plan_mode_exit_demotes_idle_executing_plan_before_returning_to_chat() {
    use crate::core::plan_runtime::file_store::{
        read_plan, write_plan, PlanFile, PlanFileFrontmatter, PlanFileState, TodoItem, TodoStatus,
        PLAN_FILE_SCHEMA_VERSION,
    };

    let _api_key = install_test_api_key();
    let (state, buffer, temp, slot) = build_initialized_state_with_streams(vec![]).await;

    slot.ctx
        .session_runtime
        .plan_runtime
        .enter_planning()
        .expect("enter planning");
    let plan_path = temp.path().join("exit-executing.plan.md");
    let plan = PlanFile {
        frontmatter: PlanFileFrontmatter {
            plan_id: "plan-exit".into(),
            goal: "leave executing safely".into(),
            state: PlanFileState::Planning,
            session_key: None,
            session_id: None,
            created_at: chrono::Utc::now().to_rfc3339(),
            schema_version: PLAN_FILE_SCHEMA_VERSION,
            todos: vec![TodoItem {
                id: "todo-1".into(),
                content: "ship it".into(),
                status: TodoStatus::Pending,
            }],
            unknown: serde_yaml::Mapping::new(),
        },
        body: "## Plan\n- pending".into(),
    };
    write_plan(&plan_path, &plan, slot.ctx.session_runtime.plan_runtime.lock_timeout_ms())
        .expect("write temp plan");
    slot.ctx
        .session_runtime
        .plan_runtime
        .set_active_planning_plan("plan-exit".into(), plan_path.clone());
    slot.ctx
        .session_runtime
        .plan_runtime
        .build_plan(
            plan_path.to_str().expect("utf8 plan path"),
            Some(slot.session_id.clone()),
        )
        .expect("build plan into executing");

    handle_command(
        Arc::clone(&state),
        ServeCommand::SetPlanMode {
            id: Some("exit-executing".to_string()),
            session_id: Some(slot.session_id.clone()),
            action: SetPlanModeAction::Exit,
            plan_id: None,
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("exit-executing")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("exit-executing"))
        .expect("exit response");
    assert_eq!(
        response.get("success").and_then(serde_json::Value::as_bool),
        Some(true)
    );
    assert_eq!(
        response["payload"]["planState"].as_str(),
        Some("chat"),
        "executing+idle 退出后应回到 chat"
    );

    let disk_plan = read_plan(&plan_path).expect("read demoted plan");
    assert_eq!(
        disk_plan.frontmatter.state,
        PlanFileState::Pending,
        "退出 chat 前必须先把盘上的 executing 降级成 pending"
    );
    assert!(
        matches!(
            slot.ctx.session_runtime.plan_runtime.mode(),
            crate::core::plan_runtime::PlanState::Chat
        ),
        "runtime 也应回到 Chat"
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_get_state_contains_plan_and_session_todos() {
    use crate::core::plan_runtime::file_store::{TodoItem, TodoStatus};

    tracing::info!(target: "test", phase = "arrange", test = "serve_get_state_contains_plan_and_session_todos");
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;

    // 注入一条 session scratchpad todo，使 get_state 的 sessionTodos 非空。
    slot.ctx
        .session_runtime
        .plan_runtime
        .replace_session_todos(vec![TodoItem {
            id: "st-1".to_string(),
            content: "wire session todos".to_string(),
            status: TodoStatus::InProgress,
        }]);

    tracing::info!(target: "test", phase = "act", test = "serve_get_state_contains_plan_and_session_todos");
    handle_command(
        Arc::clone(&state),
        ServeCommand::GetState {
            id: Some("state-todos".to_string()),
            session_id: Some(slot.session_id.clone()),
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("state-todos")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("state-todos"))
        .expect("get_state response");
    let payload = response["payload"].clone();

    tracing::info!(target: "test", phase = "assert", test = "serve_get_state_contains_plan_and_session_todos");
    assert_eq!(response["success"].as_bool(), Some(true));
    assert!(payload.get("planPath").is_some(), "get_state payload must include planPath");
    assert!(
        payload.get("contextUtilizationRatio").is_some(),
        "get_state payload must include contextUtilizationRatio"
    );
    assert!(payload["planPath"].is_null(), "no active plan => planPath null");
    assert!(
        payload["contextUtilizationRatio"].is_null(),
        "fresh session without persisted metrics => contextUtilizationRatio null"
    );
    // planTodos 字段必须存在且为数组（当前无 active plan → 空数组）。
    let plan_todos = payload["planTodos"]
        .as_array()
        .expect("get_state payload must include planTodos array");
    assert!(plan_todos.is_empty(), "no active plan => planTodos empty");
    // sessionTodos 必须回显注入的 in_progress 项。
    let session_todos = payload["sessionTodos"]
        .as_array()
        .expect("get_state payload must include sessionTodos array");
    assert_eq!(session_todos.len(), 1);
    assert_eq!(session_todos[0]["id"].as_str(), Some("st-1"));
    assert_eq!(session_todos[0]["content"].as_str(), Some("wire session todos"));
    assert_eq!(session_todos[0]["status"].as_str(), Some("in_progress"));
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_get_state_includes_active_plan_path_and_context_ratio() {
    use crate::core::plan_runtime::file_store::{
        write_plan, PlanFile, PlanFileFrontmatter, PlanFileState, TodoItem, TodoStatus,
        PLAN_FILE_SCHEMA_VERSION,
    };

    let _api_key = install_test_api_key();
    let (state, buffer, temp, slot) = build_initialized_state_with_streams(vec![]).await;
    let session_mgr = &slot.ctx.session_runtime.session;
    session_mgr
        .update_session(session_mgr.current_session_key(), |entry| {
            entry.context_utilization_ratio = Some(0.42);
        })
        .unwrap();
    slot.ctx
        .session_runtime
        .plan_runtime
        .enter_planning()
        .expect("enter planning");
    let plan_path = temp.path().join("active.plan.md");
    let plan = PlanFile {
        frontmatter: PlanFileFrontmatter {
            plan_id: "plan-1".into(),
            goal: "Restore active plan".into(),
            state: PlanFileState::Planning,
            session_key: None,
            session_id: None,
            created_at: chrono::Utc::now().to_rfc3339(),
            schema_version: PLAN_FILE_SCHEMA_VERSION,
            todos: vec![TodoItem {
                id: "todo-1".into(),
                content: "restore".into(),
                status: TodoStatus::Pending,
            }],
            unknown: serde_yaml::Mapping::new(),
        },
        body: "## Plan\n- restore".into(),
    };
    write_plan(&plan_path, &plan, slot.ctx.session_runtime.plan_runtime.lock_timeout_ms())
        .expect("write temp plan");
    slot.ctx
        .session_runtime
        .plan_runtime
        .set_active_planning_plan("plan-1".into(), plan_path.clone());

    handle_command(
        Arc::clone(&state),
        ServeCommand::GetState {
            id: Some("state-active-plan".to_string()),
            session_id: Some(slot.session_id.clone()),
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("state-active-plan")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("state-active-plan"))
        .expect("get_state response");
    let payload = response["payload"].clone();

    assert_eq!(
        payload["planPath"].as_str(),
        Some(plan_path.to_string_lossy().as_ref())
    );
    assert_eq!(payload["contextUtilizationRatio"].as_f64(), Some(0.42));
    assert_eq!(payload["planState"].as_str(), Some("planning"));
    let plan_todos = payload["planTodos"]
        .as_array()
        .expect("get_state payload must include planTodos array");
    assert_eq!(plan_todos.len(), 1);
    assert_eq!(plan_todos[0]["id"].as_str(), Some("todo-1"));
    assert_eq!(plan_todos[0]["status"].as_str(), Some("pending"));
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
