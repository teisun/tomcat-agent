//! # `SessionManager` 追加路径专项
//!
//! 覆盖：
//!
//! - `append_thinking_level_change` / `append_model_change`：会话级配置变更
//!   作为单条 transcript 落盘。
//! - `try_append_message`：单元测试 placeholder 校验（连续 tool 消息缺前置
//!   tool_call 应报错）。
//! - `generate_entry_id`：会话条目 id 在多次调用之间不重复。

use super::super::*;
use super::mocks::temp_sessions_dir;
use crate::core::session::tool_display_sidecar::{append_tool_display, tool_display_sidecar_path};
use crate::core::tools::primitive::{DiffTag, FileDiffLine};
use crate::infra::events::ToolDisplay;

fn new_copy_forward_manager() -> (tempfile::TempDir, SessionManager) {
    let temp = tempfile::tempdir().unwrap();
    let sessions_dir = temp.path().join("sessions");
    std::fs::create_dir_all(&sessions_dir).unwrap();
    let manager = SessionManager::new(sessions_dir);
    let key = manager.current_session_key().to_string();
    manager.create_session(&key, None).unwrap();
    (temp, manager)
}

fn append_failed_turn_error(manager: &SessionManager) {
    manager
        .append_error_entry(crate::core::session::transcript::ErrorEntry {
            id: Some("error-1".to_string()),
            parent_id: None,
            timestamp: "2026-08-01T00:00:00.000Z".to_string(),
            phase: None,
            provider: None,
            model: None,
            api_family: None,
            status_code: Some(429),
            request_id: None,
            failure_kind: None,
            failure_domain: None,
            summary: "retry exhausted".to_string(),
            detail: "retry exhausted".to_string(),
        })
        .unwrap();
}

fn file_display(text: &str) -> ToolDisplay {
    ToolDisplay::File {
        file: "src/lib.rs".to_string(),
        added: Some(1),
        removed: Some(0),
        diff: Some(vec![FileDiffLine {
            tag: DiffTag::Add,
            old_line: None,
            new_line: Some(1),
            skipped_lines: None,
            text: text.to_string(),
        }]),
        diff_truncated: false,
        expired: false,
    }
}

#[test]
fn tool_display_is_persisted_only_in_its_sidecar() {
    let (_temp, manager) = new_copy_forward_manager();
    let transcript = manager.current_transcript_path().unwrap().unwrap();
    manager
        .append_message(serde_json::json!({
            "role": "assistant",
            "tool_calls": [{
                "id": "display-1",
                "type": "function",
                "function": { "name": "edit", "arguments": "{}" }
            }]
        }))
        .unwrap();
    manager
        .append_message(serde_json::json!({
            "role": "tool",
            "tool_call_id": "display-1",
            "content": "edited",
            "tool_display": {
                "kind": "file",
                "file": "src/lib.rs",
                "added": 1,
                "removed": 0,
                "diff": [{"tag": "add", "newLine": 1, "text": "new"}]
            }
        }))
        .unwrap();

    let main = std::fs::read_to_string(&transcript).unwrap();
    assert!(!main.contains("\"tool_display\""));
    let sidecar = std::fs::read_to_string(tool_display_sidecar_path(&transcript)).unwrap();
    assert!(sidecar.contains("\"toolCallId\":\"display-1\""));
    assert!(sidecar.contains("\"kind\":\"file\""));
}

#[test]
fn sidecar_write_can_precede_a_failed_transcript_append_without_creating_a_message() {
    let (_temp, manager) = new_copy_forward_manager();
    let transcript = manager.current_transcript_path().unwrap().unwrap();
    manager
        .append_message(serde_json::json!({
            "role": "assistant",
            "tool_calls": [{
                "id": "orphan-display",
                "type": "function",
                "function": { "name": "edit", "arguments": "{}" }
            }]
        }))
        .unwrap();
    let before_failed_append = std::fs::read_to_string(&transcript).unwrap();
    let original_permissions = std::fs::metadata(&transcript).unwrap().permissions();
    let mut readonly_permissions = original_permissions.clone();
    readonly_permissions.set_readonly(true);
    std::fs::set_permissions(&transcript, readonly_permissions).unwrap();

    let error = manager
        .append_message(serde_json::json!({
            "role": "tool",
            "tool_call_id": "orphan-display",
            "content": "this tool result must not enter the main transcript",
            "tool_display": {
                "kind": "file",
                "file": "src/lib.rs",
                "added": 1,
                "removed": 0,
                "diff": [{"tag": "add", "newLine": 1, "text": "new"}]
            }
        }))
        .unwrap_err();
    std::fs::set_permissions(&transcript, original_permissions).unwrap();

    assert!(
        error.to_string().contains("Permission denied") || error.to_string().contains("permission"),
        "the injected transcript append failure must reach the caller: {error}"
    );
    assert!(
        !before_failed_append.contains("this tool result"),
        "the failed tool result cannot have been in the prior main transcript"
    );
    assert_eq!(
        std::fs::read_to_string(&transcript).unwrap(),
        before_failed_append,
        "a failed transcript append must not create a tool-result message"
    );
    let sidecar = std::fs::read_to_string(tool_display_sidecar_path(&transcript)).unwrap();
    assert_eq!(
        sidecar.lines().count(),
        1,
        "the durable sidecar must contain each display-producing tool result even if its main row fails"
    );
    assert!(sidecar.contains("\"toolCallId\":\"orphan-display\""));
}

#[test]
fn compaction_skips_a_busy_session_until_a_later_housekeeping_run() {
    let (_temp, manager) = new_copy_forward_manager();
    let transcript = manager.current_transcript_path().unwrap().unwrap();
    let old = chrono::Utc::now() - chrono::Duration::days(8);
    append_tool_display(
        &transcript,
        "old-display",
        &old.to_rfc3339(),
        &file_display("old"),
    )
    .unwrap();

    let lock = manager.transcript_mutex_for_path(&transcript).unwrap();
    let guard = lock.lock().unwrap();
    assert_eq!(manager.compact_tool_display_sidecars().unwrap(), 0);
    drop(guard);

    assert_eq!(manager.compact_tool_display_sidecars().unwrap(), 1);
}

#[test]
fn delete_session_removes_its_tool_display_sidecar() {
    let (_temp, manager) = new_copy_forward_manager();
    let session_id = manager.current_session_id().unwrap().unwrap();
    let transcript = manager.transcript_path(&session_id);
    append_tool_display(
        &transcript,
        "display-to-delete",
        &chrono::Utc::now().to_rfc3339(),
        &file_display("deleted"),
    )
    .unwrap();
    let sidecar = tool_display_sidecar_path(&transcript);
    assert!(sidecar.exists());

    manager.delete_session(&session_id).unwrap();

    assert!(!sidecar.exists());
}

#[test]
fn copy_forward_preserves_failed_message_and_appends_a_live_copy_after_annotations() {
    let (_temp, manager) = new_copy_forward_manager();
    let session_id = manager
        .current_session_id()
        .unwrap()
        .expect("active session id");
    let attachment_store = manager.attachment_store();
    let unrelated_pending_blob = attachment_store.put(b"draft image bytes").unwrap();
    attachment_store
        .mark_pending(&session_id, &unrelated_pending_blob)
        .unwrap();
    let pending_before_copy = attachment_store.list_pending(&session_id).unwrap();
    let blob_count_before_copy = std::fs::read_dir(attachment_store.blobs_dir())
        .unwrap()
        .count();
    let original_id = manager
        .append_message(serde_json::json!({
            "role": "user",
            "content": [{"type": "input_text", "text": "retry this exact payload"}],
            "kind": "signal",
            "attachments": [{"mime": "image/png", "data": "abc"}],
        }))
        .unwrap();
    assert_eq!(manager.mark_trailing_user_messages_superseded().unwrap(), 1);
    for attempt in 2..=4 {
        manager
            .append_custom_entry(serde_json::json!({
                "event": "auto_retry_start",
                "attempt": attempt,
            }))
            .unwrap();
    }
    manager
        .append_custom_entry(serde_json::json!({
            "event": "auto_retry_end",
            "attempt": 4,
        }))
        .unwrap();
    append_failed_turn_error(&manager);

    let copied_id = manager.copy_user_message_forward(&original_id).unwrap();
    assert_ne!(copied_id, original_id);

    let entries = manager.get_entries(16).unwrap();
    assert_eq!(
        entries.len(),
        7,
        "three retry-start diagnostics, retry-end, and error are retained between old and new rows"
    );
    let messages = entries
        .iter()
        .filter_map(|entry| match entry {
            TranscriptEntry::Message(message) => Some(message),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].id.as_deref(), Some(original_id.as_str()));
    assert_eq!(messages[1].id.as_deref(), Some(copied_id.as_str()));
    assert_eq!(messages[0].message["superseded"], true);
    assert_eq!(messages[0].message["turn_failed"], true);
    assert!(messages[1].message.get("superseded").is_none());
    assert!(messages[1].message.get("turn_failed").is_none());

    let mut expected_copy = messages[0].message.clone();
    expected_copy.as_object_mut().unwrap().remove("superseded");
    expected_copy.as_object_mut().unwrap().remove("turn_failed");
    assert_eq!(
        messages[1].message, expected_copy,
        "copy-forward must preserve every archived message field other than failure markers"
    );
    assert_eq!(
        attachment_store.list_pending(&session_id).unwrap(),
        pending_before_copy,
        "copy-forward duplicates inline transcript JSON only; it must not acquire or release blob leases"
    );
    assert_eq!(
        std::fs::read_dir(attachment_store.blobs_dir())
            .unwrap()
            .count(),
        blob_count_before_copy,
        "copy-forward must not duplicate attachment bytes into the blob store"
    );
}

#[test]
fn copy_forward_stamps_an_unstamped_anchor_before_hydration() {
    let (_temp, manager) = new_copy_forward_manager();
    let original_id = manager
        .append_message(serde_json::json!({
            "role": "user",
            "content": "completion guard failed after this prompt",
        }))
        .unwrap();
    manager
        .append_message(serde_json::json!({
            "role": "assistant",
            "content": "I started, but the stream disconnected.",
        }))
        .unwrap();
    append_failed_turn_error(&manager);

    let copied_id = manager.copy_user_message_forward(&original_id).unwrap();
    let messages = manager
        .get_entries(8)
        .unwrap()
        .into_iter()
        .filter_map(|entry| match entry {
            TranscriptEntry::Message(message) => Some(message),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].id.as_deref(), Some(original_id.as_str()));
    assert_eq!(messages[2].id.as_deref(), Some(copied_id.as_str()));
    assert_eq!(messages[0].message["superseded"], true);
    assert_eq!(messages[0].message["turn_failed"], true);
    assert_eq!(messages[1].message["superseded"], true);

    let context = init_context_state(&manager, &ContextConfig::default(), "system").unwrap();
    let prompts = context
        .messages
        .iter()
        .filter(|message| {
            message.role == crate::core::llm::ChatMessageRole::User
                && message.text_content() == Some("completion guard failed after this prompt")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        prompts.len(),
        1,
        "hydration must send only the fresh copy, never both source and copy"
    );
    assert_eq!(prompts[0].msg_id.as_deref(), Some(copied_id.as_str()));
    assert!(
        context.messages.iter().all(
            |message| message.text_content() != Some("I started, but the stream disconnected.")
        ),
        "the abandoned partial assistant response must not survive alongside the copied prompt"
    );
}

#[test]
fn copy_forward_rejects_stale_anchor_without_mutating_the_transcript() {
    let (_temp, manager) = new_copy_forward_manager();
    let original_id = manager
        .append_message(serde_json::json!({
            "role": "user",
            "content": "old request",
        }))
        .unwrap();
    manager
        .append_message(serde_json::json!({
            "role": "user",
            "content": "newer live request",
        }))
        .unwrap();
    let transcript = manager.current_transcript_path().unwrap().unwrap();
    let before = std::fs::read(&transcript).unwrap();

    let error = manager.copy_user_message_forward(&original_id).unwrap_err();
    assert!(matches!(
        error,
        crate::AppError::Config(message) if message == "retry_target_stale"
    ));
    assert_eq!(std::fs::read(transcript).unwrap(), before);
}

#[test]
fn copy_forward_rejects_a_second_click_after_the_first_copy() {
    let (_temp, manager) = new_copy_forward_manager();
    let original_id = manager
        .append_message(serde_json::json!({
            "role": "user",
            "content": "retry once",
        }))
        .unwrap();
    manager.mark_trailing_user_messages_superseded().unwrap();
    append_failed_turn_error(&manager);

    manager.copy_user_message_forward(&original_id).unwrap();
    let error = manager.copy_user_message_forward(&original_id).unwrap_err();
    assert!(matches!(
        error,
        crate::AppError::Config(message) if message == "retry_target_stale"
    ));
    let message_count = manager
        .get_entries(8)
        .unwrap()
        .iter()
        .filter(|entry| matches!(entry, TranscriptEntry::Message(_)))
        .count();
    assert_eq!(message_count, 2, "double-click must append only one copy");
}

#[test]
fn append_thinking_level_change_succeeds() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();
    let r = mgr.append_thinking_level_change("full");
    assert!(r.is_ok());
    let entries = mgr.get_entries(10).unwrap();
    assert_eq!(entries.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn append_model_change_succeeds() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();
    let r = mgr.append_model_change(Some("openai"), Some("gpt-4"));
    assert!(r.is_ok());
    let entries = mgr.get_entries(10).unwrap();
    assert_eq!(entries.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn append_thinking_trace_succeeds() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();

    mgr.append_thinking_trace("chain-of-thought-part", Some("sig-1"))
        .unwrap();

    let entries = mgr.get_entries(10).unwrap();
    assert_eq!(entries.len(), 1);
    match &entries[0] {
        TranscriptEntry::ThinkingTrace(e) => {
            assert_eq!(e.text, "chain-of-thought-part");
            assert_eq!(e.signature.as_deref(), Some("sig-1"));
        }
        other => panic!("expected thinking_trace, got {:?}", other),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn try_append_returns_err_on_violation() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();
    mgr.try_append_message(serde_json::json!({ "role": "user", "content": "hi" }))
        .unwrap();
    let result = mgr.try_append_message(serde_json::json!({
        "role": "tool",
        "tool_call_id": "c1",
        "content": "ok"
    }));
    assert!(result.is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn every_user_append_resolves_pending_ask_question_before_writing_prompt() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();
    mgr.append_message(serde_json::json!({
        "role": "assistant",
        "tool_calls": [{
            "id": "ask-1",
            "type": "function",
            "function": {
                "name": "ask_question",
                "arguments": "{\"questions\":[]}"
            }
        }]
    }))
    .unwrap();
    mgr.append_message(serde_json::json!({
        "role": "tool",
        "tool_call_id": "ask-1",
        "content": "[pending]"
    }))
    .unwrap();

    mgr.try_append_message(serde_json::json!({
        "role": "user",
        "content": "不回答旧问题，发新提示词"
    }))
    .expect("the central append gate must settle pending question");

    let entries = mgr.get_entries(16).unwrap();
    let messages = entries
        .iter()
        .filter_map(|entry| match entry {
            TranscriptEntry::Message(message) => Some(&message.message),
            _ => None,
        })
        .collect::<Vec<_>>();
    let active_ask_results = messages
        .iter()
        .filter(|message| {
            message.get("role").and_then(serde_json::Value::as_str) == Some("tool")
                && message
                    .get("tool_call_id")
                    .and_then(serde_json::Value::as_str)
                    == Some("ask-1")
                && message
                    .get("superseded")
                    .and_then(serde_json::Value::as_bool)
                    != Some(true)
        })
        .collect::<Vec<_>>();
    assert_eq!(active_ask_results.len(), 1);
    assert_eq!(
        active_ask_results[0]["content"],
        r#"{"outcome":"skipped","cancelled":true,"answers":[]}"#
    );
    crate::core::session::assert_active_tool_result_integrity(&entries)
        .expect("settling a pending question must leave one active result");
    assert_eq!(
        messages
            .last()
            .and_then(|message| message.get("role"))
            .and_then(serde_json::Value::as_str),
        Some("user")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn concurrent_question_answers_keep_only_first_terminal_result() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = std::sync::Arc::new(SessionManager::new(dir.clone()));
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();
    mgr.append_message(serde_json::json!({
        "role": "assistant",
        "tool_calls": [{
            "id": "ask-race",
            "type": "function",
            "function": {
                "name": "ask_question",
                "arguments": "{\"questions\":[]}"
            }
        }]
    }))
    .unwrap();
    mgr.append_message(serde_json::json!({
        "role": "tool",
        "tool_call_id": "ask-race",
        "content": "[pending]"
    }))
    .unwrap();

    let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
    let mut handles = Vec::new();
    for result in ["first", "second"] {
        let mgr = std::sync::Arc::clone(&mgr);
        let barrier = std::sync::Arc::clone(&barrier);
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            mgr.replace_tool_result_by_tool_call_id("ask-race", result.to_string())
        }));
    }
    barrier.wait();
    let outcomes = handles
        .into_iter()
        .map(|handle| handle.join().expect("answer worker"))
        .collect::<Vec<_>>();
    assert_eq!(
        outcomes.iter().filter(|result| result.is_ok()).count(),
        1,
        "the second window must not overwrite a real answer"
    );

    let entries = mgr.get_entries(16).unwrap();
    let active_results = entries
        .iter()
        .filter_map(|entry| match entry {
            TranscriptEntry::Message(message)
                if message
                    .message
                    .get("role")
                    .and_then(serde_json::Value::as_str)
                    == Some("tool")
                    && message
                        .message
                        .get("tool_call_id")
                        .and_then(serde_json::Value::as_str)
                        == Some("ask-race")
                    && message
                        .message
                        .get("superseded")
                        .and_then(serde_json::Value::as_bool)
                        != Some(true) =>
            {
                Some(
                    message.message["content"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                )
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(active_results.len(), 1);
    assert!(matches!(active_results[0].as_str(), "first" | "second"));
    crate::core::session::assert_active_tool_result_integrity(&entries)
        .expect("concurrent replacement must leave one active result");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn failed_tool_result_replacement_validation_keeps_transcript_byte_identical() {
    use crate::core::session::transcript::{append_entry, ErrorEntry, MessageEntry};

    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();
    mgr.append_message(serde_json::json!({
        "role": "assistant",
        "tool_calls": [{
            "id": "ask-1",
            "type": "function",
            "function": {
                "name": "ask_question",
                "arguments": "{\"questions\":[]}"
            }
        }]
    }))
    .unwrap();

    // Bypass the normal append gate to create a deliberately malformed old transcript:
    // the owning assistant falls just outside the bounded validation tail. This is the
    // exact case that used to mark `[pending]` superseded before discovering it could
    // not append the replacement.
    let path = mgr.current_transcript_path().unwrap().unwrap();
    for index in 0..64 {
        append_entry(
            &path,
            &TranscriptEntry::Error(ErrorEntry {
                id: Some(format!("noise-{index}")),
                parent_id: None,
                timestamp: "2025-01-01T00:00:00.000Z".to_string(),
                phase: None,
                provider: None,
                model: None,
                api_family: None,
                status_code: None,
                request_id: None,
                failure_kind: None,
                failure_domain: None,
                summary: "noise".to_string(),
                detail: "noise".to_string(),
            }),
        )
        .unwrap();
    }
    append_entry(
        &path,
        &TranscriptEntry::Message(MessageEntry {
            id: Some("pending-ask".to_string()),
            parent_id: None,
            timestamp: "2025-01-01T00:00:00.000Z".to_string(),
            message: serde_json::json!({
                "role": "tool",
                "tool_call_id": "ask-1",
                "content": "[pending]"
            }),
        }),
    )
    .unwrap();
    let before = std::fs::read(&path).unwrap();

    let result = mgr.replace_tool_result_by_tool_call_id("ask-1", "answer".to_string());

    assert!(
        result.is_err(),
        "the bounded chain cannot validate without its owner"
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "a rejected replacement must not supersede the durable placeholder"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn append_generates_unique_ids() {
    let id1 = generate_entry_id();
    let id2 = generate_entry_id();
    let id3 = generate_entry_id();
    assert_ne!(id1, id2);
    assert_ne!(id2, id3);
    assert_ne!(id1, id3);
}

#[test]
fn derive_title_takes_first_non_empty_line_and_truncates_to_40() {
    assert_eq!(derive_title_from_user_message("hello world"), "hello world");
    assert_eq!(
        derive_title_from_user_message("\n  \nfirst real line\nsecond"),
        "first real line"
    );
    let long = "一".repeat(50);
    let title = derive_title_from_user_message(&long);
    let chars: Vec<char> = title.chars().collect();
    assert_eq!(chars.len(), 41);
    assert_eq!(chars.last(), Some(&'\u{2026}'));
    assert_eq!(derive_title_from_user_message("   \n  \n"), "New session");
    assert_eq!(derive_title_from_user_message(""), "New session");
}

#[test]
fn extract_user_text_from_content_supports_structured_input_text_parts() {
    let content = serde_json::json!([
        { "type": "input_text", "text": "before " },
        {
            "type": "input_reference",
            "ref_kind": "file",
            "path": "src/app.ts",
            "label": "app.ts"
        },
        { "type": "input_text", "text": "after" },
        { "type": "input_file", "file_id": "file-123" }
    ]);
    assert_eq!(
        extract_user_text_from_content(&content).as_deref(),
        Some("before after")
    );
}

#[test]
fn extract_user_text_from_content_supports_plain_string_and_reference_only_none() {
    assert_eq!(
        extract_user_text_from_content(&serde_json::json!("hello")).as_deref(),
        Some("hello")
    );
    assert_eq!(
        extract_user_text_from_content(&serde_json::json!([
            {
                "type": "input_reference",
                "ref_kind": "file",
                "path": "src/app.ts",
                "label": "app.ts"
            }
        ])),
        None
    );
}

#[test]
fn append_user_message_with_structured_parts_derives_title_from_input_text() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key().to_string();
    mgr.create_session(&key, None).unwrap();

    mgr.append_message(serde_json::json!({
        "role": "user",
        "content": [
            { "type": "input_text", "text": "hello" },
            {
                "type": "input_reference",
                "ref_kind": "file",
                "path": "src/app.ts",
                "label": "app.ts"
            }
        ]
    }))
    .unwrap();

    let entry = mgr.current_session_entry().unwrap().unwrap();
    assert_eq!(entry.title.as_deref(), Some("hello"));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn append_user_message_persists_title_once_and_never_overwrites() {
    let title = "帮我重构 session 列表的标题逻辑";
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key().to_string();
    mgr.create_session(&key, None).unwrap();

    // 首条 user message 写入后，title 应被派生并持久化。
    mgr.append_message(serde_json::json!({
        "role": "user",
        "content": format!("{title}\n第二行不该进标题"),
    }))
    .unwrap();
    let entry = mgr.current_session_entry().unwrap().unwrap();
    assert_eq!(entry.title.as_deref(), Some(title));

    // 后续 user message 不应覆盖已有 title。
    mgr.append_message(serde_json::json!({
        "role": "user",
        "content": "另一条完全不同的 user message",
    }))
    .unwrap();
    let entry_after = mgr.current_session_entry().unwrap().unwrap();
    assert_eq!(entry_after.title.as_deref(), Some(title));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn append_non_user_message_does_not_set_title() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key().to_string();
    mgr.create_session(&key, None).unwrap();

    mgr.append_message(serde_json::json!({
        "role": "assistant",
        "content": "hi there",
    }))
    .unwrap();
    let entry = mgr.current_session_entry().unwrap().unwrap();
    assert!(entry.title.is_none());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn is_rule_derived_title_distinguishes_placeholder_from_semantic() {
    let text = "帮我重构 session 列表的标题逻辑";
    let placeholder = derive_title_from_user_message(text);
    assert!(is_rule_derived_title(&placeholder, text));
    // 语义 title 与规则派生串不同 → 非占位。
    assert!(!is_rule_derived_title("Refactor session list titles", text));
    // 不同 user 文本派生出不同占位，对原文本不成立。
    assert!(!is_rule_derived_title(&placeholder, "完全不同的另一条消息"));
}

#[test]
fn placeholder_title_is_replaced_by_semantic_then_preserved() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key().to_string();
    mgr.create_session(&key, None).unwrap();

    let user_text = "帮我重构 session 列表的标题逻辑";
    mgr.append_message(serde_json::json!({
        "role": "user",
        "content": user_text,
    }))
    .unwrap();
    let entry = mgr.current_session_entry().unwrap().unwrap();
    assert_eq!(entry.title.as_deref(), Some(user_text));
    assert!(is_rule_derived_title(
        entry.title.as_deref().unwrap(),
        user_text
    ));

    // 模拟异步 LLM 语义 title 覆盖占位（与 maybe_spawn_semantic_session_title 写回路径一致）。
    mgr.update_session(&key, |e| {
        e.title = Some("Refactor session list titles".to_string());
    })
    .unwrap();
    let after = mgr.current_session_entry().unwrap().unwrap();
    assert_eq!(after.title.as_deref(), Some("Refactor session list titles"));
    assert!(!is_rule_derived_title(
        after.title.as_deref().unwrap(),
        user_text
    ));

    // 语义 title 写入后，后续同文本 user append 不应回退为规则占位。
    mgr.append_message(serde_json::json!({
        "role": "user",
        "content": user_text,
    }))
    .unwrap();
    let final_entry = mgr.current_session_entry().unwrap().unwrap();
    assert_eq!(
        final_entry.title.as_deref(),
        Some("Refactor session list titles")
    );

    let _ = std::fs::remove_dir_all(&dir);
}
