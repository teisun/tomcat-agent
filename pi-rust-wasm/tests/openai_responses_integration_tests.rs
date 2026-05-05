//! 集成测试：OpenAI Responses 适配器与真实 API（`POST /v1/responses`）。
//!
//! 不 Mock 网络；已配置 `OPENAI_API_KEY` 时真实发起 HTTP；无 key 时视为失败，不得 `ignore`。
//! 写法与 `tests/llm_tests.rs` 对齐：`mod common`、`dotenvy::dotenv`、`setup_logging`、60s 超时
//! （INTEGRATION_TEST_ROBUSTNESS 2.2）。
//!
//! 调用面：通过 [`pi_wasm::resolve_llm`] 拿 `Arc<dyn LlmProvider>`，**不直接构造**
//! 任何 concrete Provider 类型；`provider = "openai-responses"` 即可路由到
//! Responses 适配器（实现细节由 `core/llm/registry.rs` 单点维护）。

mod common;

use futures_util::StreamExt;
use pi_wasm::{
    resolve_llm, ChatMessage, ChatMessageContentPart, ChatRequest, LlmConfig, IMAGE_MAX_BYTES,
};
use std::time::Duration;

/// Sample puppy PNG (≈ 46 KB), base64 字面量；fixture 详见
/// [`tests/fixtures/llm_multimodal/README.md`](tests/fixtures/llm_multimodal/README.md)。
const SAMPLE_IMAGE_B64: &str = include_str!("fixtures/llm_multimodal/sample_image_b64.txt");
/// Sample one-page PDF（reportlab 生成，含字符串 "Hello PDF content for LLM summarize test"），base64 字面量。
const SAMPLE_PDF_B64: &str = include_str!("fixtures/llm_multimodal/sample_pdf_b64.txt");

fn responses_config() -> LlmConfig {
    LlmConfig {
        provider: "openai-responses".to_string(),
        ..LlmConfig::default()
    }
}

/// [Responses 非流式 chat] 真实 API 调用返回合法响应
///
/// 验证：choices 非空、首条 index=0（超时 60s）
#[tokio::test]
async fn test_openai_responses_chat_real_request_returns_ok(
) -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = tracing::info_span!("test_openai_responses_chat_real_request_returns_ok").entered();
    let _ = dotenvy::dotenv().ok();

    let config = responses_config();
    let provider = resolve_llm(&config)
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
        "Arrange: LlmConfig(provider=openai-responses) → resolve_llm → Arc<dyn LlmProvider>"
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
    let provider = resolve_llm(&config)
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

/// [Responses 多模态 inline image] 真 API roundtrip：发一张小狗 PNG，让模型描述图片内容
///
/// 模型要求：沿用 `LlmConfig.default_model`（当前 `gpt-5.2`，已确认支持 vision）；
/// 若未来默认模型不支持 vision，本测试会以 API 4xx 暴露问题，不静默跳过。
///
/// 验证：HTTP 200 + 响应文本非空 + 至少命中关键词 [dog/puppy/animal/pet/canine]
/// 之一（容忍 LLM 输出口径漂移），超时 60s。
#[tokio::test]
async fn responses_inline_image_describe_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = tracing::info_span!("responses_inline_image_describe_roundtrip").entered();
    let _ = dotenvy::dotenv().ok();

    let config = responses_config();
    let provider = resolve_llm(&config)
        .expect("集成测试要求设置 OPENAI_API_KEY（环境变量或 .env），无 key 视为失败");

    let image_b64 = SAMPLE_IMAGE_B64.trim();
    let parts = vec![
        ChatMessageContentPart::text("Describe what you see in this image in one short sentence."),
        ChatMessageContentPart::image_b64("image/png", image_b64.to_string())?,
    ];
    let request = ChatRequest {
        messages: vec![ChatMessage::user_with_parts(parts)],
        // 不显式指定 model，沿用 LlmConfig.default_model（当前 gpt-5.2，支持 vision）
        model: config.default_model.clone(),
        temperature: Some(0.0),
        max_tokens: Some(96),
        stream: Some(false),
        model_override: None,
        tools: None,
    };
    tracing::info!(
        image_b64_len = image_b64.len(),
        "Arrange: image/png inline base64 → /v1/responses input_image"
    );

    let resp = tokio::time::timeout(Duration::from_secs(60), provider.chat(request))
        .await
        .map_err(|_| "vision chat 超时 60s")??;
    assert!(!resp.choices.is_empty(), "vision 响应应包含 choices");
    let text = resp.choices[0]
        .message
        .text_content()
        .unwrap_or("")
        .to_ascii_lowercase();
    tracing::info!(response = %text, "Vision 响应内容");
    assert!(
        !text.trim().is_empty(),
        "vision 响应文本不应为空: {:?}",
        text
    );

    // 关键词集允许通用词（dog / puppy / animal / pet / canine）与常见品种词
    // （beagle / labrador / retriever / terrier / shepherd / poodle / bulldog / corgi / husky）；
    // 任一命中即视为 vision 正常工作，避免 LLM 把品种名当主语时（"a happy beagle ..."）漏判。
    let keywords = [
        "dog",
        "puppy",
        "animal",
        "pet",
        "canine",
        "beagle",
        "labrador",
        "retriever",
        "terrier",
        "shepherd",
        "poodle",
        "bulldog",
        "corgi",
        "husky",
    ];
    let hit = keywords.iter().any(|kw| text.contains(kw));
    assert!(
        hit,
        "vision 响应应至少命中关键词 {:?} 之一（容忍 LLM 输出漂移），实际: {:?}",
        keywords, text
    );
    Ok(())
}

/// [Responses 多模态 inline PDF] 真 API roundtrip：发一份 reportlab 生成的 PDF，
/// 让模型总结其内容。
///
/// 模型要求：沿用 `LlmConfig.default_model`（当前 `gpt-5.2`，已确认支持 input_file）；
/// 若未来默认模型不支持 input_file，本测试会以 API 4xx 暴露问题，不静默跳过。
///
/// 验证：HTTP 200 + 响应文本非空 + 至少命中关键词 [hello/pdf/summary/summarize/test]
/// 之一（容忍 LLM 输出口径漂移），超时 60s。
#[tokio::test]
async fn responses_inline_pdf_input_file_summarize_roundtrip(
) -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span =
        tracing::info_span!("responses_inline_pdf_input_file_summarize_roundtrip").entered();
    let _ = dotenvy::dotenv().ok();

    let config = responses_config();
    let provider = resolve_llm(&config)
        .expect("集成测试要求设置 OPENAI_API_KEY（环境变量或 .env），无 key 视为失败");

    let pdf_b64 = SAMPLE_PDF_B64.trim();
    let parts = vec![
        ChatMessageContentPart::text("Summarize the attached PDF in one short sentence."),
        ChatMessageContentPart::file_b64("sample.pdf", "application/pdf", pdf_b64.to_string())?,
    ];
    let request = ChatRequest {
        messages: vec![ChatMessage::user_with_parts(parts)],
        // 不显式指定 model，沿用 LlmConfig.default_model（当前 gpt-5.2，支持 input_file）
        model: config.default_model.clone(),
        temperature: Some(0.0),
        max_tokens: Some(96),
        stream: Some(false),
        model_override: None,
        tools: None,
    };
    tracing::info!(
        pdf_b64_len = pdf_b64.len(),
        "Arrange: application/pdf inline base64 → /v1/responses input_file"
    );

    let resp = tokio::time::timeout(Duration::from_secs(60), provider.chat(request))
        .await
        .map_err(|_| "PDF chat 超时 60s")??;
    assert!(!resp.choices.is_empty(), "PDF 响应应包含 choices");
    let text = resp.choices[0]
        .message
        .text_content()
        .unwrap_or("")
        .to_ascii_lowercase();
    tracing::info!(response = %text, "PDF 响应内容");
    assert!(!text.trim().is_empty(), "PDF 响应文本不应为空: {:?}", text);

    let keywords = ["hello", "pdf", "summary", "summarize", "test"];
    let hit = keywords.iter().any(|kw| text.contains(kw));
    assert!(
        hit,
        "PDF 响应应至少命中关键词 {:?} 之一（容忍 LLM 输出漂移），实际: {:?}",
        keywords, text
    );
    Ok(())
}

/// [本地 helper 校验] inline image 超 IMAGE_MAX_BYTES 必须立即结构化报错，无需联真 API。
///
/// 与上面两个 roundtrip 不同，本用例**不依赖 OPENAI_API_KEY**：纯本地构造伪图字节
/// 走 `image_b64` helper，断言返回 `AppError::Llm(_)` 且文案含「IMAGE_MAX_BYTES」。
#[test]
fn responses_inline_image_b64_helper_rejects_oversize() {
    let oversize = vec![0u8; IMAGE_MAX_BYTES + 1];
    let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &oversize);
    let err = ChatMessageContentPart::image_b64("image/png", b64)
        .expect_err("超 IMAGE_MAX_BYTES 应返回结构化错误");
    let s = err.to_string();
    assert!(
        s.contains("IMAGE_MAX_BYTES"),
        "错误文案应含 IMAGE_MAX_BYTES，实际: {}",
        s
    );
}
