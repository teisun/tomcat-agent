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
use crate::core::llm::multimodal::{
    UNSUPPORTED_FILE_INPUT_PLACEHOLDER, UNSUPPORTED_IMAGE_INPUT_PLACEHOLDER,
};
use crate::core::llm::tests::mocks::load_dotenv;
use crate::core::llm::types::{
    ChatMessage, ChatMessageContent, ChatMessageContentPart, ChatRequest, ContextReference,
};
use crate::core::llm::{
    thinking_policy::resolve_request_fields, Capabilities, Credential, ModelEntry, ThinkingLevel,
};
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

fn openai_entry(api_key_env: &str) -> ModelEntry {
    ModelEntry {
        id: "gpt-5.4".to_string(),
        model_name: None,
        api: "openai".to_string(),
        provider: "openai".to_string(),
        api_key_env: Some(api_key_env.to_string()),
        base_url: Some("https://api.openai.com".to_string()),
        capabilities: Capabilities::default(),
        context_window: None,
        cost: None,
        thinking_format: Some("openai".to_string()),
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
        thinking_level: None,
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

/// Completions 路径是纯文本通道：历史里的图片 / 文件附件应降级为占位符文本，
/// 而不是把整轮请求硬拒绝。
#[test]
fn parts_with_image_degrade_to_placeholder_text() {
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
        ChatMessageContentPart::text("see this: "),
        part,
    ])];
    let normalized = normalize_for_completions(&msgs);
    let normalized = normalized.as_ref();
    let expected = format!("see this: {UNSUPPORTED_IMAGE_INPUT_PLACEHOLDER}");
    assert!(matches!(
        &normalized[0].content,
        Some(ChatMessageContent::Text(text))
            if text == &expected
    ));
}

#[test]
fn normalize_for_completions_degrades_mixed_image_and_file_history() {
    let msgs = vec![
        ChatMessage::user_with_parts(vec![
            ChatMessageContentPart::text("image "),
            ChatMessageContentPart::image_file_id("file-image").unwrap(),
        ]),
        ChatMessage::user_with_parts(vec![
            ChatMessageContentPart::text("file "),
            ChatMessageContentPart::file_file_id("file-pdf", Some("guide.pdf".to_string()))
                .unwrap(),
        ]),
    ];

    let normalized = normalize_for_completions(&msgs);
    let normalized = normalized.as_ref();
    let expected_image = format!("image {UNSUPPORTED_IMAGE_INPUT_PLACEHOLDER}");
    let expected_file = format!("file {UNSUPPORTED_FILE_INPUT_PLACEHOLDER}");
    assert_eq!(normalized.len(), 2);
    assert!(matches!(
        &normalized[0].content,
        Some(ChatMessageContent::Text(text))
            if text == &expected_image
    ));
    assert!(matches!(
        &normalized[1].content,
        Some(ChatMessageContent::Text(text))
            if text == &expected_file
    ));
}

#[test]
fn normalize_for_completions_flattens_references_into_text() {
    let msgs = vec![ChatMessage::user_with_parts(vec![
        ChatMessageContentPart::text("before "),
        ChatMessageContentPart::reference(ContextReference::selection(
            "src/lib.rs",
            "lib.rs:10-12",
            Some(10),
            Some(12),
            Some("fn hello() {}".to_string()),
        )),
        ChatMessageContentPart::text(" after"),
    ])];
    let normalized = normalize_for_completions(&msgs);
    let normalized = normalized.as_ref();
    assert_eq!(normalized.len(), 1);
    assert!(matches!(
        &normalized[0].content,
        Some(ChatMessageContent::Text(text))
            if text
                == "before <selection file=\"src/lib.rs\" lines=\"10-12\">\nfn hello() {}\n</selection> after"
    ));
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
        thinking_level: None,
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

#[test]
fn thinking_level_override_updates_openai_reasoning_effort() {
    let entry = openai_entry("OPENAI_API_KEY");
    let runtime = LlmConfig::default().runtime();
    let credential = Credential {
        provider: "openai".to_string(),
        env_name: "OPENAI_API_KEY".to_string(),
        value: "stub-key".to_string(),
    };
    let provider = OpenAiProvider::new(&entry, &runtime, &credential).unwrap();
    let request = ChatRequest {
        messages: vec![ChatMessage::user("hello")],
        model: entry.request_model_name().to_string(),
        temperature: None,
        max_tokens: None,
        stream: Some(false),
        model_override: None,
        thinking_level: Some(ThinkingLevel::Low),
        tools: None,
    };

    let cfg = provider.thinking_cfg_for_request(&request);
    let fields = resolve_request_fields(
        &cfg,
        provider.thinking_format_for_model(&provider.effective_model(&request)),
    );

    assert_eq!(cfg.level, "low");
    assert_eq!(fields.reasoning_effort.as_deref(), Some("low"));
}

#[test]
fn thinking_level_override_updates_deepseek_reasoning_effort() {
    let entry = deepseek_entry("DEEPSEEK_API_KEY");
    let runtime = LlmConfig::default().runtime();
    let credential = Credential {
        provider: "deepseek".to_string(),
        env_name: "DEEPSEEK_API_KEY".to_string(),
        value: "stub-key".to_string(),
    };
    let provider = OpenAiProvider::new(&entry, &runtime, &credential).unwrap();
    let request = ChatRequest {
        messages: vec![ChatMessage::user("hello")],
        model: entry.request_model_name().to_string(),
        temperature: None,
        max_tokens: None,
        stream: Some(false),
        model_override: None,
        thinking_level: Some(ThinkingLevel::Xhigh),
        tools: None,
    };

    let cfg = provider.thinking_cfg_for_request(&request);
    let fields = resolve_request_fields(
        &cfg,
        provider.thinking_format_for_model(&provider.effective_model(&request)),
    );

    assert_eq!(cfg.level, "xhigh");
    assert_eq!(fields.reasoning_effort.as_deref(), Some("max"));
}
