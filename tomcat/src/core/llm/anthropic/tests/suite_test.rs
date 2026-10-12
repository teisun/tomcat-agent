use super::*;
use crate::core::llm::tests::mocks::{MockHttpServer, ScriptedHttpResponse};
use crate::core::llm::{ChatRequest, Credential, LlmProvider};
use crate::infra::LlmConfig;
use bytes::Bytes;
use tokio_stream::wrappers::IntervalStream;
use tokio_stream::StreamExt;

#[test]
fn parse_retry_after_ms_converts_integer_seconds() {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::RETRY_AFTER,
        reqwest::header::HeaderValue::from_static("17"),
    );

    assert_eq!(parse_retry_after_ms(&headers), Some(17_000));
}

#[test]
fn parse_retry_after_ms_converts_rfc2822_http_date() {
    let retry_at = chrono::Utc::now() + chrono::Duration::hours(1);
    let raw = retry_at.format("%a, %d %b %Y %H:%M:%S GMT").to_string();
    let retry_at = chrono::DateTime::parse_from_rfc2822(&raw)
        .expect("formatted HTTP date must remain RFC2822-compatible")
        .with_timezone(&chrono::Utc);
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::RETRY_AFTER,
        raw.parse().expect("valid HTTP header value"),
    );

    let before = chrono::Utc::now();
    let delay_ms = parse_retry_after_ms(&headers).expect("HTTP date must produce a delay");
    let after = chrono::Utc::now();
    let min_delay_ms = retry_at
        .signed_duration_since(after)
        .num_milliseconds()
        .max(0) as u64;
    let max_delay_ms = retry_at
        .signed_duration_since(before)
        .num_milliseconds()
        .max(0) as u64;

    assert!(
        (min_delay_ms..=max_delay_ms).contains(&delay_ms),
        "parsed delay {delay_ms}ms must be bounded by its two observed UTC instants \
         [{min_delay_ms}, {max_delay_ms}]ms"
    );
}

#[test]
fn switches_only_between_the_two_anthropic_thinking_formats() {
    assert_eq!(
        alternative_thinking_format(ThinkingFormat::AnthropicAdaptive),
        Some(ThinkingFormat::Anthropic)
    );
    assert_eq!(
        alternative_thinking_format(ThinkingFormat::Anthropic),
        Some(ThinkingFormat::AnthropicAdaptive)
    );
    assert_eq!(alternative_thinking_format(ThinkingFormat::Openai), None);
    assert!(is_thinking_format_rejection(&AppError::Llm(
        "400 adaptive thinking is not available for this model".to_string()
    )));
    assert!(is_thinking_format_rejection(&AppError::Llm(
        "400 adaptive thinking is not supported for this model".to_string()
    )));
    assert!(is_thinking_format_rejection(&AppError::Llm(
        "400 budget_tokens is not accepted".to_string()
    )));
    assert!(!is_thinking_format_rejection(&AppError::Llm(
        "400 malformed tool input".to_string()
    )));
}

#[test]
fn learned_thinking_format_overrides_only_the_current_runtime() {
    let learned = RwLock::new(None);
    assert_eq!(
        learned_or_configured_thinking_format(&learned, ThinkingFormat::AnthropicAdaptive),
        ThinkingFormat::AnthropicAdaptive
    );

    *learned.write() = Some(ThinkingFormat::Anthropic);
    assert_eq!(
        learned_or_configured_thinking_format(&learned, ThinkingFormat::AnthropicAdaptive),
        ThinkingFormat::Anthropic
    );
}

#[tokio::test]
async fn rejected_thinking_format_is_retried_once_and_cached_for_this_provider() {
    let server = MockHttpServer::start(vec![
        ScriptedHttpResponse::json(
            400,
            r#"{"error":{"message":"adaptive thinking is not supported for this model"}}"#,
        ),
        ScriptedHttpResponse::json(
            200,
            r#"{"id":"msg_1","type":"message","role":"assistant","content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}}"#,
        ),
        ScriptedHttpResponse::json(
            200,
            r#"{"id":"msg_2","type":"message","role":"assistant","content":[{"type":"text","text":"still ok"}],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}}"#,
        ),
        ScriptedHttpResponse::json(
            200,
            r#"{"id":"msg_b","type":"message","role":"assistant","content":[{"type":"text","text":"B keeps adaptive"}],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}}"#,
        ),
    ])
    .await;
    let entry = ModelEntry {
        id: "thinking-fallback-test".to_string(),
        model_name: Some("thinking-fallback-test".to_string()),
        api: "anthropic-messages".to_string(),
        provider: "anthropic".to_string(),
        api_key_env: None,
        base_url: Some(server.base_url.clone()),
        capabilities: Capabilities {
            reasoning: true,
            ..Capabilities::default()
        },
        context_window: Some(200_000),
        context_window_options: Vec::new(),
        max_output_tokens: Some(32_000),
        description: None,
        supported_speeds: Vec::new(),
        thinking_format: Some("anthropic-adaptive".to_string()),
        supported_reasoning_levels: vec!["high".to_string()],
    };
    let runtime = LlmConfig::default().runtime();
    let credential = Credential {
        provider: "anthropic".to_string(),
        env_name: "TEST_KEY".to_string(),
        value: "stub".to_string(),
    };
    let route = super::super::ProviderRoute::new(&runtime).expect("shared route");
    let provider =
        AnthropicProvider::with_route(&entry, &runtime, &credential, &route).expect("provider A");
    let mut entry_b = entry.clone();
    entry_b.id = "thinking-model-b".into();
    entry_b.model_name = Some(entry_b.id.clone());
    let provider_b = AnthropicProvider::with_route(&entry_b, &runtime, &credential, &route)
        .expect("provider B on the same route");
    let request = ChatRequest {
        messages: vec![ChatMessage::user("hello")],
        model: entry.id.clone(),
        temperature: None,
        max_tokens: None,
        resolved_output_limit: None,
        diagnostic_request_id: None,
        stream: Some(false),
        model_override: None,
        speed: None,
        thinking_level: None,
        cache_key: None,
        tools: None,
    };

    assert_eq!(
        provider
            .chat(request.clone())
            .await
            .expect("fallback request")
            .choices[0]
            .message
            .text_content(),
        Some("ok")
    );
    provider
        .chat(request.clone())
        .await
        .expect("cached fallback request");
    provider_b
        .chat(ChatRequest {
            model: entry_b.id,
            ..request
        })
        .await
        .expect("B request");

    assert_eq!(server.request_count(), 4);
    let requests = server.request_texts();
    assert!(requests[0].contains(r#""type":"adaptive""#));
    assert!(requests[1].contains(r#""type":"enabled""#));
    assert!(
        requests[2].contains(r#""type":"enabled""#),
        "the succeeding fallback must be reused instead of repeating the rejected adaptive format"
    );
    assert!(
        requests[3].contains(r#""type":"adaptive""#),
        "A's learned fallback must not leak through the shared route to B"
    );
    server.shutdown().await;
}

#[tokio::test]
async fn cached_provider_uses_each_requests_resolved_output_limit() {
    let server = MockHttpServer::start(vec![
        ScriptedHttpResponse::json(
            200,
            r#"{"id":"msg_1","type":"message","role":"assistant","content":[{"type":"text","text":"first"}],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}}"#,
        ),
        ScriptedHttpResponse::json(
            200,
            r#"{"id":"msg_2","type":"message","role":"assistant","content":[{"type":"text","text":"second"}],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}}"#,
        ),
    ])
    .await;
    let entry = ModelEntry {
        id: "anthropic-small".to_string(),
        model_name: Some("anthropic-small".to_string()),
        api: "anthropic-messages".to_string(),
        provider: "anthropic".to_string(),
        api_key_env: None,
        base_url: Some(server.base_url.clone()),
        capabilities: Capabilities::default(),
        context_window: Some(200_000),
        context_window_options: Vec::new(),
        max_output_tokens: Some(8_192),
        description: None,
        supported_speeds: Vec::new(),
        thinking_format: Some("anthropic-adaptive".to_string()),
        supported_reasoning_levels: vec!["high".to_string()],
    };
    let provider = AnthropicProvider::new(
        &entry,
        &LlmConfig::default().runtime(),
        &Credential {
            provider: "anthropic".to_string(),
            env_name: "TEST_KEY".to_string(),
            value: "stub".to_string(),
        },
    )
    .expect("provider");

    for (model, resolved_output_limit) in [("anthropic-small", 8_192), ("anthropic-large", 16_384)]
    {
        provider
            .chat(ChatRequest {
                messages: vec![ChatMessage::user("hello")],
                model: model.to_string(),
                resolved_output_limit: Some(resolved_output_limit),
                stream: Some(false),
                ..Default::default()
            })
            .await
            .expect("request succeeds");
    }

    let requests = server.request_texts();
    assert!(requests[0].contains(r#""max_tokens":8192"#));
    assert!(
        requests[1].contains(r#""max_tokens":16384"#),
        "the second model's resolved cap must not inherit the provider construction entry"
    );
    server.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn keepalive_bytes_still_trigger_idle_timeout_when_no_events_arrive() {
    let interval = tokio::time::interval(Duration::from_millis(200));
    let source = IntervalStream::new(interval).map(|_| Ok(Bytes::from_static(b": keepalive\n\n")));
    let event_stream = stream::AnthropicStream::new(
        source,
        ProviderCompatProfile::anthropic_messages("claude-opus-4-8"),
        true,
    );
    let mut stream = apply_stream_idle_timeout(event_stream, 1);
    let next_task = tokio::spawn(async move { stream.next().await });

    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(2)).await;

    let item = next_task
        .await
        .expect("join ok")
        .expect("should produce timeout error");
    match item {
        Err(err) => {
            assert_eq!(
                crate::infra::error::llm_stage(&err),
                Some(LlmErrorStage::IdleTimeout)
            );
            let msg = crate::infra::error::llm_summary(&err).unwrap_or_else(|| err.to_string());
            assert!(
                msg.contains("stream_timeout_sec=1s"),
                "unexpected msg: {}",
                msg
            );
        }
        other => panic!("expected timeout AppError, got {:?}", other),
    }
}
