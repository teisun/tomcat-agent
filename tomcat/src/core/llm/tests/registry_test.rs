//! # Provider 注册表焦小测
//!
//! 覆盖（plan §5 Phase E.1）：
//!
//! - `resolve_llm`：未知 provider → [`AppError::Config`] 且消息列出已注册 id；
//! - `resolve_llm(&LlmConfig::default())` → `provider_name == "openai-responses"`（需 stub API key）；
//! - `provider = "openai"` → Completions 适配器。

use super::super::registry::resolve_llm;
use super::mocks::load_dotenv;
use crate::infra::error::AppError;
use crate::infra::LlmConfig;

const REGISTRY_STUB_ENV: &str = "__PI_REGISTRY_STUB_OPENAI_KEY__";

#[test]
fn resolve_llm_unknown_provider_returns_config_error_listing_ids() {
    let cfg = LlmConfig {
        provider: "claude".to_string(),
        ..LlmConfig::default()
    };
    let err = match resolve_llm(&cfg) {
        Err(e) => e,
        Ok(_) => panic!("expected unknown provider error"),
    };
    let msg = err.to_string();
    assert!(
        matches!(err, AppError::Config(_)),
        "expected Config error, got {:?}",
        err
    );
    assert!(msg.contains("openai"));
    assert!(msg.contains("openai-responses"));
}

#[test]
fn resolve_llm_default_returns_openai_responses() {
    load_dotenv();
    // SAFETY: 单测串行；临时 stub env。
    unsafe { std::env::set_var(REGISTRY_STUB_ENV, "stub") };
    let cfg = LlmConfig {
        api_key_env: Some(REGISTRY_STUB_ENV.to_string()),
        ..LlmConfig::default()
    };
    let llm = resolve_llm(&cfg).expect("resolve default");
    assert_eq!(llm.provider_name(), "openai-responses");
    unsafe { std::env::remove_var(REGISTRY_STUB_ENV) };
}

#[test]
fn resolve_llm_explicit_openai_returns_completions() {
    load_dotenv();
    unsafe { std::env::set_var(REGISTRY_STUB_ENV, "stub") };
    let cfg = LlmConfig {
        provider: "openai".to_string(),
        api_key_env: Some(REGISTRY_STUB_ENV.to_string()),
        ..LlmConfig::default()
    };
    let llm = resolve_llm(&cfg).expect("resolve openai");
    assert_eq!(llm.provider_name(), "openai");
    unsafe { std::env::remove_var(REGISTRY_STUB_ENV) };
}
