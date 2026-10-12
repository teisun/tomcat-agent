use std::sync::{Arc, Barrier};

use super::super::model_thinking::ModelPrefsStore;
use crate::core::llm::ThinkingLevel;

#[test]
fn load_missing_file_initializes_store() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("model-thinking.json");

    let store = ModelPrefsStore::load(&path, ThinkingLevel::High).unwrap();

    assert!(
        path.exists(),
        "missing store should be created on first load"
    );
    assert!(store.snapshot().is_empty());
    assert_eq!(store.reasoning_for("gpt-5.4"), ThinkingLevel::High);
}

#[test]
fn set_and_reload_roundtrip_persists_override() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("model-thinking.json");
    let store = ModelPrefsStore::load(&path, ThinkingLevel::Medium).unwrap();

    store.set_reasoning("gpt-5.4", ThinkingLevel::Low).unwrap();
    assert_eq!(store.reasoning_for("gpt-5.4"), ThinkingLevel::Low);

    let reloaded = ModelPrefsStore::load(&path, ThinkingLevel::Medium).unwrap();
    assert_eq!(reloaded.reasoning_for("gpt-5.4"), ThinkingLevel::Low);
}

#[test]
fn set_and_reload_roundtrip_preserves_max_token() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("model-thinking.json");
    let store = ModelPrefsStore::load(&path, ThinkingLevel::Medium).unwrap();

    store.set_reasoning("glm-5.2", ThinkingLevel::Max).unwrap();
    assert_eq!(store.reasoning_for("glm-5.2"), ThinkingLevel::Max);

    let reloaded = ModelPrefsStore::load(&path, ThinkingLevel::Medium).unwrap();
    assert_eq!(reloaded.reasoning_for("glm-5.2"), ThinkingLevel::Max);
}

#[test]
fn updates_write_only_object_preferences_in_clean_shape() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("model-thinking.json");
    let store = ModelPrefsStore::load(&path, ThinkingLevel::Medium).unwrap();
    store.set_reasoning("gpt-5.6", ThinkingLevel::High).unwrap();
    store.set_context_window("gpt-5.6", Some(400_000)).unwrap();

    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        written,
        serde_json::json!({
            "models": {
                "gpt-5.6": {
                    "reasoning": "high",
                    "contextWindow": 400_000,
                    "speed": "standard"
                }
            }
        })
    );
}

#[test]
fn unknown_model_falls_back_to_default_level() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("model-thinking.json");
    let store = ModelPrefsStore::load(&path, ThinkingLevel::High).unwrap();

    store
        .set_reasoning("deepseek-v4-pro", ThinkingLevel::Xhigh)
        .unwrap();

    assert_eq!(store.reasoning_for("missing-model"), ThinkingLevel::High);
}

#[test]
fn corrupt_json_is_preserved_before_store_is_reset() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("model-thinking.json");
    let corrupt = "{not-json";
    std::fs::write(&path, corrupt).unwrap();

    let store = ModelPrefsStore::load(&path, ThinkingLevel::Low).unwrap();
    let rewritten = std::fs::read_to_string(&path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&rewritten).unwrap();
    let preserved = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|candidate| {
            candidate
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("model-thinking.json.corrupt-"))
        })
        .expect("corrupt source should be moved to a sibling backup");

    assert_eq!(store.reasoning_for("gpt-5.4"), ThinkingLevel::Low);
    assert_eq!(parsed["models"], serde_json::json!({}));
    assert_eq!(std::fs::read_to_string(preserved).unwrap(), corrupt);
}

#[test]
fn legacy_bare_reasoning_values_are_not_deserialized() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("model-thinking.json");
    std::fs::write(
        &path,
        r#"{
  "models": {
    "gpt-5.6": "xhigh"
  }
}"#,
    )
    .unwrap();

    let store = ModelPrefsStore::load(&path, ThinkingLevel::Medium).unwrap();

    assert_eq!(store.reasoning_for("gpt-5.6"), ThinkingLevel::Medium);
    assert!(
        std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with("model-thinking.json.corrupt-"))
            }),
        "unsupported legacy data must be preserved instead of interpreted"
    );
}

#[test]
fn object_preferences_roundtrip_without_reserved_context_keys() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("model-thinking.json");
    std::fs::write(
        &path,
        r#"{
  "models": {
    "gpt-5.6": {
      "reasoning": "high",
      "contextWindow": 1000000,
      "futureField": "ignored"
    }
  }
}"#,
    )
    .unwrap();

    let store =
        super::super::model_thinking::ModelPrefsStore::load(&path, ThinkingLevel::Medium).unwrap();
    assert_eq!(store.reasoning_for("gpt-5.6"), ThinkingLevel::High);
    assert_eq!(store.context_window_for("gpt-5.6"), Some(1_000_000));

    store.set_context_window("gpt-5.6", Some(400_000)).unwrap();
    let written = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&written).unwrap()["models"]["gpt-5.6"]
            ["contextWindow"],
        serde_json::json!(400_000),
    );
    assert!(!written.contains("__tomcat_context_window__:"));
}

#[test]
fn speed_defaults_roundtrips_and_preserves_other_preferences() {
    use crate::core::llm::Speed;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("model-thinking.json");
    let original = r#"{"models":{"relay":{"reasoning":"high","contextWindow":400000}}}"#;
    std::fs::write(&path, original).unwrap();
    let store = ModelPrefsStore::load(&path, ThinkingLevel::Medium).unwrap();
    assert_eq!(store.speed_for("relay"), Speed::Standard);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    store.set_speed("relay", Speed::Ultrafast).unwrap();
    let loaded = ModelPrefsStore::load(&path, ThinkingLevel::Medium).unwrap();
    assert_eq!(loaded.speed_for("relay"), Speed::Ultrafast);
    assert_eq!(loaded.reasoning_for("relay"), ThinkingLevel::High);
    assert_eq!(loaded.context_window_for("relay"), Some(400_000));
    loaded.set_reasoning("relay", ThinkingLevel::Max).unwrap();
    loaded.set_context_window("relay", Some(1_000_000)).unwrap();
    assert_eq!(loaded.speed_for("relay"), Speed::Ultrafast);
}

#[test]
fn speed_failed_save_keeps_memory_and_disk_unchanged() {
    use crate::core::llm::Speed;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("model-thinking.json");
    let store = ModelPrefsStore::load(&path, ThinkingLevel::Medium).unwrap();
    store.set_reasoning("relay", ThinkingLevel::High).unwrap();
    let saved = dir.path().join("saved.json");
    std::fs::rename(&path, &saved).unwrap();
    std::fs::create_dir(&path).unwrap();
    let before = std::fs::read(&saved).unwrap();
    let prefs_before = store.snapshot();
    assert!(store.set_speed("relay", Speed::Fast).is_err());
    assert_eq!(store.snapshot(), prefs_before);
    assert_eq!(std::fs::read(&saved).unwrap(), before);
    std::fs::remove_dir(&path).unwrap();
    std::fs::rename(&saved, &path).unwrap();
    store.set_context_window("relay", Some(400_000)).unwrap();
    let loaded = ModelPrefsStore::load(&path, ThinkingLevel::Medium).unwrap();
    assert_eq!(loaded.speed_for("relay"), Speed::Standard);
    assert_eq!(loaded.reasoning_for("relay"), ThinkingLevel::High);
}

#[test]
fn concurrent_updates_do_not_lose_persisted_preferences() {
    const WORKERS: usize = 16;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("model-thinking.json");
    let store = Arc::new(ModelPrefsStore::load(&path, ThinkingLevel::Medium).unwrap());
    let start = Arc::new(Barrier::new(WORKERS));
    let handles = (0..WORKERS)
        .map(|index| {
            let store = Arc::clone(&store);
            let start = Arc::clone(&start);
            std::thread::spawn(move || {
                let model = format!("concurrent-model-{index}");
                start.wait();
                store
                    .set_reasoning(&model, ThinkingLevel::High)
                    .expect("persist reasoning");
                store
                    .set_context_window(&model, Some(400_000))
                    .expect("persist context");
            })
        })
        .collect::<Vec<_>>();

    for handle in handles {
        handle.join().expect("worker should not panic");
    }

    let reloaded = ModelPrefsStore::load(&path, ThinkingLevel::Medium).unwrap();
    for index in 0..WORKERS {
        let prefs = reloaded.prefs_for(&format!("concurrent-model-{index}"));
        assert_eq!(prefs.reasoning, ThinkingLevel::High);
        assert_eq!(prefs.context_window, Some(400_000));
    }
}
