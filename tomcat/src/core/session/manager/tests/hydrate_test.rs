//! # `init_context_state` 与 `build_context_from_state` 还原路径
//!
//! 覆盖：
//!
//! - `init_context_state`：空会话 / 普通消息 / 含 compaction 摘要 /
//!   未创建会话 / boundary 边界丢弃旧轮 / 非 boundary compaction 保留旧轮
//!   六种场景，断言 `messages` / `estimate_context_chars` / `context_budget_chars`
//!   等字段符合预期。
//! - `build_context_from_state`：把 `ContextState` 拼成最终发给 LLM 的
//!   `Vec<ChatMessage>`，验证顺序与 role/kind 不丢失。

use std::path::PathBuf;

use chrono::{Duration, Utc};

use super::super::context::{compute_slice_start_anchor, compute_tail_count};
use super::super::*;
use super::mocks::temp_sessions_dir;
use crate::core::llm::{
    ChatMessage, ChatMessageRole, ContinuityMetadata, MessageKind, ReasoningContinuation,
    ReasoningFormat, ReplayRequirement,
};
use crate::core::session::resume_index::{
    load_or_rebuild_resume_index, resume_index_path, ResumeAnchor, ResumeDayAnchor,
    ResumeEntryKind, ResumeIndex,
};

fn tool_call_json(id: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "type": "function",
        "function": {
            "name": "read",
            "arguments": "{}"
        }
    })
}

fn append_json_line(path: &std::path::Path, value: serde_json::Value) {
    crate::core::session::transcript::append_line(path, &value.to_string()).unwrap();
}

fn chat_message_snapshot(message: &ChatMessage) -> serde_json::Value {
    serde_json::json!({
        "role": format!("{:?}", message.role),
        "content": message.content.clone(),
        "name": message.name.clone(),
        "tool_calls": message.tool_calls.clone(),
        "tool_call_id": message.tool_call_id.clone(),
        "finish_reason": message.finish_reason.clone(),
        "error_message": message.error_message.clone(),
        "error_code": message.error_code.clone(),
        "thinking_text": message.thinking_text.clone(),
        "reasoning_continuation": message.reasoning_continuation.clone(),
        "continuity": message.continuity.clone(),
        "msg_id": message.msg_id.clone(),
        "kind": format!("{:?}", message.kind),
        "timestamp": message.timestamp.clone(),
    })
}

fn plan_event_snapshot(event: &Option<PlanEventRef>) -> serde_json::Value {
    match event {
        Some(event) => serde_json::json!({
            "kind": format!("{:?}", event.kind),
            "plan_id": event.plan_id,
            "path": event.path.to_string_lossy(),
        }),
        None => serde_json::Value::Null,
    }
}

fn preheat_snapshot(preheat: &crate::core::compaction::preheat::Preheat) -> serde_json::Value {
    serde_json::json!({
        "idle": preheat.is_idle(),
        "running": preheat.is_running(),
        "finished": preheat.is_finished(),
        "pending": preheat.preheat_result_pending(),
        "exhausted": preheat.is_exhausted_pending(),
    })
}

fn assert_context_parity(actual: &ContextState, expected: &ContextState) {
    let actual_messages: Vec<_> = actual.messages.iter().map(chat_message_snapshot).collect();
    let expected_messages: Vec<_> = expected
        .messages
        .iter()
        .map(chat_message_snapshot)
        .collect();
    assert_eq!(
        actual_messages, expected_messages,
        "messages should match full hydration"
    );
    assert_eq!(
        plan_event_snapshot(&actual.latest_plan_event),
        plan_event_snapshot(&expected.latest_plan_event)
    );
    assert_eq!(
        preheat_snapshot(&actual.preheat),
        preheat_snapshot(&expected.preheat)
    );
}

fn make_anchor(id: &str, ordinal: usize) -> ResumeAnchor {
    ResumeAnchor {
        entry_id: Some(id.to_string()),
        ordinal,
        timestamp: "2025-01-01T00:00:00.000Z".to_string(),
        entry_kind: ResumeEntryKind::Message,
    }
}

fn base_resume_index(total_entries: usize) -> ResumeIndex {
    ResumeIndex {
        schema_version: 1,
        transcript_size: 0,
        transcript_mtime_ms: 0,
        total_entries,
        last_entry_id: None,
        latest_boundary: None,
        recent_turn_starts: Vec::new(),
        latest_day_first_entry: None,
        latest_plan_event: None,
    }
}

#[test]
fn init_context_state_empty_session() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();

    let cfg = ContextConfig::default();
    let state = init_context_state(&mgr, &cfg, "system prompt").unwrap();
    assert!(state.messages.is_empty());
    assert_eq!(state.estimate_context_chars, "system prompt".len());
    assert_eq!(state.context_budget_chars, 1_088_000);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn init_context_state_with_messages() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();

    mgr.append_message(serde_json::json!({"role":"user","content":"q1"}))
        .unwrap();
    mgr.append_message(serde_json::json!({"role":"assistant","content":"a1"}))
        .unwrap();
    mgr.append_message(serde_json::json!({"role":"user","content":"q2"}))
        .unwrap();
    mgr.append_message(serde_json::json!({"role":"assistant","content":"a2"}))
        .unwrap();

    let cfg = ContextConfig::default();
    let state = init_context_state(&mgr, &cfg, "sys").unwrap();
    assert_eq!(state.messages.len(), 4);
    assert!(state.estimate_context_chars > 0);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn init_context_state_preserves_assistant_completion_metadata() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();

    mgr.append_message(serde_json::json!({"role":"user","content":"q1"}))
        .unwrap();
    mgr.append_message(serde_json::json!({
        "role":"assistant",
        "content":"partial",
        "finish_reason":"error:boom",
        "error_message":"boom",
        "error_code":"server_error"
    }))
    .unwrap();

    let state = init_context_state(&mgr, &ContextConfig::default(), "sys").unwrap();
    assert_eq!(state.messages.len(), 2);
    let assistant = &state.messages[1];
    assert_eq!(assistant.role, ChatMessageRole::Assistant);
    assert_eq!(assistant.finish_reason.as_deref(), Some("error:boom"));
    assert_eq!(assistant.error_message.as_deref(), Some("boom"));
    assert_eq!(assistant.error_code.as_deref(), Some("server_error"));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn transcript_roundtrip_preserves_reasoning_continuation() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();

    let assistant = ChatMessage::assistant("done").with_reasoning_state(
        Some("safe summary".to_string()),
        Some(ReasoningContinuation {
            source_provider: "openai".to_string(),
            source_api: "responses".to_string(),
            source_model: "gpt-5".to_string(),
            format: ReasoningFormat::OpenaiResponsesReasoningItems,
            opaque_payload: serde_json::json!([{
                "type": "reasoning",
                "encrypted_content": "enc_123"
            }]),
            fallback_text: Some("safe summary".to_string()),
            provider_refs: None,
        }),
        Some(ContinuityMetadata {
            had_tool_call: false,
            replay_requirement: ReplayRequirement::SameProfileOptional,
        }),
    );
    mgr.append_message(serde_json::to_value(&assistant).unwrap())
        .unwrap();

    let state = init_context_state(&mgr, &ContextConfig::default(), "sys").unwrap();
    let assistant = state.messages.last().expect("assistant message");
    assert_eq!(assistant.text_content(), Some("done"));
    assert_eq!(assistant.thinking_text.as_deref(), Some("safe summary"));
    let continuation = assistant
        .reasoning_continuation
        .as_ref()
        .expect("reasoning_continuation");
    assert_eq!(continuation.source_provider, "openai");
    assert_eq!(
        continuation.opaque_payload[0]["encrypted_content"],
        serde_json::json!("enc_123")
    );
    let continuity = assistant.continuity.as_ref().expect("continuity");
    assert!(!continuity.had_tool_call);
    assert_eq!(
        continuity.replay_requirement,
        ReplayRequirement::SameProfileOptional
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn legacy_transcript_without_continuity_fields_still_hydrates() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();

    mgr.append_message(serde_json::json!({"role":"user","content":"q1"}))
        .unwrap();
    mgr.append_message(serde_json::json!({
        "role":"assistant",
        "content":"legacy answer",
        "finish_reason":"stop"
    }))
    .unwrap();

    let state = init_context_state(&mgr, &ContextConfig::default(), "sys").unwrap();
    assert_eq!(state.messages.len(), 2);
    let assistant = state.messages.last().expect("assistant");
    assert_eq!(assistant.text_content(), Some("legacy answer"));
    assert!(assistant.thinking_text.is_none());
    assert!(assistant.reasoning_continuation.is_none());
    assert!(assistant.continuity.is_none());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn init_context_state_extracts_latest_plan_event() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();

    let older_path = dir.join("older.plan.md");
    let latest_path = dir.join("latest.plan.md");
    mgr.append_custom_entry(serde_json::json!({
        "event": crate::infra::wire::WIRE_PLAN_CREATE,
        "plan_id": "plan_old",
        "path": older_path.to_string_lossy(),
        "state": "planning",
    }))
    .unwrap();
    mgr.append_custom_entry(serde_json::json!({
        "event": crate::infra::wire::WIRE_PLAN_BUILD,
        "plan_id": "plan_latest",
        "path": latest_path.to_string_lossy(),
        "state": "executing",
    }))
    .unwrap();

    let state = init_context_state(&mgr, &ContextConfig::default(), "sys").unwrap();
    let event = state
        .latest_plan_event
        .expect("should keep latest plan event");
    assert_eq!(event.kind, PlanEventKind::Build);
    assert_eq!(event.plan_id, "plan_latest");
    assert_eq!(event.path, latest_path);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn init_context_state_restores_plan_event_via_sidecar_outside_legacy_cap() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();

    let transcript_path = mgr.current_transcript_path().unwrap().unwrap();
    let plan_path = dir.join("latest.plan.md");
    crate::core::session::transcript::append_line(
        &transcript_path,
        &serde_json::json!({
            "type": "custom",
            "id": "plan_evt_1",
            "timestamp": "2025-01-01T00:00:00.000Z",
            "event": crate::infra::wire::WIRE_PLAN_BUILD,
            "plan_id": "plan_latest",
            "path": plan_path.to_string_lossy(),
            "state": "executing",
        })
        .to_string(),
    )
    .unwrap();
    for idx in 0..5001usize {
        crate::core::session::transcript::append_line(
            &transcript_path,
            &serde_json::json!({
                "type": "message",
                "id": format!("m_{idx}"),
                "timestamp": "2025-01-01T00:00:01.000Z",
                "message": {
                    "role": "user",
                    "content": format!("turn-{idx}"),
                }
            })
            .to_string(),
        )
        .unwrap();
    }

    let state = init_context_state(&mgr, &ContextConfig::default(), "sys").unwrap();
    let latest = state
        .latest_plan_event
        .expect("plan event should be restored");
    assert_eq!(latest.kind, PlanEventKind::Build);
    assert_eq!(latest.plan_id, "plan_latest");
    assert_eq!(latest.path, plan_path);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn compute_k_from_anchor_ordinal_and_total() {
    assert_eq!(compute_tail_count(100, 72), 28);
    assert_eq!(compute_tail_count(10, 0), 10);
    assert_eq!(compute_tail_count(10, 10), 0);
    assert_eq!(compute_tail_count(10, 12), 0);
}

#[test]
fn slice_lower_bound_is_min_of_boundary_today_and_nth_turn() {
    let today = chrono::NaiveDate::from_ymd_opt(2025, 1, 2).unwrap();
    let mut index = base_resume_index(100);
    index.latest_boundary = Some(make_anchor("boundary", 40));
    index.recent_turn_starts = vec![
        make_anchor("turn_earlier", 28),
        make_anchor("turn_later", 64),
    ];
    index.latest_day_first_entry = Some(ResumeDayAnchor {
        date: today.to_string(),
        first_entry: make_anchor("today_first", 32),
    });

    let slice_start = compute_slice_start_anchor(&index, today).expect("slice start");
    assert_eq!(slice_start.entry_id.as_deref(), Some("boundary"));

    index.latest_boundary = None;
    let slice_start =
        compute_slice_start_anchor(&index, today).expect("slice start without boundary");
    assert_eq!(slice_start.entry_id.as_deref(), Some("turn_earlier"));
}

#[test]
fn targeted_hydration_edge_anchor_mismatch_falls_back_to_full() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();

    let transcript_path = mgr.current_transcript_path().unwrap().unwrap();
    for idx in 0..12usize {
        append_json_line(
            &transcript_path,
            serde_json::json!({
                "type": "message",
                "id": format!("old_u_{idx}"),
                "timestamp": "2025-01-01T00:00:01.000Z",
                "message": { "role": "user", "content": format!("old-q-{idx}") }
            }),
        );
        append_json_line(
            &transcript_path,
            serde_json::json!({
                "type": "message",
                "id": format!("old_a_{idx}"),
                "timestamp": "2025-01-01T00:00:02.000Z",
                "message": { "role": "assistant", "content": format!("old-a-{idx}") }
            }),
        );
    }
    append_json_line(
        &transcript_path,
        serde_json::json!({
            "type": "branch_summary",
            "id": "boundary_edge",
            "timestamp": Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            "summary": "boundary summary",
            "coveredCount": 24,
            "isBoundary": true,
        }),
    );
    for idx in 0..4usize {
        append_json_line(
            &transcript_path,
            serde_json::json!({
                "type": "message",
                "id": format!("recent_u_{idx}"),
                "timestamp": Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                "message": { "role": "user", "content": format!("recent-q-{idx}") }
            }),
        );
        append_json_line(
            &transcript_path,
            serde_json::json!({
                "type": "message",
                "id": format!("recent_a_{idx}"),
                "timestamp": Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                "message": { "role": "assistant", "content": format!("recent-a-{idx}") }
            }),
        );
    }

    let _ = load_or_rebuild_resume_index(&transcript_path).unwrap();
    let sidecar_path = resume_index_path(&transcript_path);
    let mut sidecar_json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&sidecar_path).unwrap()).unwrap();
    sidecar_json["latest_boundary"]["entry_id"] = serde_json::json!("stale-boundary");
    std::fs::write(
        &sidecar_path,
        serde_json::to_vec_pretty(&sidecar_json).unwrap(),
    )
    .unwrap();

    let tail = init_context_state(
        &mgr,
        &ContextConfig {
            resume_hydration_mode: crate::infra::config::ResumeHydrationMode::Tail,
            ..ContextConfig::default()
        },
        "sys",
    )
    .unwrap();
    let full = init_context_state(
        &mgr,
        &ContextConfig {
            resume_hydration_mode: crate::infra::config::ResumeHydrationMode::Full,
            ..ContextConfig::default()
        },
        "sys",
    )
    .unwrap();
    assert_context_parity(&tail, &full);

    let repaired: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&sidecar_path).unwrap()).unwrap();
    assert_eq!(
        repaired["latest_boundary"]["entry_id"],
        serde_json::json!("boundary_edge")
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn targeted_hydration_backfills_min_turns_across_midnight() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();

    let transcript_path = mgr.current_transcript_path().unwrap().unwrap();
    let yesterday =
        (Utc::now() - Duration::days(1)).to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let today = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    for idx in 0..8usize {
        append_json_line(
            &transcript_path,
            serde_json::json!({
                "type": "message",
                "id": format!("y_u_{idx}"),
                "timestamp": yesterday,
                "message": { "role": "user", "content": format!("yesterday-q-{idx}") }
            }),
        );
        append_json_line(
            &transcript_path,
            serde_json::json!({
                "type": "message",
                "id": format!("y_a_{idx}"),
                "timestamp": yesterday,
                "message": { "role": "assistant", "content": format!("yesterday-a-{idx}") }
            }),
        );
    }
    for idx in 0..3usize {
        append_json_line(
            &transcript_path,
            serde_json::json!({
                "type": "message",
                "id": format!("t_u_{idx}"),
                "timestamp": today,
                "message": { "role": "user", "content": format!("today-q-{idx}") }
            }),
        );
        append_json_line(
            &transcript_path,
            serde_json::json!({
                "type": "message",
                "id": format!("t_a_{idx}"),
                "timestamp": today,
                "message": { "role": "assistant", "content": format!("today-a-{idx}") }
            }),
        );
    }

    let tail = init_context_state(
        &mgr,
        &ContextConfig {
            resume_hydration_mode: crate::infra::config::ResumeHydrationMode::Tail,
            ..ContextConfig::default()
        },
        "sys",
    )
    .unwrap();
    let full = init_context_state(
        &mgr,
        &ContextConfig {
            resume_hydration_mode: crate::infra::config::ResumeHydrationMode::Full,
            ..ContextConfig::default()
        },
        "sys",
    )
    .unwrap();
    assert_context_parity(&tail, &full);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn targeted_hydration_today_priority_matches_full_scan() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();

    let transcript_path = mgr.current_transcript_path().unwrap().unwrap();
    let yesterday =
        (Utc::now() - Duration::days(1)).to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let today = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    for idx in 0..12usize {
        append_json_line(
            &transcript_path,
            serde_json::json!({
                "type": "message",
                "id": format!("y_u_{idx}"),
                "timestamp": yesterday,
                "message": { "role": "user", "content": format!("yesterday-q-{idx}") }
            }),
        );
        append_json_line(
            &transcript_path,
            serde_json::json!({
                "type": "message",
                "id": format!("y_a_{idx}"),
                "timestamp": yesterday,
                "message": { "role": "assistant", "content": format!("yesterday-a-{idx}") }
            }),
        );
    }
    for idx in 0..12usize {
        append_json_line(
            &transcript_path,
            serde_json::json!({
                "type": "message",
                "id": format!("t_u_{idx}"),
                "timestamp": today,
                "message": { "role": "user", "content": format!("today-q-{idx}") }
            }),
        );
        append_json_line(
            &transcript_path,
            serde_json::json!({
                "type": "message",
                "id": format!("t_a_{idx}"),
                "timestamp": today,
                "message": { "role": "assistant", "content": format!("today-a-{idx}") }
            }),
        );
    }

    let tail = init_context_state(
        &mgr,
        &ContextConfig {
            resume_hydration_mode: crate::infra::config::ResumeHydrationMode::Tail,
            ..ContextConfig::default()
        },
        "sys",
    )
    .unwrap();
    let full = init_context_state(
        &mgr,
        &ContextConfig {
            resume_hydration_mode: crate::infra::config::ResumeHydrationMode::Full,
            ..ContextConfig::default()
        },
        "sys",
    )
    .unwrap();
    assert_context_parity(&tail, &full);
    assert!(
        tail.messages
            .iter()
            .filter_map(|message| message.text_content())
            .all(|text| !text.contains("yesterday-")),
        "today-priority path should not reintroduce pre-today turns"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn targeted_hydration_no_boundary_loads_min_10_turns() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();

    let transcript_path = mgr.current_transcript_path().unwrap().unwrap();
    let yesterday =
        (Utc::now() - Duration::days(1)).to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    for idx in 0..15usize {
        append_json_line(
            &transcript_path,
            serde_json::json!({
                "type": "message",
                "id": format!("y_u_{idx}"),
                "timestamp": yesterday,
                "message": { "role": "user", "content": format!("yesterday-q-{idx}") }
            }),
        );
        append_json_line(
            &transcript_path,
            serde_json::json!({
                "type": "message",
                "id": format!("y_a_{idx}"),
                "timestamp": yesterday,
                "message": { "role": "assistant", "content": format!("yesterday-a-{idx}") }
            }),
        );
    }

    let tail = init_context_state(
        &mgr,
        &ContextConfig {
            resume_hydration_mode: crate::infra::config::ResumeHydrationMode::Tail,
            ..ContextConfig::default()
        },
        "sys",
    )
    .unwrap();
    let full = init_context_state(
        &mgr,
        &ContextConfig {
            resume_hydration_mode: crate::infra::config::ResumeHydrationMode::Full,
            ..ContextConfig::default()
        },
        "sys",
    )
    .unwrap();
    assert_context_parity(&tail, &full);
    let texts: Vec<_> = tail
        .messages
        .iter()
        .filter_map(|message| message.text_content())
        .collect();
    assert!(texts.iter().any(|text| text.contains("yesterday-q-5")));
    assert!(!texts.iter().any(|text| text.contains("yesterday-q-0")));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn init_context_state_auto_matches_full_for_large_boundary_session() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();

    let transcript_path = mgr.current_transcript_path().unwrap().unwrap();
    for idx in 0..1_100usize {
        crate::core::session::transcript::append_line(
            &transcript_path,
            &serde_json::json!({
                "type": "message",
                "id": format!("u_old_{idx}"),
                "timestamp": "2025-01-01T00:00:01.000Z",
                "message": {
                    "role": "user",
                    "content": format!("old-q-{idx}"),
                }
            })
            .to_string(),
        )
        .unwrap();
        crate::core::session::transcript::append_line(
            &transcript_path,
            &serde_json::json!({
                "type": "message",
                "id": format!("a_old_{idx}"),
                "timestamp": "2025-01-01T00:00:02.000Z",
                "message": {
                    "role": "assistant",
                    "content": format!("old-a-{idx}"),
                }
            })
            .to_string(),
        )
        .unwrap();
    }
    crate::core::session::transcript::append_line(
        &transcript_path,
        &serde_json::json!({
            "type": "branch_summary",
            "id": "boundary_keep",
            "timestamp": Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            "summary": "boundary summary",
            "coveredCount": 2200,
            "isBoundary": true,
        })
        .to_string(),
    )
    .unwrap();
    for idx in 0..12usize {
        crate::core::session::transcript::append_line(
            &transcript_path,
            &serde_json::json!({
                "type": "message",
                "id": format!("u_new_{idx}"),
                "timestamp": Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                "message": {
                    "role": "user",
                    "content": format!("new-q-{idx}"),
                    "superseded": idx == 3,
                }
            })
            .to_string(),
        )
        .unwrap();
        crate::core::session::transcript::append_line(
            &transcript_path,
            &serde_json::json!({
                "type": "message",
                "id": format!("a_new_{idx}"),
                "timestamp": Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                "message": {
                    "role": "assistant",
                    "content": format!("new-a-{idx}"),
                    "superseded": idx == 3,
                }
            })
            .to_string(),
        )
        .unwrap();
    }

    let auto = init_context_state(&mgr, &ContextConfig::default(), "sys").unwrap();
    let full = init_context_state(
        &mgr,
        &ContextConfig {
            resume_hydration_mode: crate::infra::config::ResumeHydrationMode::Full,
            ..ContextConfig::default()
        },
        "sys",
    )
    .unwrap();

    assert_context_parity(&auto, &full);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn init_context_state_with_compaction_entry() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();

    mgr.append_compaction(Some("summary of old turns")).unwrap();
    mgr.append_message(serde_json::json!({"role":"user","content":"q_after"}))
        .unwrap();
    mgr.append_message(serde_json::json!({"role":"assistant","content":"a_after"}))
        .unwrap();

    let cfg = ContextConfig::default();
    let state = init_context_state(&mgr, &cfg, "sys").unwrap();
    assert_eq!(state.messages.len(), 3);
    assert_eq!(state.messages[0].kind, MessageKind::CompactionSummary);
    assert_eq!(
        state.messages[0].text_content(),
        Some("summary of old turns")
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn build_context_from_state_flattens_turns() {
    let mut summary_msg = ChatMessage::compaction_summary("summary");
    summary_msg.msg_id = Some("sum_1".to_string());
    summary_msg.timestamp = Some("2026-04-04T12:00:00Z".to_string());

    let mut user_msg = ChatMessage::user("hello");
    user_msg.msg_id = Some("turn_1_u".to_string());
    user_msg.timestamp = Some("2026-04-04T12:00:00Z".to_string());

    let mut asst_msg = ChatMessage::assistant("world");
    asst_msg.msg_id = Some("turn_1_a".to_string());
    asst_msg.timestamp = Some("2026-04-04T12:00:00Z".to_string());

    let state = ContextState {
        messages: vec![summary_msg, user_msg, asst_msg],
        estimate_context_chars: 100,
        context_budget_chars: 1000,
        context_budget_tokens: 250,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        latest_plan_event: None,
        preheat: crate::core::compaction::preheat::Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    };
    let msgs = build_context_from_state(&state);
    assert_eq!(msgs.len(), 3);
    assert_eq!(msgs[0].kind, MessageKind::CompactionSummary);
    assert_eq!(msgs[1].role, ChatMessageRole::User);
    assert_eq!(msgs[2].role, ChatMessageRole::Assistant);
}

#[test]
fn init_context_state_no_session() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());

    let cfg = ContextConfig::default();
    let state = init_context_state(&mgr, &cfg, "sys").unwrap();
    assert!(state.messages.is_empty());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn init_context_state_boundary_discards_prior() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();

    mgr.append_message(serde_json::json!({"role":"user","content":"old q1"}))
        .unwrap();
    mgr.append_message(serde_json::json!({"role":"assistant","content":"old a1"}))
        .unwrap();
    mgr.append_message(serde_json::json!({"role":"user","content":"old q2"}))
        .unwrap();
    mgr.append_message(serde_json::json!({"role":"assistant","content":"old a2"}))
        .unwrap();

    let path = mgr.current_transcript_path().unwrap().unwrap();
    let boundary_entry = TranscriptEntry::BranchSummary(BranchSummaryEntry {
        id: None,
        parent_id: None,
        timestamp: "2026-01-01T00:00:00.000Z".to_string(),
        summary: Some("boundary summary".to_string()),
        covered_start_id: None,
        covered_end_id: None,
        covered_count: Some(2),
        is_boundary: Some(true),
        preheat_compaction_id: None,
        estimated_covered_tokens_before: None,
        estimated_summary_tokens: None,
        estimated_tokens_saved: None,
        error: None,
        attempts: None,
    });
    crate::core::session::transcript::append_entry(&path, &boundary_entry).unwrap();

    mgr.append_message(serde_json::json!({"role":"user","content":"new q"}))
        .unwrap();
    mgr.append_message(serde_json::json!({"role":"assistant","content":"new a"}))
        .unwrap();

    let cfg = ContextConfig::default();
    let state = init_context_state(&mgr, &cfg, "sys").unwrap();

    assert_eq!(state.messages.len(), 3, "boundary summary + 2 new messages");

    let has_boundary_summary = state.messages.iter().any(|m| {
        m.kind == MessageKind::CompactionSummary && m.text_content() == Some("boundary summary")
    });
    assert!(has_boundary_summary, "should contain boundary summary");

    let has_old = state
        .messages
        .iter()
        .any(|m| m.text_content().is_some_and(|t| t.contains("old")));
    assert!(!has_old, "old turns before boundary should be discarded");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn init_context_state_non_boundary_compaction_preserves_prior() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();

    mgr.append_message(serde_json::json!({"role":"user","content":"q1"}))
        .unwrap();
    mgr.append_message(serde_json::json!({"role":"assistant","content":"a1"}))
        .unwrap();
    mgr.append_compaction(Some("non-boundary summary")).unwrap();
    mgr.append_message(serde_json::json!({"role":"user","content":"q2"}))
        .unwrap();

    let cfg = ContextConfig::default();
    let state = init_context_state(&mgr, &cfg, "sys").unwrap();

    assert!(
        state.messages.len() >= 3,
        "should preserve pre-compaction turn + summary + post turn"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn init_context_state_ignores_thinking_trace_entries() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();

    mgr.append_message(serde_json::json!({"role":"user","content":"q1"}))
        .unwrap();
    mgr.append_thinking_trace("internal plan", Some("sig-test"))
        .unwrap();
    mgr.append_message(serde_json::json!({"role":"assistant","content":"a1"}))
        .unwrap();

    let cfg = ContextConfig::default();
    let state = init_context_state(&mgr, &cfg, "sys").unwrap();

    assert_eq!(
        state.messages.len(),
        2,
        "thinking_trace 不应 hydrate 进上行 messages"
    );
    let texts: Vec<String> = state
        .messages
        .iter()
        .filter_map(|m| m.text_content().map(str::to_string))
        .collect();
    assert!(texts.iter().any(|t| t == "q1"));
    assert!(texts.iter().any(|t| t == "a1"));
    assert!(
        !texts.iter().any(|t| t.contains("internal plan")),
        "thinking_trace 内容不应进入 assistant 正文"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn init_context_state_skips_superseded_messages() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();

    mgr.append_message(serde_json::json!({"role":"user","content":"q1"}))
        .unwrap();
    mgr.append_message(serde_json::json!({"role":"assistant","content":"a1"}))
        .unwrap();
    mgr.append_message(serde_json::json!({"role":"user","content":"q2","superseded":true}))
        .unwrap();
    mgr.append_message(serde_json::json!({"role":"assistant","content":"a2","superseded":true}))
        .unwrap();

    let cfg = ContextConfig::default();
    let state = init_context_state(&mgr, &cfg, "sys").unwrap();
    let texts: Vec<String> = state
        .messages
        .iter()
        .filter_map(|m| m.text_content().map(str::to_string))
        .collect();
    assert!(texts.iter().any(|t| t == "q1"));
    assert!(texts.iter().any(|t| t == "a1"));
    assert!(!texts.iter().any(|t| t == "q2"));
    assert!(!texts.iter().any(|t| t == "a2"));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn init_context_state_heals_single_dangling_tool_call_and_appends_marker() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();

    mgr.append_message(serde_json::json!({"role":"user","content":"继续"}))
        .unwrap();
    mgr.append_message(serde_json::json!({
        "role":"assistant",
        "content":"准备调用工具",
        "tool_calls":[tool_call_json("call_1")]
    }))
    .unwrap();

    let cfg = ContextConfig::default();
    let state = init_context_state(&mgr, &cfg, "sys").unwrap();

    let healed_tool = state
        .messages
        .iter()
        .find(|m| m.role == ChatMessageRole::Tool)
        .expect("hydrated messages should include synthetic tool result");
    assert_eq!(healed_tool.tool_call_id.as_deref(), Some("call_1"));
    assert_eq!(healed_tool.text_content(), Some("[interrupted]"));

    let path = mgr.current_transcript_path().unwrap().unwrap();
    let entries = crate::core::session::transcript::read_entries_tail(&path, 16).unwrap();
    let interrupted_tools = entries
        .iter()
        .filter(|entry| {
            matches!(
                entry,
                TranscriptEntry::Message(me)
                    if me.message.get("role").and_then(|v| v.as_str()) == Some("tool")
                        && me.message.get("tool_call_id").and_then(|v| v.as_str()) == Some("call_1")
                        && me.message.get("content").and_then(|v| v.as_str()) == Some("[interrupted]")
            )
        })
        .count();
    assert_eq!(
        interrupted_tools, 1,
        "should append exactly one synthetic tool result"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn init_context_state_heals_only_missing_last_tool_result() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();

    mgr.append_message(serde_json::json!({
        "role":"assistant",
        "content":"两次工具调用",
        "tool_calls":[tool_call_json("call_1"), tool_call_json("call_2")]
    }))
    .unwrap();
    mgr.append_message(serde_json::json!({
        "role":"tool",
        "tool_call_id":"call_1",
        "content":"ok"
    }))
    .unwrap();

    let cfg = ContextConfig::default();
    let state = init_context_state(&mgr, &cfg, "sys").unwrap();
    let tool_ids: Vec<&str> = state
        .messages
        .iter()
        .filter_map(|m| m.tool_call_id.as_deref())
        .collect();
    assert_eq!(tool_ids, vec!["call_1", "call_2"]);
    let last_tool = state
        .messages
        .iter()
        .rfind(|m| m.role == ChatMessageRole::Tool)
        .expect("last tool should exist");
    assert_eq!(last_tool.text_content(), Some("[interrupted]"));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn init_context_state_heals_all_missing_tail_tool_results() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();

    mgr.append_message(serde_json::json!({
        "role":"assistant",
        "content":"两次工具调用",
        "tool_calls":[tool_call_json("call_1"), tool_call_json("call_2")]
    }))
    .unwrap();

    let cfg = ContextConfig::default();
    let state = init_context_state(&mgr, &cfg, "sys").unwrap();
    let interrupted_tools: Vec<_> = state
        .messages
        .iter()
        .filter(|m| m.role == ChatMessageRole::Tool)
        .collect();
    assert_eq!(interrupted_tools.len(), 2);
    assert_eq!(interrupted_tools[0].tool_call_id.as_deref(), Some("call_1"));
    assert_eq!(interrupted_tools[1].tool_call_id.as_deref(), Some("call_2"));
    assert!(
        interrupted_tools
            .iter()
            .all(|m| m.text_content() == Some("[interrupted]")),
        "all missing tail tool results should be healed with [interrupted]"
    );

    let path = mgr.current_transcript_path().unwrap().unwrap();
    let entries = crate::core::session::transcript::read_entries_tail(&path, 8).unwrap();
    let appended_interrupted = entries
        .iter()
        .filter(|entry| {
            matches!(
                entry,
                TranscriptEntry::Message(me)
                    if me.message.get("role").and_then(|v| v.as_str()) == Some("tool")
                        && me.message.get("content").and_then(|v| v.as_str()) == Some("[interrupted]")
            )
        })
        .count();
    assert_eq!(
        appended_interrupted, 2,
        "multi-missing case should append one synthetic tool result per missing tool"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn init_context_state_does_not_heal_when_non_tool_role_interrupts_tail_tool_round() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();

    mgr.append_message(serde_json::json!({
        "role":"assistant",
        "content":"两次工具调用",
        "tool_calls":[tool_call_json("call_1"), tool_call_json("call_2")]
    }))
    .unwrap();

    let path = mgr.current_transcript_path().unwrap().unwrap();
    crate::core::session::transcript::append_entry(
        &path,
        &TranscriptEntry::Message(MessageEntry {
            id: Some(generate_entry_id()),
            parent_id: None,
            timestamp: "2026-05-26T00:00:00.000Z".to_string(),
            message: serde_json::json!({
                "role": "user",
                "content": "steering inserted here"
            }),
        }),
    )
    .unwrap();

    let cfg = ContextConfig::default();
    let state = init_context_state(&mgr, &cfg, "sys").unwrap();
    assert!(
        state
            .messages
            .iter()
            .all(|m| m.text_content() != Some("[interrupted]")),
        "non-tool tail role should make hydrate refuse to guess"
    );

    let entries = crate::core::session::transcript::read_entries_tail(&path, 8).unwrap();
    assert!(
        !entries.iter().any(|entry| {
            matches!(
                entry,
                TranscriptEntry::Message(me)
                    if me.message.get("role").and_then(|v| v.as_str()) == Some("tool")
                        && me.message.get("content").and_then(|v| v.as_str()) == Some("[interrupted]")
            )
        }),
        "broken tail should not append synthetic interrupted tool results"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
