//! 集成测试：会话管理模块（SessionManager、store、transcript）组合行为。
//! 黑盒测试，仅通过 pi_wasm 公共 API；使用临时目录隔离数据。

mod common;

use pi_wasm::{SessionManager, TranscriptEntry};
use std::path::PathBuf;
use tempfile::TempDir;

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
    tracing::info!("Assert: 验证 list 非空且包含当前 key 与对应 entry");
    assert!(!list.is_empty(), "创建会话后 list_sessions 应非空");
    let (k, e) = list
        .iter()
        .find(|(k, _)| k.as_str() == key)
        .expect("应找到当前 key");
    assert_eq!(k, key);
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
    mgr.create_session(key, None)?;
    tracing::info!("Arrange: 创建临时目录、SessionManager 与一条会话");
    assert_eq!(mgr.list_sessions()?.len(), 1);
    tracing::info!("Act: 调用 delete_session(key)");
    mgr.delete_session(key)?;
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
