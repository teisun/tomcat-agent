use super::*;
use crate::infra::LlmConfig;
use std::path::Path;

/// 从 crate 根目录加载 .env，便于本地有 key 时跑测试（CI 无 .env 则跳过依赖 key 的用例）。
fn load_dotenv() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(".env");
    let _ = dotenvy::from_path(path);
}

#[test]
fn openai_provider_new_fails_without_api_key() {
    println!("[TEST] openai_provider_new_fails_without_api_key — 开始");
    let config = LlmConfig {
        api_key_env: Some("PI_WASM_TEST_NONEXISTENT_ENV_VAR_12345".to_string()),
        ..LlmConfig::default()
    };
    println!("[TEST] 过程: OpenAiProvider::new(api_key_env=不存在变量)");
    let r = OpenAiProvider::new(&config);
    assert!(r.is_err());
    let err = r.unwrap_err();
    let msg = err.to_string();
    println!("[TEST] 过程: 错误信息 = {}", msg);
    assert!(msg.contains("未设置"));
    println!("[TEST] 结果: 通过（new 在无 key 时正确返回 Err）");
}

/// 依赖 OPENAI_API_KEY：有 key 时断言 new 成功且 provider_name 正确；无 key 时不通过（宪法：核心功能不得跳过）。
#[test]
fn openai_provider_new_succeeds_with_api_key() {
    load_dotenv();
    println!("[TEST] openai_provider_new_succeeds_with_api_key — 开始");
    if std::env::var("OPENAI_API_KEY").is_err() {
        panic!("OPENAI_API_KEY 未配置，本用例不通过（宪法与单测规范：无 key 不得跳过）");
    }
    println!("[TEST] 过程: OPENAI_API_KEY 已设置，调用 OpenAiProvider::new(LlmConfig::default())");
    let config = LlmConfig::default();
    let provider = OpenAiProvider::new(&config).expect("OPENAI_API_KEY 已设置时应创建成功");
    assert_eq!(provider.provider_name(), "openai");
    println!(
        "[TEST] 过程: provider_name() = {}",
        provider.provider_name()
    );
    println!("[TEST] 结果: 通过");
}

/// 依赖 OPENAI_API_KEY：有 key 时断言 new 成功且 count_tokens 返回合理值（近似）；无 key 时不通过。
#[test]
fn count_tokens_approximate() {
    load_dotenv();
    println!("[TEST] count_tokens_approximate — 开始");
    if std::env::var("OPENAI_API_KEY").is_err() {
        panic!("OPENAI_API_KEY 未配置，本用例不通过（宪法与单测规范：无 key 不得跳过）");
    }
    let config = LlmConfig::default();
    let provider = OpenAiProvider::new(&config).expect("OPENAI_API_KEY 已设置时应创建成功");
    let messages = vec![
        ChatMessage::user("hello world"),
        ChatMessage::assistant("hi there"),
    ];
    println!("[TEST] 过程: count_tokens(messages) 本地近似计算");
    let n = provider.count_tokens(&messages).unwrap();
    println!("[TEST] 过程: count_tokens 返回 = {}", n);
    assert!(n >= 1, "count_tokens 应至少为 1");
    assert!(n <= 20, "count_tokens 近似值应在合理范围");
    println!("[TEST] 结果: 通过 (n={})", n);
}

#[test]
fn is_retriable_detects_429_and_5xx() {
    println!("[TEST] is_retriable_detects_429_and_5xx — 开始");
    println!("[TEST] 过程: 检查 429/502 为可重试、400 为不可重试");
    assert!(OpenAiProvider::is_retriable(&AppError::Llm(
        "API 错误 429: rate limit".to_string()
    )));
    assert!(OpenAiProvider::is_retriable(&AppError::Llm(
        "API 错误 502: bad gateway".to_string()
    )));
    assert!(!OpenAiProvider::is_retriable(&AppError::Llm(
        "API 错误 400: bad request".to_string()
    )));
    println!("[TEST] 结果: 通过");
}

#[test]
fn is_retriable_returns_false_for_non_llm_error() {
    assert!(!OpenAiProvider::is_retriable(&AppError::Config(
        "config error".to_string()
    )));
}

#[test]
fn test_openai_chunk_with_usage_emits_usage_event() {
    let chunk: OpenAiStreamChunk = serde_json::from_str(
        r#"{"choices":[],"usage":{"prompt_tokens":150,"completion_tokens":42,"total_tokens":192}}"#,
    )
    .expect("should parse chunk with usage");
    let events = openai_chunk_to_stream_events(chunk);
    assert_eq!(events.len(), 1, "should emit exactly one Usage event");
    match &events[0] {
        StreamEvent::Usage {
            prompt_tokens,
            completion_tokens,
            total_tokens,
        } => {
            assert_eq!(*prompt_tokens, 150);
            assert_eq!(*completion_tokens, 42);
            assert_eq!(*total_tokens, Some(192));
        }
        other => panic!("expected StreamEvent::Usage, got {:?}", other),
    }
}

#[test]
fn test_openai_chunk_without_usage_no_usage_event() {
    let chunk: OpenAiStreamChunk = serde_json::from_str(
        r#"{"choices":[{"delta":{"content":"hi"}}]}"#,
    )
    .expect("should parse chunk without usage");
    let events = openai_chunk_to_stream_events(chunk);
    assert!(
        !events.iter().any(|e| matches!(e, StreamEvent::Usage { .. })),
        "should not contain Usage event when chunk has no usage field"
    );
    assert_eq!(events.len(), 1);
    assert!(matches!(&events[0], StreamEvent::ContentDelta { delta } if delta == "hi"));
}

#[tokio::test]
async fn sse_stream_parses_and_yields_events() {
    println!("[TEST] sse_stream_parses_and_yields_events — 开始");
    use super::*;
    use tokio_stream::StreamExt;
    let chunks: Vec<Result<Bytes, AppError>> = vec![
        Ok(Bytes::from(
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n",
        )),
        Ok(Bytes::from(
            "data: {\"choices\":[{\"delta\":{\"content\":\" world\"}}]}\n\n",
        )),
        Ok(Bytes::from(
            "data: {\"choices\":[{\"finish_reason\":\"stop\"}]}\n\n",
        )),
    ];
    println!("[TEST] 过程: 使用 mock SSE 字节流解析");
    let stream = tokio_stream::iter(chunks);
    let mut event_stream = SseEventStream::new(stream);
    let mut events = Vec::new();
    while let Some(item) = event_stream.next().await {
        events.push(item);
    }
    assert_eq!(events.len(), 3);
    assert!(matches!(&events[0], Ok(StreamEvent::ContentDelta { delta } ) if delta == "Hello"));
    assert!(matches!(&events[1], Ok(StreamEvent::ContentDelta { delta } ) if delta == " world"));
    assert!(matches!(&events[2], Ok(StreamEvent::FinishReason { reason } ) if reason == "stop"));
    println!("[TEST] 过程: 解析到 3 个事件 (ContentDelta x2, FinishReason x1)");
    println!("[TEST] 结果: 通过");
}

/// 依赖 OPENAI_API_KEY 与可用配额：有 key 时调用真实 chat 接口一次，打印请求与响应；无 key 时 panic。
/// CI/无配额环境默认跳过，本机有配额时可用 `cargo test -- --ignored` 运行。
#[tokio::test]
#[ignore = "依赖真实 OpenAI API 与配额，CI 默认跳过"]
async fn chat_real_request_response_print() {
    load_dotenv();
    println!("[TEST] chat_real_request_response_print — 开始");
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
    println!("[TEST] 过程: 请求体（发往 OpenAI /chat/completions）:");
    if let Ok(json) = serde_json::to_string_pretty(&request) {
        println!("{}", json);
    }
    println!("[TEST] 过程: 调用 provider.chat(request).await ...");
    match provider.chat(request).await {
        Ok(resp) => {
            println!("[TEST] 过程: 响应体（OpenAI 返回）:");
            println!("  id: {:?}", resp.id);
            for (i, c) in resp.choices.iter().enumerate() {
                println!("  choices[{}].message.content: {:?}", i, c.message.content);
                println!("  choices[{}].finish_reason: {:?}", i, c.finish_reason);
            }
            if let Some(u) = &resp.usage {
                println!(
                    "  usage: prompt_tokens={}, completion_tokens={}, total_tokens={:?}",
                    u.prompt_tokens, u.completion_tokens, u.total_tokens
                );
            }
            println!("[TEST] 结果: 通过（已打印请求与响应）");
        }
        Err(e) => {
            println!("[TEST] 过程: 请求失败: {}", e);
            panic!(
                "chat 请求失败: {}（请在本机终端运行 cargo test，并确认可访问 api.openai.com 且已配置 OPENAI_API_KEY）",
                e
            );
        }
    }
}
