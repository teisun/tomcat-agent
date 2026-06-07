use std::path::PathBuf;

use tempfile::TempDir;
use tomcat::{init_context_state, ContextConfig, ResumeHydrationMode, SessionManager};

fn append_json_line(path: &std::path::Path, value: serde_json::Value) {
    tomcat::core::session::append_line(path, &value.to_string()).unwrap();
}

fn sidecar_path(transcript_path: &std::path::Path) -> PathBuf {
    let stem = transcript_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap();
    transcript_path.with_file_name(format!("{stem}.resume-index.json"))
}

fn setup_mgr() -> (TempDir, SessionManager) {
    let dir = tempfile::tempdir().unwrap();
    let mgr = SessionManager::new(dir.path().to_path_buf());
    let key = mgr.current_session_key().to_string();
    mgr.create_session(&key, None).unwrap();
    (dir, mgr)
}

fn chat_message_snapshot(message: &tomcat::ChatMessage) -> serde_json::Value {
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

fn assert_public_context_parity(actual: &tomcat::ContextState, expected: &tomcat::ContextState) {
    let actual_messages: Vec<_> = actual.messages.iter().map(chat_message_snapshot).collect();
    let expected_messages: Vec<_> = expected
        .messages
        .iter()
        .map(chat_message_snapshot)
        .collect();
    assert_eq!(
        actual_messages, expected_messages,
        "public API should preserve hydrated message fields"
    );
}

#[test]
fn public_init_context_state_auto_matches_full_for_large_session() {
    let (_dir, mgr) = setup_mgr();
    let transcript_path = mgr.current_transcript_path().unwrap().unwrap();
    for idx in 0..1_100usize {
        append_json_line(
            &transcript_path,
            serde_json::json!({
                "type": "message",
                "id": format!("u_old_{idx}"),
                "timestamp": "2025-01-01T00:00:01.000Z",
                "message": { "role": "user", "content": format!("old-q-{idx}") }
            }),
        );
        append_json_line(
            &transcript_path,
            serde_json::json!({
                "type": "message",
                "id": format!("a_old_{idx}"),
                "timestamp": "2025-01-01T00:00:02.000Z",
                "message": { "role": "assistant", "content": format!("old-a-{idx}") }
            }),
        );
    }
    append_json_line(
        &transcript_path,
        serde_json::json!({
            "type": "branch_summary",
            "id": "boundary_public",
            "timestamp": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            "summary": "boundary summary",
            "coveredCount": 2200,
            "isBoundary": true,
        }),
    );
    for idx in 0..12usize {
        append_json_line(
            &transcript_path,
            serde_json::json!({
                "type": "message",
                "id": format!("u_new_{idx}"),
                "timestamp": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                "message": { "role": "user", "content": format!("new-q-{idx}") }
            }),
        );
        append_json_line(
            &transcript_path,
            serde_json::json!({
                "type": "message",
                "id": format!("a_new_{idx}"),
                "timestamp": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                "message": { "role": "assistant", "content": format!("new-a-{idx}") }
            }),
        );
    }

    let auto = init_context_state(&mgr, &ContextConfig::default(), "sys").unwrap();
    let full = init_context_state(
        &mgr,
        &ContextConfig {
            resume_hydration_mode: ResumeHydrationMode::Full,
            ..ContextConfig::default()
        },
        "sys",
    )
    .unwrap();

    assert_public_context_parity(&auto, &full);
}

#[test]
fn public_init_context_state_rebuilds_after_sidecar_delete() {
    let (_dir, mgr) = setup_mgr();
    let transcript_path = mgr.current_transcript_path().unwrap().unwrap();
    for idx in 0..2_500usize {
        append_json_line(
            &transcript_path,
            serde_json::json!({
                "type": "message",
                "id": format!("m_{idx}"),
                "timestamp": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                "message": { "role": "user", "content": format!("turn-{idx}") }
            }),
        );
    }

    let baseline = init_context_state(&mgr, &ContextConfig::default(), "sys").unwrap();
    let index_path = sidecar_path(&transcript_path);
    assert!(index_path.exists(), "baseline init should create sidecar");
    std::fs::remove_file(&index_path).unwrap();

    let rebuilt = init_context_state(&mgr, &ContextConfig::default(), "sys").unwrap();
    assert!(
        index_path.exists(),
        "resume init should rebuild missing sidecar"
    );
    assert_public_context_parity(&rebuilt, &baseline);
}

#[test]
fn kill_switch_full_uses_legacy_plan_scan_behavior() {
    let (_dir, mgr) = setup_mgr();
    let transcript_path = mgr.current_transcript_path().unwrap().unwrap();
    let plan_path = transcript_path.with_extension("plan.md");
    append_json_line(
        &transcript_path,
        serde_json::json!({
            "type": "custom",
            "id": "plan_evt_1",
            "timestamp": "2025-01-01T00:00:00.000Z",
            "event": tomcat::infra::wire::WIRE_PLAN_BUILD,
            "plan_id": "plan_old",
            "path": plan_path.to_string_lossy(),
            "state": "executing",
        }),
    );
    for idx in 0..5_001usize {
        append_json_line(
            &transcript_path,
            serde_json::json!({
                "type": "message",
                "id": format!("m_{idx}"),
                "timestamp": "2025-01-01T00:00:01.000Z",
                "message": { "role": "user", "content": format!("turn-{idx}") }
            }),
        );
    }

    let auto = init_context_state(&mgr, &ContextConfig::default(), "sys").unwrap();
    let full = init_context_state(
        &mgr,
        &ContextConfig {
            resume_hydration_mode: ResumeHydrationMode::Full,
            ..ContextConfig::default()
        },
        "sys",
    )
    .unwrap();

    assert!(
        auto.latest_plan_event.is_some(),
        "auto should restore plan from sidecar"
    );
    assert!(
        full.latest_plan_event.is_none(),
        "full kill switch should retain legacy MAX_PLAN_SCAN semantics"
    );
}
