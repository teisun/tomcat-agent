use crate::infra::config::{write_default_model, AppConfig};

#[test]
fn default_model_write_creates_only_the_requested_key_and_preserves_other_tables() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("nested/config.toml");
    write_default_model(&path, "deepseek-v4-pro").unwrap();
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
    write_default_model(&path, "gpt-5.4").unwrap();
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
        assert!(write_default_model(&path, "gpt-5.4").is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), bad);
    }
}
