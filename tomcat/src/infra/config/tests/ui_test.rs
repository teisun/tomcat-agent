use crate::infra::config::{ui::*, AppConfig, UiLanguage};
use crate::infra::i18n::Locale;

#[test]
fn ui_language_default_and_invalid_values() {
    let config: AppConfig = toml::from_str("").unwrap();
    assert_eq!(config.ui.language, UiLanguage::Auto);
    assert!(toml::from_str::<AppConfig>("[ui]\nlanguage='fr'").is_err());
    assert!(parse_language("fr").is_err());
}

#[test]
fn ui_precedence_is_explicit_and_environment_is_not_persisted() {
    for (language, env, host, system, expected) in [
        (UiLanguage::Auto, None, Some("zh-Hant"), "en", Locale::ZhCn),
        (UiLanguage::Auto, None, None, "zh_CN.UTF-8", Locale::ZhCn),
        (UiLanguage::En, None, Some("zh"), "zh", Locale::En),
        (UiLanguage::ZhCn, Some("en"), Some("zh"), "zh", Locale::En),
        (UiLanguage::Auto, None, None, "C", Locale::En),
        (UiLanguage::Auto, None, None, "fr", Locale::En),
    ] {
        let result = resolve_preferences(language, env, host, system).unwrap();
        assert_eq!(result.language, language);
        assert_eq!(result.effective, expected);
        assert_eq!(result.env_override, env.is_some());
    }
}

#[test]
fn ui_write_preserves_other_config_and_adds_missing_table() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.toml");
    std::fs::write(
        &path,
        "[unknown]\nfuture='kept'\n[security]\nenable_audit_log=false\n",
    )
    .unwrap();
    assert_eq!(read_language(&path).unwrap(), UiLanguage::Auto);
    write_language(&path, UiLanguage::En).unwrap();
    let result: toml::Value = std::fs::read_to_string(&path).unwrap().parse().unwrap();
    assert_eq!(result["unknown"]["future"].as_str(), Some("kept"));
    assert_eq!(
        result["security"]["enable_audit_log"].as_bool(),
        Some(false)
    );
    assert_eq!(read_language(&path).unwrap(), UiLanguage::En);
    write_language(&path, UiLanguage::Auto).unwrap();
    assert_eq!(read_language(&path).unwrap(), UiLanguage::Auto);
}

#[test]
fn ui_invalid_or_missing_file_is_not_replaced() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.toml");
    assert!(write_language(&path, UiLanguage::En).is_err());
    assert!(!path.exists());
    for bad in ["not toml!", "ui = 'invalid'"] {
        std::fs::write(&path, bad).unwrap();
        assert!(write_language(&path, UiLanguage::En).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), bad);
    }
}

#[test]
#[serial_test::serial(env_lock)]
fn ui_host_locale_is_not_a_configuration_override() {
    let _lock = crate::test_support::home_env_lock().lock().unwrap();
    let previous = std::env::var_os("TOMCAT_HOST_LOCALE");
    std::env::set_var("TOMCAT_HOST_LOCALE", "zh-CN");
    let layered = config::Config::builder()
        .add_source(config::Environment::with_prefix("TOMCAT").separator("__"))
        .build()
        .unwrap();
    let found = layered.get::<String>("host_locale");
    match previous {
        Some(value) => std::env::set_var("TOMCAT_HOST_LOCALE", value),
        None => std::env::remove_var("TOMCAT_HOST_LOCALE"),
    }
    assert!(found.is_err());
}
