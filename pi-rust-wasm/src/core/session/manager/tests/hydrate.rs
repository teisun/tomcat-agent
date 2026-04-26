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

use super::super::*;
use super::mocks::temp_sessions_dir;
use crate::core::llm::{ChatMessage, ChatMessageRole, MessageKind};

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
