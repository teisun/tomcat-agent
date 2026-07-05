use super::super::model_thinking::ModelThinkingStore;
use crate::core::llm::ThinkingLevel;

#[test]
fn load_missing_file_initializes_store() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("model-thinking.json");

    let store = ModelThinkingStore::load(&path, ThinkingLevel::High).unwrap();

    assert!(path.exists(), "missing store should be created on first load");
    assert!(store.snapshot().is_empty());
    assert_eq!(store.get("gpt-5.4"), ThinkingLevel::High);
}

#[test]
fn set_and_reload_roundtrip_persists_override() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("model-thinking.json");
    let store = ModelThinkingStore::load(&path, ThinkingLevel::Medium).unwrap();

    store.set("gpt-5.4", ThinkingLevel::Low).unwrap();
    assert_eq!(store.get("gpt-5.4"), ThinkingLevel::Low);

    let reloaded = ModelThinkingStore::load(&path, ThinkingLevel::Medium).unwrap();
    assert_eq!(reloaded.get("gpt-5.4"), ThinkingLevel::Low);
}

#[test]
fn seed_file_loads_existing_model_levels() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("model-thinking.json");
    std::fs::write(
        &path,
        r#"{
  "models": {
    "deepseek-v4-pro": "xhigh"
  }
}"#,
    )
    .unwrap();

    let store = ModelThinkingStore::load(&path, ThinkingLevel::Medium).unwrap();

    assert_eq!(store.get("deepseek-v4-pro"), ThinkingLevel::Xhigh);
}

#[test]
fn unknown_model_falls_back_to_default_level() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("model-thinking.json");
    let store = ModelThinkingStore::load(&path, ThinkingLevel::High).unwrap();

    store.set("deepseek-v4-pro", ThinkingLevel::Xhigh).unwrap();

    assert_eq!(store.get("missing-model"), ThinkingLevel::High);
}

#[test]
fn invalid_json_resets_to_empty_store() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("model-thinking.json");
    std::fs::write(&path, "{not-json").unwrap();

    let store = ModelThinkingStore::load(&path, ThinkingLevel::Low).unwrap();
    let rewritten = std::fs::read_to_string(&path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&rewritten).unwrap();

    assert_eq!(store.get("gpt-5.4"), ThinkingLevel::Low);
    assert_eq!(parsed["models"], serde_json::json!({}));
}
