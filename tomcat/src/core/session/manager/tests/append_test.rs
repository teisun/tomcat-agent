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
    assert_eq!(
        entry.title.as_deref(),
        Some("帮我重构 session 列表的标题逻辑")
    );

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
