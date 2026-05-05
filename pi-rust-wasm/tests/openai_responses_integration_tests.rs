//! 集成测试：`OpenAiResponsesProvider` 与真实 OpenAI Responses API（`POST /v1/responses`）。
//!
//! 不 Mock 网络；已配置 `OPENAI_API_KEY` 时真实发起 HTTP；无 key 时视为失败，不得 `ignore`。
//! 写法与 `tests/llm_tests.rs` 对齐：`mod common`、`dotenvy::dotenv`、`setup_logging`、60s 超时
//! （INTEGRATION_TEST_ROBUSTNESS 2.2）。

mod common;

use futures_util::StreamExt;
use pi_wasm::{ChatMessage, ChatRequest, LlmConfig, LlmProvider, OpenAiResponsesProvider};
use std::time::Duration;

fn responses_config() -> LlmConfig {
    LlmConfig {
        provider: "openai-responses".to_string(),
        ..LlmConfig::default()
    }
}

/// [Responses 非流式 chat] 真实 API 调用 `OpenAiResponsesProvider::chat` 返回合法响应
///
/// 验证：choices 非空、首条 index=0（超时 60s）
#[tokio::test]
async fn test_openai_responses_chat_real_request_returns_ok(
) -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = tracing::info_span!("test_openai_responses_chat_real_request_returns_ok").entered();
    let _ = dotenvy::dotenv().ok();

    let config = responses_config();
    let provider = OpenAiResponsesProvider::new(&config)
        .expect("集成测试要求设置 OPENAI_API_KEY（环境变量或 .env），无 key 视为失败");
    let request = ChatRequest {
        messages: vec![ChatMessage::user("Say exactly: ok")],
        model: config.default_model.clone(),
        temperature: Some(0.0),
        max_tokens: Some(16),
        stream: Some(false),
        model_override: None,
        tools: None,
    };
    tracing::info!(
        "Arrange: LlmConfig(provider=openai-responses)、OpenAiResponsesProvider、ChatRequest"
    );
    let resp = tokio::time::timeout(Duration::from_secs(60), provider.chat(request))
        .await
        .map_err(|_| "chat 超时 60s，可能网络或上游不可达")??;
    tracing::info!("Assert: choices 非空且首条 index 为 0");
    assert!(!resp.choices.is_empty(), "chat 响应应包含 choices");
    assert_eq!(resp.choices[0].index, 0);

    Ok(())
}

/// [Responses 流式 chat_stream] 真实 API 调用产生流式事件
///
/// 验证：至少产生一个 `StreamEvent`（超时 60s）
#[tokio::test]
async fn test_openai_responses_chat_stream_real_request_yields_events(
) -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = tracing::info_span!("test_openai_responses_chat_stream_real_request_yields_events")
        .entered();
    let _ = dotenvy::dotenv().ok();

    let config = responses_config();
    let provider = OpenAiResponsesProvider::new(&config)
        .expect("集成测试要求设置 OPENAI_API_KEY（环境变量或 .env），无 key 视为失败");
    let request = ChatRequest {
        messages: vec![ChatMessage::user("Say hi")],
        model: config.default_model.clone(),
        temperature: Some(0.0),
        max_tokens: Some(16),
        stream: Some(true),
        model_override: None,
        tools: None,
    };
    tracing::info!("Arrange: ChatRequest(stream=true) → Responses SSE");
    let mut stream = tokio::time::timeout(Duration::from_secs(60), async move {
        provider.chat_stream(request).await
    })
    .await
    .map_err(|_| "chat_stream 超时 60s，可能网络或上游不可达")??;

    let mut events = Vec::new();
    while let Some(item) = stream.next().await {
        events.push(item?);
    }
    tracing::info!("Assert: 至少产生一个 StreamEvent");
    assert!(
        !events.is_empty(),
        "chat_stream 应至少产生一个 StreamEvent（ContentDelta 或 FinishReason）"
    );

    Ok(())
}
