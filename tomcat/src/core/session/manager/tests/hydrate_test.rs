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
fn init_context_state_plan_event_scan_caps_at_max_plan_scan() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();

    let stale_path = dir.join("stale.plan.md");
    mgr.append_custom_entry(serde_json::json!({
        "event": crate::infra::wire::WIRE_PLAN_BUILD,
        "plan_id": "stale_plan",
        "path": stale_path.to_string_lossy(),
        "state": "executing",
    }))
    .unwrap();
    for idx in 0..5001 {
        mgr.append_message(serde_json::json!({
            "role": "user",
            "content": format!("turn-{idx}"),
        }))
        .unwrap();
    }

    let state = init_context_state(&mgr, &ContextConfig::default(), "sys").unwrap();
    assert!(
        state.latest_plan_event.is_none(),
        "plan event older than MAX_PLAN_SCAN should be ignored"
    );

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
