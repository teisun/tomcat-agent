//! # `SessionManager` 基础 CRUD / store / transcript 路径测试
//!
//! 覆盖：
//!
//! - `create_session` / `list_sessions` / `delete_session` / `get_session`：
//!   会话条目的增删查改。
//! - `load_store` / `from_sessions_dir`：磁盘存储的初始/解析状态。
//! - `transcript_path` / `read_session_header` / `current_transcript_path`
//!   等只读查询。
//! - `get_entry` / `get_children` / `get_leaf_entry` / `get_branch`：transcript
//!   只读 API 在空/有内容场景的退化值。
//! - `update_session`：闭包式字段更新与 `updated_at` 单调递增。

use super::super::*;
use super::mocks::temp_sessions_dir;

#[test]
fn create_session_and_list() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    let entry = mgr.create_session(key, Some("/tmp".to_string())).unwrap();
    assert!(!entry.session_id.is_empty());
    assert!(entry.updated_at > 0);
    let list = mgr.list_sessions().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].0, entry.session_id);
    assert_eq!(list[0].1.session_key, key);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn new_current_session_repoints_default_key_and_keeps_history() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());

    let first = mgr
        .new_current_session(Some("/tmp/one".to_string()))
        .expect("first session");
    let second = mgr
        .new_current_session(Some("/tmp/two".to_string()))
        .expect("second session");

    assert_ne!(first.session_id, second.session_id);
    assert_eq!(
        mgr.current_session_id().unwrap().as_deref(),
        Some(second.session_id.as_str())
    );
    let ids = mgr.list_session_ids().unwrap();
    assert!(ids.contains(&first.session_id));
    assert!(ids.contains(&second.session_id));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn switch_current_to_session_id_repoints_default_key() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());

    let first = mgr.new_current_session(None).expect("first");
    let second = mgr.new_current_session(None).expect("second");
    assert_eq!(
        mgr.current_session_id().unwrap().as_deref(),
        Some(second.session_id.as_str())
    );

    let switched = mgr
        .switch_current_to_session_id(&first.session_id)
        .expect("switch to first");
    assert_eq!(switched.session_id, first.session_id);
    assert_eq!(
        mgr.current_session_id().unwrap().as_deref(),
        Some(first.session_id.as_str())
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn load_store_empty_when_no_file() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let store = mgr.load_store().unwrap();
    assert!(store.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn ensure_current_session_rebuilds_legacy_store_without_init() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("sessions.json"),
        r#"{
  "agent:main:main": {
    "sessionId": "legacy_1",
    "updatedAt": 42,
    "cwd": "/tmp/project"
  }
}"#,
    )
    .unwrap();

    let mgr = SessionManager::new(dir.clone());
    let entry = mgr
        .ensure_current_session(Some("/tmp/new".to_string()))
        .expect("ensure current session");
    let store = mgr.load_store().expect("load rebuilt store");

    assert_eq!(
        store
            .current
            .get(mgr.current_session_key())
            .map(String::as_str),
        Some(entry.session_id.as_str())
    );
    assert_eq!(store.sessions.len(), 1, "legacy data should be replaced");
    assert_eq!(
        store
            .sessions
            .get(&entry.session_id)
            .and_then(|entry| entry.cwd.as_deref()),
        Some("/tmp/new")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn create_then_get_entries() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();
    let entries = mgr.get_entries(10).unwrap();
    assert!(entries.is_empty());
    mgr.append_message(serde_json::json!({"role":"user","content":"hi"}))
        .unwrap();
    let entries2 = mgr.get_entries(10).unwrap();
    assert_eq!(entries2.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn delete_session_removes_from_store() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    let entry = mgr.create_session(key, None).unwrap();
    assert_eq!(mgr.list_sessions().unwrap().len(), 1);
    mgr.delete_session(&entry.session_id).unwrap();
    assert!(mgr.list_sessions().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn get_session_returns_none_for_unknown_key() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let opt = mgr.get_session("unknown:key").unwrap();
    assert!(opt.is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn from_sessions_dir_with_absolute_path() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path_str = dir.to_string_lossy();
    let mgr = SessionManager::from_sessions_dir(path_str.as_ref()).unwrap();
    assert!(mgr.store_path().ends_with("sessions.json"));
    assert!(mgr.load_store().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn transcript_path_format() {
    let dir = temp_sessions_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let p = mgr.transcript_path("sid_123");
    assert!(p.ends_with("sid_123.jsonl"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn get_session_returns_some_after_create() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    let created = mgr.create_session(key, None).unwrap();
    let opt = mgr.get_session(key).unwrap();
    assert!(opt.is_some());
    let entry = opt.unwrap();
    assert_eq!(entry.session_id, created.session_id);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn read_session_header_after_create() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();
    let header = mgr.read_session_header().unwrap();
    assert!(header.is_some());
    assert!(!header.unwrap().id.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn read_session_header_none_when_no_session() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let header = mgr.read_session_header().unwrap();
    assert!(header.is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn get_entry_with_session_returns_option() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();
    mgr.append_message(serde_json::json!({"role":"user","content":"hi"}))
        .unwrap();
    let opt = mgr.get_entry("unknown_id").unwrap();
    assert!(opt.is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn get_children_with_session_returns_vec() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();
    let children = mgr.get_children("parent", 5).unwrap();
    assert!(children.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn get_leaf_entry_with_session_returns_last() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();
    mgr.append_message(serde_json::json!({"role":"user","content":"hi"}))
        .unwrap();
    let leaf = mgr.get_leaf_entry().unwrap();
    assert!(leaf.is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn get_branch_with_session_returns_vec() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();
    let branch = mgr.get_branch("any_leaf").unwrap();
    assert!(branch.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn update_session_modifies_store() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();
    let before = mgr.get_session(key).unwrap().unwrap().updated_at;
    mgr.update_session(key, |e| {
        e.cwd = Some("/updated".to_string());
    })
    .unwrap();
    let after = mgr.get_session(key).unwrap().unwrap();
    assert!(after.updated_at >= before);
    assert_eq!(after.cwd.as_deref(), Some("/updated"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn switch_current_model_updates_override_and_appends_event() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();

    mgr.switch_current_model(Some("openai"), Some("deepseek-v4-flash"))
        .unwrap();

    let entry = mgr.get_session(key).unwrap().unwrap();
    assert_eq!(entry.model_override.as_deref(), Some("deepseek-v4-flash"));

    let entries = mgr.get_entries(8).unwrap();
    let model_change = entries
        .into_iter()
        .find_map(|entry| match entry {
            TranscriptEntry::ModelChange(change) => Some(change),
            _ => None,
        })
        .expect("应追加 model_change 事件");
    assert_eq!(model_change.provider.as_deref(), Some("openai"));
    assert_eq!(model_change.model_id.as_deref(), Some("deepseek-v4-flash"));

    let _ = std::fs::remove_dir_all(&dir);
}
