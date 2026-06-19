//! # `OpenAiProvider` 行为焦小测
//!
//! 覆盖：
//!
//! - `OpenAiProvider::new`：缺 API key 报错；有 API key 成功创建。
//! - `count_tokens`：基于 tokenizer 的近似估算落在合理范围。
//! - `OpenAiProvider::is_retriable`：429 / 5xx 重试，4xx（非限流）不重试，
//!   非 LLM 错误一律不重试。
//! - `chat_real_request_response_print`：`#[ignore]` 真实 API 冒烟。

use super::*;
use crate::core::llm::tests::mocks::load_dotenv;
use crate::core::llm::types::{ChatMessage, ChatMessageContentPart, ChatRequest};
use crate::core::llm::{Capabilities, Credential, ModelEntry};
use crate::infra::error::{llm_http_status_error, AppError};
use crate::infra::LlmConfig;

fn deepseek_entry(api_key_env: &str) -> ModelEntry {
    ModelEntry {
        id: "deepseek-v4-pro".to_string(),
        model_name: None,
        api: "openai".to_string(),
        provider: "deepseek".to_string(),
        api_key_env: Some(api_key_env.to_string()),
        base_url: Some("https://api.deepseek.com".to_string()),
        capabilities: Capabilities::default(),
        context_window: None,
        cost: None,
        thinking_format: Some("deepseek".to_string()),
    }
}

#[test]
fn openai_provider_new_uses_supplied_credential_without_env_lookup() {
    let entry = deepseek_entry("TOMCAT_TEST_NONEXISTENT_ENV_VAR_12345");
    let runtime = LlmConfig::default().runtime();
    let credential = Credential {
        provider: "deepseek".to_string(),
        env_name: "TOMCAT_TEST_NONEXISTENT_ENV_VAR_12345".to_string(),
        value: "stub-key".to_string(),
    };
    let provider =
        OpenAiProvider::new(&entry, &runtime, &credential).expect("显式 credential 应可直接构造");
    assert_eq!(provider.provider_name(), "openai");
}

#[test]
fn openai_provider_new_succeeds_with_api_key() {
    let entry = deepseek_entry("DEEPSEEK_API_KEY");
    let runtime = LlmConfig::default().runtime();
    let credential = Credential {
        provider: "deepseek".to_string(),
        env_name: "DEEPSEEK_API_KEY".to_string(),
        value: "stub-key".to_string(),
    };
    let provider =
        OpenAiProvider::new(&entry, &runtime, &credential).expect("显式 credential 应创建成功");
    assert_eq!(provider.provider_name(), "openai");
}

#[test]
fn openai_provider_effective_model_maps_catalog_id_to_model_name() {
    let mut entry = deepseek_entry("LITELLM_SUNMI_API_KEY");
    entry.id = "gpt-5.4_litellm-sunmi".to_string();
    entry.model_name = Some("gpt-5.4".to_string());
    entry.provider = "litellm-sunmi".to_string();
    entry.base_url = Some("https://aigateway.sunmi.com".to_string());
    entry.thinking_format = Some("openai".to_string());
    let runtime = LlmConfig::default().runtime();
    let credential = Credential {
        provider: "litellm-sunmi".to_string(),
        env_name: "LITELLM_SUNMI_API_KEY".to_string(),
        value: "stub-key".to_string(),
    };
    let provider =
        OpenAiProvider::new(&entry, &runtime, &credential).expect("显式 credential 应创建成功");
    let request = ChatRequest {
        messages: vec![ChatMessage::user("Say exactly: ok")],
        // 这里故意传 catalog id，覆盖“调用方仍给本地 id 时 provider 必须 remap 到 wire model_name”。
        model: "gpt-5.4_litellm-sunmi".to_string(),
        temperature: Some(0.0),
        max_tokens: Some(10),
        stream: Some(false),
        model_override: None,
        tools: None,
    };
    assert_eq!(provider.effective_model(&request), "gpt-5.4");
}

#[test]
fn count_tokens_approximate() {
    let entry = deepseek_entry("DEEPSEEK_API_KEY");
    let runtime = LlmConfig::default().runtime();
    let credential = Credential {
        provider: "deepseek".to_string(),
        env_name: "DEEPSEEK_API_KEY".to_string(),
        value: "stub-key".to_string(),
    };
    let provider =
        OpenAiProvider::new(&entry, &runtime, &credential).expect("显式 credential 应创建成功");
    let messages = vec![
        ChatMessage::user("hello world"),
        ChatMessage::assistant("hi there"),
    ];
    let n = provider.count_tokens(&messages).unwrap();
    assert!(n >= 1, "count_tokens 应至少为 1");
    assert!(n <= 20, "count_tokens 近似值应在合理范围");
}

#[test]
fn is_retriable_detects_429_and_5xx() {
    assert!(OpenAiProvider::is_retriable(&llm_http_status_error(
        "openai",
        429,
        "rate limit",
    )));
    assert!(OpenAiProvider::is_retriable(&llm_http_status_error(
        "openai",
        502,
        "bad gateway",
    )));
    assert!(!OpenAiProvider::is_retriable(&llm_http_status_error(
        "openai",
        400,
        "bad request",
    )));
}

#[test]
fn is_retriable_returns_false_for_non_llm_error() {
    assert!(!OpenAiProvider::is_retriable(&AppError::Config(
        "config error".to_string()
    )));
}

/// Completions 路径不支持多模态附件：含非 InputText part 的 messages 必须立刻拒绝，
/// 错误文案必须把诊断指向 `provider=openai-responses` 以引导调用方迁移；并且要
/// **不可重试**（不被 `is_retriable` 命中），避免在 Agent Loop 的退避循环里反复打。
#[test]
fn parts_with_image_returns_structured_error() {
    use base64::Engine;
    const TINY_PNG_B64: &str =
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=";
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(TINY_PNG_B64)
        .unwrap();
    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    std::io::Write::write_all(&mut tmp, &bytes).unwrap();
    let part = ChatMessageContentPart::image_b64("image/png", tmp.path()).expect("image_b64 ok");
    let msgs = vec![ChatMessage::user_with_parts(vec![
        ChatMessageContentPart::text("see this:"),
        part,
    ])];
    let err = reject_multimodal_parts(&msgs).expect_err("应拒绝多模态 part");
    let s = err.to_string();
    assert!(
        s.contains("openai-responses"),
        "错误文案应引导改用 openai-responses，实际: {}",
        s
    );
    assert!(
        s.contains("不支持多模态附件"),
        "错误文案应说明拒绝原因，实际: {}",
        s
    );
    assert!(
        !OpenAiProvider::is_retriable(&err),
        "多模态拒绝错误必须是不可重试的"
    );
}

/// 依赖 DEEPSEEK_API_KEY 与可用配额：有 key 时调用真实 chat 接口一次，打印请求与响应；无 key 时 panic。
#[tokio::test]
#[ignore = "依赖真实 DeepSeek API 与配额，CI 默认跳过"]
async fn chat_real_request_response_print() {
    load_dotenv();
    let api_key = match std::env::var("DEEPSEEK_API_KEY") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => {
            panic!("DEEPSEEK_API_KEY 未配置，本用例不通过（宪法与单测规范：无 key 不得跳过）");
        }
    };

    let entry = deepseek_entry("DEEPSEEK_API_KEY");
    let runtime = LlmConfig::default().runtime();
    let credential = Credential {
        provider: "deepseek".to_string(),
        env_name: "DEEPSEEK_API_KEY".to_string(),
        value: api_key,
    };
    let provider = OpenAiProvider::new(&entry, &runtime, &credential)
        .expect("DEEPSEEK_API_KEY 已设置时应创建成功");
    let request = ChatRequest {
        messages: vec![ChatMessage::user("Say exactly: ok")],
        model: entry.request_model_name().to_string(),
        temperature: Some(0.0),
        max_tokens: Some(10),
        stream: Some(false),
        model_override: None,
        tools: None,
    };

    match provider.chat(request).await {
        Ok(resp) => {
            println!("id: {:?}", resp.id);
        }
        Err(e) => {
            panic!(
                "chat 请求失败: {}（请在本机终端运行 cargo test，并确认可访问 api.deepseek.com 且已配置 DEEPSEEK_API_KEY）",
                e
            );
        }
    }
}
