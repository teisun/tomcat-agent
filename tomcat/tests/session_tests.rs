//! 集成测试：会话管理模块（SessionManager、store、transcript）组合行为。
//! 黑盒测试，仅通过 tomcat 公共 API；使用临时目录隔离数据。

mod common;

use std::path::PathBuf;
use tempfile::TempDir;
use tomcat::{
    init_context_state, session_key_for, AppError, ContextConfig, SessionManager, SessionMode,
    TranscriptEntry,
};

/// [create + list] 创建会话后 list_sessions 包含该会话
///
/// 验证：list_sessions 非空且含当前 key，session_id 一致
/// 意义：TASK-02 会话管理——create 与 list 端到端
#[test]
fn test_session_manager_create_and_list_sessions_returns_entries(
) -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span =
        tracing::info_span!("test_session_manager_create_and_list_sessions_returns_entries")
            .entered();

    let tmp = TempDir::new()?;
    let sessions_dir: PathBuf = tmp.path().to_path_buf();
    let mgr = SessionManager::new(sessions_dir);
    let key = mgr.current_session_key();
    tracing::info!("Arrange: 创建临时目录与 SessionManager，获取 current_session_key");
    let entry = mgr.create_session(key, Some("/tmp".to_string()))?;
    tracing::info!("Act: 调用 create_session 与 list_sessions");
    let list = mgr.list_sessions()?;
    tracing::info!("Assert: 验证 list 非空且包含刚创建的 session_id");
    assert!(!list.is_empty(), "创建会话后 list_sessions 应非空");
    let (session_id, e) = list
        .iter()
        .find(|(session_id, _)| session_id.as_str() == entry.session_id)
        .expect("应找到刚创建的 session_id");
    assert_eq!(session_id, &entry.session_id);
    assert_eq!(e.session_id, entry.session_id);
    Ok(())
}

/// [get_session] 创建后 get_session 返回 Some 且 id 一致
///
/// 验证：get_session 返回 Some、session_id 与 create 返回值相同
/// 意义：TASK-02 会话管理——get 可查询已创建会话
#[test]
fn test_session_manager_get_session_after_create_returns_some(
) -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span =
        tracing::info_span!("test_session_manager_get_session_after_create_returns_some").entered();

    let tmp = TempDir::new()?;
    let mgr = SessionManager::new(tmp.path().to_path_buf());
    let key = mgr.current_session_key();
    tracing::info!("Arrange: 创建临时目录与 SessionManager，create_session 创建会话");
    let created = mgr.create_session(key, None)?;
    tracing::info!("Act: 调用 get_session(key)");
    let got = mgr.get_session(key)?;
    tracing::info!("Assert: 验证 get_session 返回 Some 且 session_id 一致");
    assert!(got.is_some(), "get_session 应返回 Some");
    assert_eq!(got.unwrap().session_id, created.session_id);
    Ok(())
}

/// [delete_session] 删除会话后 list_sessions 为空
///
/// 验证：delete_session 后 list_sessions 长度为 0
/// 意义：TASK-02 会话管理——delete 可清除会话数据
#[test]
fn test_session_manager_delete_session_removes_from_list() -> Result<(), Box<dyn std::error::Error>>
{
    common::setup_logging();
    let _span =
        tracing::info_span!("test_session_manager_delete_session_removes_from_list").entered();

    let tmp = TempDir::new()?;
    let mgr = SessionManager::new(tmp.path().to_path_buf());
    let key = mgr.current_session_key();
    let entry = mgr.create_session(key, None)?;
    tracing::info!("Arrange: 创建临时目录、SessionManager 与一条会话");
    assert_eq!(mgr.list_sessions()?.len(), 1);
    tracing::info!("Act: 调用 delete_session(session_id)");
    mgr.delete_session(&entry.session_id)?;
    tracing::info!("Assert: 验证 list_sessions 为空");
    assert!(
        mgr.list_sessions()?.is_empty(),
        "删除后 list_sessions 应为空"
    );
    Ok(())
}

/// [append_message + get_entries] 追加消息后可读取到 transcript 条目
///
/// 验证：append_message 后 get_entries 非空且包含 Message 类型条目
/// 意义：TASK-02 会话管理——消息持久化端到端（transcript JSONL 写入与读取）
#[test]
fn test_session_manager_add_and_get_messages_persists() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = tracing::info_span!("test_session_manager_add_and_get_messages_persists").entered();

    let tmp = TempDir::new()?;
    let sessions_dir: PathBuf = tmp.path().to_path_buf();
    let mgr = SessionManager::new(sessions_dir);
    let key = mgr.current_session_key();
    mgr.create_session(key, Some("/tmp".to_string()))?;

    tracing::info!("Arrange: 创建会话，准备追加消息");
    let msg = serde_json::json!({
        "role": "user",
        "content": "integration test message"
    });
    mgr.append_message(msg)?;
    tracing::info!("Act: append_message + get_entries(10)");

    let entries = mgr.get_entries(10)?;
    tracing::info!("Assert: entries 非空且含 Message 条目");
    assert!(!entries.is_empty(), "append_message 后 get_entries 应非空");
    let has_message = entries
        .iter()
        .any(|e| matches!(e, TranscriptEntry::Message(_)));
    assert!(has_message, "entries 中应包含 Message 类型的条目");

    Ok(())
}

#[test]
fn test_append_message_invalid_chain_returns_invariant() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let tmp = TempDir::new()?;
    let mgr = SessionManager::new(tmp.path().to_path_buf());
    let key = mgr.current_session_key();
    mgr.create_session(key, Some("/tmp".to_string()))?;

    mgr.append_message(serde_json::json!({
        "role": "assistant",
        "content": "call tool",
        "tool_calls": [{
            "id": "call_1",
            "type": "function",
            "function": { "name": "read", "arguments": "{}" }
        }]
    }))?;

    let err = mgr
        .append_message(serde_json::json!({"role": "user", "content": "illegal"}))
        .expect_err("should reject invalid chain");
    assert!(matches!(
        err,
        AppError::Invariant {
            stage: "append_message_chain",
            ..
        }
    ));
    Ok(())
}

#[test]
fn test_append_message_invalid_tool_call_arguments_do_not_persist(
) -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let tmp = TempDir::new()?;
    let mgr = SessionManager::new(tmp.path().to_path_buf());
    let key = mgr.current_session_key();
    mgr.create_session(key, Some("/tmp".to_string()))?;

    let entries_before = mgr.get_entries(10)?;
    assert!(
        entries_before.is_empty(),
        "fresh session should not contain transcript messages"
    );

    let err = mgr
        .append_message(serde_json::json!({
            "role": "assistant",
            "content": "call tool",
            "tool_calls": [{
                "id": "call_bad",
                "type": "function",
                "function": { "name": "read", "arguments": "{\"country\":\"" }
            }]
        }))
        .expect_err("invalid tool_call arguments should be rejected");
    assert!(matches!(
        err,
        AppError::Invariant {
            stage: "append_message_chain",
            ..
        }
    ));

    let entries_after = mgr.get_entries(10)?;
    assert_eq!(
        entries_after.len(),
        entries_before.len(),
        "rejected assistant message must not be persisted"
    );
    Ok(())
}

#[test]
fn test_init_context_state_heals_all_missing_tail_tool_results(
) -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let tmp = TempDir::new()?;
    let mgr = SessionManager::new(tmp.path().to_path_buf());
    let key = mgr.current_session_key();
    mgr.create_session(key, Some("/tmp".to_string()))?;

    mgr.append_message(serde_json::json!({
        "role":"assistant",
        "content":"两次工具调用",
        "tool_calls":[
            {
                "id":"call_1",
                "type":"function",
                "function":{"name":"read","arguments":"{}"}
            },
            {
                "id":"call_2",
                "type":"function",
                "function":{"name":"read","arguments":"{}"}
            }
        ]
    }))?;

    let state = init_context_state(&mgr, &ContextConfig::default(), "sys")?;
    let healed_tools: Vec<_> = state
        .messages
        .iter()
        .filter(|m| m.tool_call_id.is_some())
        .collect();
    assert_eq!(healed_tools.len(), 2);
    assert_eq!(healed_tools[0].tool_call_id.as_deref(), Some("call_1"));
    assert_eq!(healed_tools[1].tool_call_id.as_deref(), Some("call_2"));
    assert!(healed_tools
        .iter()
        .all(|m| m.text_content() == Some("[interrupted]")));
    Ok(())
}

#[test]
fn test_code_mode_isolates_sessions_across_projects() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let tmp = TempDir::new()?;
    let sessions_dir = tmp.path().join("sessions");
    let project_a = tmp.path().join("project-a");
    let project_b = tmp.path().join("project-b");
    std::fs::create_dir_all(&sessions_dir)?;
    std::fs::create_dir_all(&project_a)?;
    std::fs::create_dir_all(&project_b)?;

    let key_a = session_key_for(SessionMode::Code, &project_a);
    let key_b = session_key_for(SessionMode::Code, &project_b);
    assert_ne!(key_a, key_b, "不同项目的 code session key 应不同");

    let mgr_a = SessionManager::new_scoped(sessions_dir.clone(), key_a.clone());
    let mgr_b = SessionManager::new_scoped(sessions_dir, key_b.clone());
    let entry_a = mgr_a.create_session(&key_a, Some(project_a.display().to_string()))?;
    let entry_b = mgr_b.create_session(&key_b, Some(project_b.display().to_string()))?;
    mgr_a.append_message(serde_json::json!({"role":"user","content":"from-a"}))?;
    mgr_b.append_message(serde_json::json!({"role":"user","content":"from-b"}))?;

    assert_eq!(mgr_a.list_sessions()?.len(), 1);
    assert_eq!(mgr_b.list_sessions()?.len(), 1);
    assert_eq!(
        mgr_a.current_session_id()?.as_deref(),
        Some(entry_a.session_id.as_str())
    );
    assert_eq!(
        mgr_b.current_session_id()?.as_deref(),
        Some(entry_b.session_id.as_str())
    );
    assert_ne!(entry_a.session_id, entry_b.session_id);
    assert_eq!(mgr_a.get_entries(8)?.len(), 1);
    assert_eq!(mgr_b.get_entries(8)?.len(), 1);
    Ok(())
}

#[test]
fn test_claw_mode_reuses_global_session_across_projects() -> Result<(), Box<dyn std::error::Error>>
{
    common::setup_logging();
    let tmp = TempDir::new()?;
    let sessions_dir = tmp.path().join("sessions");
    let project_a = tmp.path().join("project-a");
    let project_b = tmp.path().join("project-b");
    std::fs::create_dir_all(&sessions_dir)?;
    std::fs::create_dir_all(&project_a)?;
    std::fs::create_dir_all(&project_b)?;

    let claw_a = session_key_for(SessionMode::Claw, &project_a);
    let claw_b = session_key_for(SessionMode::Claw, &project_b);
    assert_eq!(claw_a, claw_b, "claw 应始终落到全局 session key");

    let mgr_a = SessionManager::new_scoped(sessions_dir.clone(), claw_a.clone());
    let created = mgr_a.create_session(&claw_a, Some(project_a.display().to_string()))?;
    mgr_a.append_message(serde_json::json!({"role":"user","content":"shared"}))?;

    let mgr_b = SessionManager::new_scoped(sessions_dir, claw_b);
    assert_eq!(
        mgr_b.current_session_id()?.as_deref(),
        Some(created.session_id.as_str())
    );
    let entries = mgr_b.get_entries(8)?;
    assert_eq!(entries.len(), 1);
    match &entries[0] {
        TranscriptEntry::Message(message) => {
            assert_eq!(
                message
                    .message
                    .get("content")
                    .and_then(|value| value.as_str()),
                Some("shared")
            );
        }
        other => panic!("expected transcript message entry, got {other:?}"),
    }
    Ok(())
}
