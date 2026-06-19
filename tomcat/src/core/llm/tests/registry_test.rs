//! # Provider 注册表焦小测
//!
//! 覆盖（plan §5 Phase E.1）：
//!
//! - `build_provider`：未知 api → [`AppError::Config`] 且消息列出已注册 id；
//! - `api = "openai-responses"` → `provider_name == "openai-responses"`；
//! - `api = "openai"` → Completions 适配器。

use super::super::registry::build_provider;
use crate::core::llm::{Capabilities, Credential, ModelEntry};
use crate::infra::error::AppError;
use crate::infra::LlmConfig;

const REGISTRY_STUB_ENV: &str = "__PI_REGISTRY_STUB_OPENAI_KEY__";

fn entry_with_api(api: &str) -> ModelEntry {
    ModelEntry {
        id: format!("test-{api}"),
        model_name: None,
        api: api.to_string(),
        provider: "openai".to_string(),
        api_key_env: Some(REGISTRY_STUB_ENV.to_string()),
        base_url: Some("https://api.openai.com".to_string()),
        capabilities: Capabilities::default(),
        context_window: None,
        cost: None,
        thinking_format: Some("openai".to_string()),
    }
}

fn stub_credential() -> Credential {
    Credential {
        provider: "openai".to_string(),
        env_name: REGISTRY_STUB_ENV.to_string(),
        value: "stub".to_string(),
    }
}

#[test]
fn build_provider_unknown_api_returns_config_error_listing_ids() {
    let cfg = LlmConfig::default();
    let runtime = cfg.runtime();
    let entry = entry_with_api("claude");
    let err = match build_provider(&entry, &runtime, &stub_credential()) {
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
fn build_provider_openai_responses_returns_responses_impl() {
    let cfg = LlmConfig::default();
    let runtime = cfg.runtime();
    let entry = entry_with_api("openai-responses");
    let llm = build_provider(&entry, &runtime, &stub_credential()).expect("build provider");
    assert_eq!(llm.provider_name(), "openai-responses");
}

#[test]
fn build_provider_openai_returns_completions() {
    let cfg = LlmConfig::default();
    let runtime = cfg.runtime();
    let entry = entry_with_api("openai");
    let llm = build_provider(&entry, &runtime, &stub_credential()).expect("build provider");
    assert_eq!(llm.provider_name(), "openai");
}
