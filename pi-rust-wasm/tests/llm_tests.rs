//! 集成测试：LLM 与真实外部 API 的协作（chat / chat_stream）。
//! 不 Mock 网络，在配置 OPENAI_API_KEY 时真实发起 HTTP 请求；无 key 时视为失败，不得 ignore。
//! 鲁棒性：异步用例均包裹在超时内，避免依赖挂起导致测试挂起（INTEGRATION_TEST_ROBUSTNESS 2.2）。
//!
//! 调用面：所有 Provider 通过 [`pi_wasm::resolve_llm`] 拿 `Arc<dyn LlmProvider>`，
//! 不直接构造 concrete 类型——这是与「`registry.rs` 单一注册入口」对齐的对外契约。

mod common;

use futures_util::StreamExt;
use pi_wasm::{resolve_llm, ChatMessage, ChatRequest, LlmConfig};
use std::time::Duration;

fn completions_config() -> LlmConfig {
    LlmConfig {
        provider: "openai".to_string(),
        ..LlmConfig::default()
    }
}

/// [LLM 非流式 chat] 真实 API 调用 OpenAI Chat Completions 返回合法响应
///
/// 验证：choices 非空、首条 index=0（超时 60s）
/// 意义：TASK-05 LLM 端到端——非流式请求正向路径；无 OPENAI_API_KEY 时用例必须失败（INTEGRATION_TEST_SPEC）
#[tokio::test]
async fn test_llm_provider_chat_real_request_returns_ok() -> Result<(), Box<dyn std::error::Error>>
{
    common::setup_logging();
    let _span = tracing::info_span!("test_llm_provider_chat_real_request_returns_ok").entered();
    let _ = dotenvy::dotenv().ok();

    let config = completions_config();
    let provider = resolve_llm(&config)
        .expect("集成测试要求设置 OPENAI_API_KEY（环境变量或 .env），无 key 视为失败");
    let request = ChatRequest {
        messages: vec![ChatMessage::user("Say exactly: ok")],
        model: config.default_model.clone(),
        temperature: None,
        max_tokens: Some(10),
        stream: Some(false),
        model_override: None,
        tools: None,
    };
    tracing::info!("Arrange: 加载 .env，resolve_llm(provider=openai) 拿 Arc<dyn LlmProvider>");
    let resp = tokio::time::timeout(Duration::from_secs(60), provider.chat(request))
        .await
        .map_err(|_| "chat 超时 60s，可能网络或上游不可达")??;
    tracing::info!("Act: 调用 provider.chat(request)");
    tracing::info!("Assert: 验证 choices 非空且首条 index 为 0");
    assert!(!resp.choices.is_empty(), "chat 响应应包含 choices");
    assert_eq!(resp.choices[0].index, 0);

    Ok(())
}

/// [LLM 流式 chat_stream] 真实 API 调用产生流式事件
///
/// 验证：stream 至少产生一个 StreamEvent（超时 60s）
/// 意义：TASK-05 LLM 端到端——流式请求正向路径；无 OPENAI_API_KEY 时用例必须失败
#[tokio::test]
async fn test_llm_provider_chat_stream_real_request_yields_events(
) -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span =
        tracing::info_span!("test_llm_provider_chat_stream_real_request_yields_events").entered();
    let _ = dotenvy::dotenv().ok();

    let config = completions_config();
    let provider = resolve_llm(&config)
        .expect("集成测试要求设置 OPENAI_API_KEY（环境变量或 .env），无 key 视为失败");
    let request = ChatRequest {
        messages: vec![ChatMessage::user("Say hi")],
        model: config.default_model.clone(),
        temperature: None,
        max_tokens: Some(5),
        stream: Some(true),
        model_override: None,
        tools: None,
    };
    tracing::info!("Arrange: ChatRequest(stream=true)");
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
    assert!(!events.is_empty(), "chat_stream 应至少产生一个 StreamEvent");

    Ok(())
}
