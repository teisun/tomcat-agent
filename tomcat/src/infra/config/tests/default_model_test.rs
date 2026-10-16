use crate::infra::config::{write_llm_model_key, AppConfig};

#[test]
fn default_model_write_creates_only_the_requested_key_and_preserves_other_tables() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("nested/config.toml");
    assert!(write_llm_model_key(&path, "default_model", "deepseek-v4-pro", false).unwrap());
    let text = std::fs::read_to_string(&path).unwrap();
    let value: toml::Value = text.parse().unwrap();
    assert_eq!(value.as_table().unwrap().len(), 1);
    assert_eq!(value["llm"].as_table().unwrap().len(), 1);
    assert_eq!(
        toml::from_str::<AppConfig>(&text)
            .unwrap()
            .llm
            .default_model,
        "deepseek-v4-pro"
    );
    std::fs::write(&path, "[ui]\nlanguage='en'\n[custom]\nuntouched=true\n").unwrap();
    write_llm_model_key(&path, "default_model", "gpt-5.6", false).unwrap();
    let value: toml::Value = std::fs::read_to_string(&path).unwrap().parse().unwrap();
    assert_eq!(value["ui"]["language"].as_str(), Some("en"));
    assert_eq!(value["custom"]["untouched"].as_bool(), Some(true));
}

#[test]
fn default_model_write_does_not_replace_corrupt_or_wrong_type_config() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.toml");
    for bad in ["not toml!", "llm = 'invalid'"] {
        std::fs::write(&path, bad).unwrap();
        assert!(write_llm_model_key(&path, "default_model", "gpt-5.6", false).is_err());
        assert!(write_llm_model_key(&path, "default_model", "gpt-5.6", true).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), bad);
    }
}

#[test]
fn only_if_unset_writes_missing_or_blank_default_and_never_replaces_an_explicit_one() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.toml");
    assert!(write_llm_model_key(&path, "default_model", "first", true).unwrap());
    assert!(!write_llm_model_key(&path, "default_model", "second", true).unwrap());
    let read_default = || {
        toml::from_str::<AppConfig>(&std::fs::read_to_string(&path).unwrap())
            .unwrap()
            .llm
            .default_model
    };
    assert_eq!(read_default(), "first");

    std::fs::write(&path, "[llm]\ndefault_model = \"  \"\n").unwrap();
    assert!(write_llm_model_key(&path, "default_model", "third", true).unwrap());
    assert_eq!(read_default(), "third");

    let before = std::fs::read(&path).unwrap();
    assert!(!write_llm_model_key(&path, "default_model", "fourth", true).unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn concurrent_only_if_unset_writes_keep_exactly_one_winner() {
    let temp = tempfile::tempdir().unwrap();
    let path = std::sync::Arc::new(temp.path().join("config.toml"));
    let handles = (0..8)
        .map(|index| {
            let path = path.clone();
            std::thread::spawn(move || {
                write_llm_model_key(&path, "default_model", &format!("model-{index}"), true)
                    .unwrap()
            })
        })
        .collect::<Vec<_>>();
    let written = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .filter(|written| *written)
        .count();
    assert_eq!(written, 1);
}

#[test]
fn title_model_write_keeps_default_model_and_other_keys() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.toml");
    std::fs::write(
        &path,
        "[llm]\ndefault_model = \"gpt-5.6\"\nretry_count = 7\n[ui]\nlanguage = \"en\"\n",
    )
    .unwrap();
    assert!(write_llm_model_key(&path, "title_model", "fcodex/claude-opus-5-5", false).unwrap());
    let config = toml::from_str::<AppConfig>(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(config.llm.default_model, "gpt-5.6");
    assert_eq!(
        config.llm.title_model.as_deref(),
        Some("fcodex/claude-opus-5-5")
    );
    let value: toml::Value = std::fs::read_to_string(&path).unwrap().parse().unwrap();
    assert_eq!(value["llm"]["retry_count"].as_integer(), Some(7));
    assert_eq!(value["ui"]["language"].as_str(), Some("en"));
}
