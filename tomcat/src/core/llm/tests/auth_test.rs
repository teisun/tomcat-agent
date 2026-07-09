use serial_test::serial;

use crate::core::llm::{
    auth::clear_managed_credentials_for_test, env_name_for_provider, missing_key_message,
    AuthStore, ModelEntry,
};

fn entry(provider: &str, api_key_env: Option<&str>) -> ModelEntry {
    ModelEntry {
        id: format!("test-{provider}"),
        model_name: None,
        api: "openai".to_string(),
        provider: provider.to_string(),
        api_key_env: api_key_env.map(str::to_string),
        base_url: None,
        capabilities: Default::default(),
        context_window: None,
        thinking_format: None,
    }
}

#[test]
#[serial(env_lock)]
fn per_provider_env_prefers_provider_specific_api_key() {
    clear_managed_credentials_for_test();
    // SAFETY: 测试串行执行，且在本用例作用域内临时写环境变量。
    unsafe {
        std::env::set_var("DEEPSEEK_API_KEY", "deepseek-secret");
        std::env::remove_var("OPENAI_API_KEY");
    }

    let store = AuthStore;
    let credential = store
        .get(&entry("deepseek", None), Some("OPENAI_API_KEY"))
        .expect("应优先命中 DEEPSEEK_API_KEY");
    assert_eq!(credential.env_name, "DEEPSEEK_API_KEY");
    assert_eq!(credential.value, "deepseek-secret");
    assert_eq!(env_name_for_provider("deepseek"), "DEEPSEEK_API_KEY");

    unsafe {
        std::env::remove_var("DEEPSEEK_API_KEY");
        std::env::remove_var("OPENAI_API_KEY");
    }
}

#[test]
#[serial(env_lock)]
fn explicit_api_key_env_overrides_default_provider_env() {
    clear_managed_credentials_for_test();
    unsafe {
        std::env::set_var("CUSTOM_OPENAI_KEY", "custom-secret");
        std::env::remove_var("OPENAI_API_KEY");
    }

    let store = AuthStore;
    let credential = store
        .get(&entry("openai", Some("CUSTOM_OPENAI_KEY")), None)
        .expect("应优先命中显式 api_key_env");
    assert_eq!(credential.env_name, "CUSTOM_OPENAI_KEY");
    assert_eq!(credential.value, "custom-secret");

    unsafe {
        std::env::remove_var("CUSTOM_OPENAI_KEY");
    }
}

#[test]
fn missing_key_message_mentions_expected_env() {
    let msg = missing_key_message("deepseek", "DEEPSEEK_API_KEY", Some("OPENAI_API_KEY"));
    assert!(msg.contains("DEEPSEEK_API_KEY"));
    assert!(msg.contains("OPENAI_API_KEY"));
}

#[test]
#[serial(env_lock)]
fn mimo_provider_uses_mimo_api_key_env() {
    clear_managed_credentials_for_test();
    // provider=mimo 走通用规则即得 MIMO_API_KEY，无需为它写专门分支。
    assert_eq!(env_name_for_provider("mimo"), "MIMO_API_KEY");

    // SAFETY: 测试串行执行，作用域内临时写环境变量。
    unsafe {
        std::env::set_var("MIMO_API_KEY", "tp-secret");
        std::env::remove_var("OPENAI_API_KEY");
    }

    let store = AuthStore;
    let credential = store
        .get(&entry("mimo", None), Some("OPENAI_API_KEY"))
        .expect("应命中 MIMO_API_KEY");
    assert_eq!(credential.env_name, "MIMO_API_KEY");
    assert_eq!(credential.value, "tp-secret");

    unsafe {
        std::env::remove_var("MIMO_API_KEY");
    }
}
