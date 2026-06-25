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
fn append_user_message_persists_title_once_and_never_overwrites() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key().to_string();
    mgr.create_session(&key, None).unwrap();

    // 首条 user message 写入后，title 应被派生并持久化。
    mgr.append_message(serde_json::json!({
        "role": "user",
        "content": "帮我重构 session 列表的标题逻辑\n第二行不该进标题",
    }))
    .unwrap();
    let entry = mgr.current_session_entry().unwrap().unwrap();
    assert_eq!(entry.title.as_deref(), Some("帮我重构 session 列表的标题逻辑"));

    // 后续 user message 不应覆盖已有 title。
    mgr.append_message(serde_json::json!({
        "role": "user",
        "content": "另一条完全不同的 user message",
    }))
    .unwrap();
    let entry_after = mgr.current_session_entry().unwrap().unwrap();
    assert_eq!(
        entry_after.title.as_deref(),
        Some("帮我重构 session 列表的标题逻辑")
    );

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
