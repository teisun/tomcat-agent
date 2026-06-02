use serial_test::serial;

use crate::core::llm::{env_name_for_provider, missing_key_message, AuthStore};

#[test]
#[serial(env_lock)]
fn per_provider_env_prefers_provider_specific_api_key() {
    // SAFETY: 测试串行执行，且在本用例作用域内临时写环境变量。
    unsafe {
        std::env::set_var("DEEPSEEK_API_KEY", "deepseek-secret");
        std::env::remove_var("OPENAI_API_KEY");
    }

    let store = AuthStore;
    let credential = store
        .get("deepseek", Some("OPENAI_API_KEY"))
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
fn missing_key_message_mentions_expected_env() {
    let msg = missing_key_message("deepseek", "DEEPSEEK_API_KEY", Some("OPENAI_API_KEY"));
    assert!(msg.contains("DEEPSEEK_API_KEY"));
    assert!(msg.contains("OPENAI_API_KEY"));
}
