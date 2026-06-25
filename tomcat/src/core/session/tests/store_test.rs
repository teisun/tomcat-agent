use super::super::store::*;

#[test]
fn load_store_missing_file_returns_empty() {
    let dir = std::env::temp_dir().join("tomcat_store_test_missing");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("nonexistent.json");
    let store = load_store(&path).unwrap();
    assert!(store.is_empty());
    assert!(
        path.exists(),
        "missing store should be initialized on first load"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn save_and_load_store_roundtrip() {
    let dir = std::env::temp_dir().join("tomcat_store_test_roundtrip");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("sessions.json");
    let mut store = SessionStore::new();
    store
        .current
        .insert("agent:main:main".to_string(), "123_abc".to_string());
    store.sessions.insert(
        "123_abc".to_string(),
        SessionEntry {
            session_key: "agent:main:main".to_string(),
            session_id: "123_abc".to_string(),
            updated_at: 1_000_000,
            session_file: None,
            cwd: Some("/tmp".to_string()),
            thinking_level: None,
            model_override: None,
            input_tokens: None,
            output_tokens: None,
            compaction_count: None,
            compaction_tokens_freed: None,
            tool_result_chars_persisted: None,
            last_checkpoint_id: None,
            title: None,
        },
    );
    save_store(&path, &store).unwrap();
    let loaded = load_store(&path).unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(
        loaded.current.get("agent:main:main").map(String::as_str),
        Some("123_abc")
    );
    let e = loaded.sessions.get("123_abc").unwrap();
    assert_eq!(e.session_key, "agent:main:main");
    assert_eq!(e.session_id, "123_abc");
    assert_eq!(e.updated_at, 1_000_000);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir(&dir);
}

#[test]
fn load_store_empty_file_returns_empty() {
    let dir = std::env::temp_dir().join("tomcat_store_test_empty");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("empty.json");
    std::fs::write(&path, "").unwrap();
    let store = load_store(&path).unwrap();
    assert!(store.is_empty());
    let rewritten = std::fs::read_to_string(&path).unwrap();
    let parsed: SessionStore = serde_json::from_str(&rewritten).unwrap();
    assert!(parsed.is_empty());
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir(&dir);
}

#[test]
fn load_store_resets_legacy_shape_to_new_store() {
    let dir = std::env::temp_dir().join("tomcat_store_test_v1_reset");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("sessions.json");
    std::fs::write(
        &path,
        r#"{
  "agent:main:main": {
    "sessionId": "legacy_1",
    "updatedAt": 42,
    "cwd": "/tmp/project"
  }
}"#,
    )
    .unwrap();

    let loaded = load_store(&path).unwrap();
    assert!(
        loaded.is_empty(),
        "legacy shape should be replaced directly"
    );
    let rewritten = std::fs::read_to_string(&path).unwrap();
    let parsed: SessionStore = serde_json::from_str(&rewritten).unwrap();
    assert!(parsed.is_empty());

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir(&dir);
}

#[test]
fn load_store_resets_invalid_json_to_new_store() {
    let dir = std::env::temp_dir().join("tomcat_store_test_invalid_reset");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("sessions.json");
    std::fs::write(&path, "{not-json").unwrap();

    let loaded = load_store(&path).unwrap();
    assert!(loaded.is_empty());
    let rewritten = std::fs::read_to_string(&path).unwrap();
    let parsed: SessionStore = serde_json::from_str(&rewritten).unwrap();
    assert!(parsed.is_empty());

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir(&dir);
}
