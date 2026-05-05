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
use crate::core::llm::types::{ChatMessage, ChatRequest};
use crate::infra::error::AppError;
use crate::infra::LlmConfig;

#[test]
fn openai_provider_new_fails_without_api_key() {
    println!("[TEST] openai_provider_new_fails_without_api_key — 开始");
    let config = LlmConfig {
        api_key_env: Some("PI_WASM_TEST_NONEXISTENT_ENV_VAR_12345".to_string()),
        ..LlmConfig::default()
    };
    let r = OpenAiProvider::new(&config);
    assert!(r.is_err());
    let msg = r.unwrap_err().to_string();
    assert!(msg.contains("未设置"));
}

#[test]
fn openai_provider_new_succeeds_with_api_key() {
    load_dotenv();
    if std::env::var("OPENAI_API_KEY").is_err() {
        panic!("OPENAI_API_KEY 未配置，本用例不通过（宪法与单测规范：无 key 不得跳过）");
    }

    let config = LlmConfig::default();
    let provider = OpenAiProvider::new(&config).expect("OPENAI_API_KEY 已设置时应创建成功");
    assert_eq!(provider.provider_name(), "openai");
}

#[test]
fn count_tokens_approximate() {
    load_dotenv();
    if std::env::var("OPENAI_API_KEY").is_err() {
        panic!("OPENAI_API_KEY 未配置，本用例不通过（宪法与单测规范：无 key 不得跳过）");
    }

    let config = LlmConfig::default();
    let provider = OpenAiProvider::new(&config).expect("OPENAI_API_KEY 已设置时应创建成功");
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
    assert!(OpenAiProvider::is_retriable(&AppError::Llm(
        "API 错误 429: rate limit".to_string()
    )));
    assert!(OpenAiProvider::is_retriable(&AppError::Llm(
        "API 错误 502: bad gateway".to_string()
    )));
    assert!(!OpenAiProvider::is_retriable(&AppError::Llm(
        "API 错误 400: bad request".to_string()
    )));
}

#[test]
fn is_retriable_returns_false_for_non_llm_error() {
    assert!(!OpenAiProvider::is_retriable(&AppError::Config(
        "config error".to_string()
    )));
}

/// 依赖 OPENAI_API_KEY 与可用配额：有 key 时调用真实 chat 接口一次，打印请求与响应；无 key 时 panic。
#[tokio::test]
#[ignore = "依赖真实 OpenAI API 与配额，CI 默认跳过"]
async fn chat_real_request_response_print() {
    load_dotenv();
    if std::env::var("OPENAI_API_KEY").is_err() {
        panic!("OPENAI_API_KEY 未配置，本用例不通过（宪法与单测规范：无 key 不得跳过）");
    }

    let config = LlmConfig::default();
    let provider = OpenAiProvider::new(&config).expect("OPENAI_API_KEY 已设置时应创建成功");
    let request = ChatRequest {
        messages: vec![ChatMessage::user("Say exactly: ok")],
        model: config.default_model.clone(),
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
                "chat 请求失败: {}（请在本机终端运行 cargo test，并确认可访问 api.openai.com 且已配置 OPENAI_API_KEY）",
                e
            );
        }
    }
}
