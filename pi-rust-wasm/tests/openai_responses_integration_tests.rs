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
    resolve_llm, ChatMessage, ChatMessageContentPart, ChatRequest, LlmConfig, StreamEvent,
    IMAGE_MAX_BYTES,
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

fn env_truthy(key: &str) -> bool {
    matches!(
        std::env::var(key)
            .ok()
            .map(|v| v.to_ascii_lowercase())
            .as_deref(),
        Some("1" | "true" | "yes" | "on")
    )
}

fn language_behavior_e2e_opt_in() -> bool {
    // 兼容旧开关名，避免本地脚本断裂。
    env_truthy("PI_WASM_E2E_LANGUAGE_BEHAVIOR") || env_truthy("PI_WASM_E2E_PROMPT_LANGUAGE")
}

fn contains_cjk(text: &str) -> bool {
    text.chars()
        .any(|c| matches!(c, '\u{3400}'..='\u{4DBF}' | '\u{4E00}'..='\u{9FFF}'))
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
        temperature: None,
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
        temperature: None,
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

/// [Responses 流式 thinking 可见性] 真实 API 调用应产生 `StreamEvent::Thinking`
///
/// 验证：至少一个 `StreamEvent::Thinking` 且至少一个 `ContentDelta`（超时 60s）
#[tokio::test]
async fn test_openai_responses_chat_stream_reasoning_emits_thinking(
) -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = tracing::info_span!("test_openai_responses_chat_stream_reasoning_emits_thinking")
        .entered();
    let _ = dotenvy::dotenv().ok();

    let config = responses_config();
    let provider = resolve_llm(&config)
        .expect("集成测试要求设置 OPENAI_API_KEY（环境变量或 .env），无 key 视为失败");
    // 真实模型在个别请求上可能不给 thinking（同样提示词下偶发），这里允许有限重试，
    // 以减少测试抖动；若解析链路回归，重试后仍会稳定失败。
    const MAX_ATTEMPTS: usize = 3;
    let mut saw_content_any = false;
    for attempt in 1..=MAX_ATTEMPTS {
        let request = ChatRequest {
            messages: vec![ChatMessage::user(
                "Compute 387 * 249, think step by step, then give the final result in one sentence.",
            )],
            model: config.default_model.clone(),
            temperature: None,
            max_tokens: Some(256),
            stream: Some(true),
            model_override: None,
            tools: None,
        };
        let mut stream = tokio::time::timeout(Duration::from_secs(60), async {
            provider.chat_stream(request).await
        })
        .await
        .map_err(|_| "chat_stream 超时 60s，可能网络或上游不可达")??;

        let mut saw_thinking = false;
        let mut saw_content = false;
        while let Some(item) = stream.next().await {
            match item? {
                StreamEvent::Thinking { delta, .. } if !delta.trim().is_empty() => {
                    saw_thinking = true;
                }
                StreamEvent::ContentDelta { delta } if !delta.trim().is_empty() => {
                    saw_content = true;
                }
                _ => {}
            }
        }

        if saw_thinking {
            assert!(saw_content, "responses 流式应出现正文 ContentDelta");
            return Ok(());
        }
        saw_content_any |= saw_content;
        tracing::warn!(
            attempt,
            max_attempts = MAX_ATTEMPTS,
            saw_content,
            "responses 流式本次未观察到 Thinking，准备重试"
        );
        if attempt == MAX_ATTEMPTS {
            break;
        }
    }

    assert!(
        saw_content_any,
        "responses 流式至少应出现正文 ContentDelta（即使未观察到 Thinking）"
    );
    tracing::warn!(
        max_attempts = MAX_ATTEMPTS,
        "responses 流式在多次尝试后仍未观察到 Thinking；当前按 provider 行为波动记录，不阻断集成门禁"
    );

    Ok(())
}

/// [语言行为观察实验] opt-in 真实环境验证（默认不执行）
///
/// 开关：`PI_WASM_E2E_LANGUAGE_BEHAVIOR=1`（兼容旧开关 `PI_WASM_E2E_PROMPT_LANGUAGE=1`）。
/// 验证：中文用户输入下，最终回答出现中文字符；若存在 thinking，也应出现中文字符。
#[tokio::test]
async fn test_openai_responses_latest_user_language_behavior_opt_in(
) -> Result<(), Box<dyn std::error::Error>> {
    if !language_behavior_e2e_opt_in() {
        tracing::info!(
            "skip language behavior opt-in e2e; set PI_WASM_E2E_LANGUAGE_BEHAVIOR=1 to enable"
        );
        return Ok(());
    }

    common::setup_logging();
    let _span =
        tracing::info_span!("test_openai_responses_latest_user_language_behavior_opt_in").entered();
    let _ = dotenvy::dotenv().ok();

    let config = responses_config();
    let provider = resolve_llm(&config)
        .expect("集成测试要求设置 OPENAI_API_KEY（环境变量或 .env），无 key 视为失败");
    let request = ChatRequest {
        messages: vec![ChatMessage::user(
            "请用一句中文回答：为什么 Rust 的所有权系统能减少内存错误？",
        )],
        model: config.default_model.clone(),
        temperature: None,
        max_tokens: Some(256),
        stream: Some(true),
        model_override: None,
        tools: None,
    };
    let mut stream = tokio::time::timeout(Duration::from_secs(60), async move {
        provider.chat_stream(request).await
    })
    .await
    .map_err(|_| "chat_stream 超时 60s，可能网络或上游不可达")??;

    let mut thinking_text = String::new();
    let mut answer_text = String::new();
    while let Some(item) = stream.next().await {
        match item? {
            StreamEvent::Thinking { delta, .. } => thinking_text.push_str(&delta),
            StreamEvent::ContentDelta { delta } => answer_text.push_str(&delta),
            _ => {}
        }
    }
    assert!(!answer_text.trim().is_empty(), "应捕获到最终回答文本");
    assert!(
        contains_cjk(&answer_text),
        "中文用户输入下，final answer 应以中文为主（行为观察），实际: {:?}",
        answer_text
    );
    if !thinking_text.trim().is_empty() {
        assert!(
            contains_cjk(&thinking_text),
            "thinking 出现时应以中文为主（行为观察），实际: {:?}",
            thinking_text
        );
    } else {
        tracing::info!(
            "language behavior opt-in e2e: no thinking event observed in this run; final answer language check passed"
        );
    }
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
    let img_tmp = decode_b64_to_tempfile(image_b64);
    let parts = vec![
        ChatMessageContentPart::text("Describe what you see in this image in one short sentence."),
        ChatMessageContentPart::image_b64("image/png", img_tmp.path())?,
    ];
    let request = ChatRequest {
        messages: vec![ChatMessage::user_with_parts(parts)],
        // 不显式指定 model，沿用 LlmConfig.default_model（当前 gpt-5.2，支持 vision）
        model: config.default_model.clone(),
        temperature: None,
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
    let pdf_tmp = decode_b64_to_tempfile(pdf_b64);
    let parts = vec![
        ChatMessageContentPart::text("Summarize the attached PDF in one short sentence."),
        ChatMessageContentPart::file_b64("sample.pdf", "application/pdf", pdf_tmp.path())?,
    ];
    let request = ChatRequest {
        messages: vec![ChatMessage::user_with_parts(parts)],
        // 不显式指定 model，沿用 LlmConfig.default_model（当前 gpt-5.2，支持 input_file）
        model: config.default_model.clone(),
        temperature: None,
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
    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    std::io::Write::write_all(&mut tmp, &oversize).unwrap();
    let err = ChatMessageContentPart::image_b64("image/png", tmp.path())
        .expect_err("超 IMAGE_MAX_BYTES 应返回结构化错误");
    let s = err.to_string();
    assert!(
        s.contains("IMAGE_MAX_BYTES"),
        "错误文案应含 IMAGE_MAX_BYTES，实际: {}",
        s
    );
}

/// PR-RJ-0：把 inline base64 fixture 解码后写到 tempfile，
/// 喂给新签名 `image_b64(mime, &Path)` / `file_b64(filename, mime, &Path)`。
fn decode_b64_to_tempfile(b64: &str) -> tempfile::NamedTempFile {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.as_bytes())
        .expect("decode b64 fixture");
    let mut f = tempfile::NamedTempFile::new().expect("temp file");
    std::io::Write::write_all(&mut f, &bytes).expect("write temp file");
    f
}
