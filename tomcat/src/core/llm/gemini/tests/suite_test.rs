use super::*;
use crate::core::llm::tests::mocks::{MockHttpServer, ScriptedHttpResponse};
use crate::core::llm::{
    ChatMessage, ChatRequest, Credential, LlmProvider, ProviderRefs, ReasoningContinuation,
    ReasoningFormat,
};
use tokio_stream::StreamExt;

fn gemini_entry(base_url: Option<&str>) -> ModelEntry {
    ModelEntry {
        id: "fcodex/gemini-4-carbon".to_string(),
        model_name: Some("gemini-4-carbon".to_string()),
        api: "google-gemini".to_string(),
        provider: "fcodex".to_string(),
        api_key_env: Some("FCODEX_2_API_KEY".to_string()),
        base_url: base_url.map(str::to_string),
        thinking_format: Some("gemini".to_string()),
        supported_reasoning_levels: vec!["low".into(), "medium".into(), "high".into()],
        supported_speeds: vec![],
        capabilities: Capabilities {
            vision: true,
            files: true,
            tools: true,
            reasoning: true,
            web_search: false,
        },
        context_window: Some(1_048_576),
        context_window_options: vec![],
        max_output_tokens: None,
        description: None,
    }
}

fn test_credential() -> Credential {
    Credential {
        provider: "fcodex".to_string(),
        env_name: "FCODEX_2_API_KEY".to_string(),
        value: "test-gemini-secret-key".to_string(),
    }
}

#[tokio::test]
async fn chat_and_stream_rewrite_v1_base_and_authenticate_only_via_x_goog_api_key() {
    let non_stream_json = r#"{"responseId":"resp_1","candidates":[{"content":{"role":"model","parts":[{"text":" Thought ","thought":true},{"text":"Hello Gemini"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":12,"candidatesTokenCount":3,"thoughtsTokenCount":5,"totalTokenCount":20}}"#;
    let stream_sse = include_str!(
        "../../../../../tests/fixtures/llm_stream/gemini_native_parallel_tool_calls.sse"
    );

    let server = MockHttpServer::start(vec![
        ScriptedHttpResponse::json(200, non_stream_json),
        ScriptedHttpResponse {
            status: 200,
            headers: vec![("Content-Type".to_string(), "text/event-stream".to_string())],
            body: stream_sse.to_string(),
            delay_ms: 0,
            declared_content_length: None,
        },
    ])
    .await;

    // Inherit `/v1` from a shared relay card; GeminiProvider must rewrite it to `/v1beta`.
    let entry = gemini_entry(Some(&format!("{}/v1", server.base_url)));
    let provider = GeminiProvider::new(&entry, &LlmRuntimeConfig::default(), &test_credential())
        .expect("construct GeminiProvider");

    let resp = provider
        .chat(ChatRequest {
            model: "fcodex/gemini-4-carbon".to_string(),
            messages: vec![ChatMessage::user("hi")],
            ..Default::default()
        })
        .await
        .expect("non-stream chat succeeds");
    assert_eq!(resp.id.as_deref(), Some("resp_1"));
    assert_eq!(resp.choices[0].message.text_content(), Some("Hello Gemini"));
    assert_eq!(
        resp.choices[0].message.thinking_text.as_deref(),
        Some("Thought")
    );
    assert_eq!(resp.usage.as_ref().unwrap().completion_tokens, 8);

    let mut stream = provider
        .chat_stream(ChatRequest {
            model: "models/gemini-4-carbon".to_string(),
            messages: vec![ChatMessage::user("list dirs")],
            ..Default::default()
        })
        .await
        .expect("stream chat succeeds");
    let mut stream_events = Vec::new();
    while let Some(ev) = stream.next().await {
        stream_events.push(ev.expect("stream event"));
    }
    assert!(stream_events
        .iter()
        .any(|e| matches!(e, StreamEvent::FinishReason { reason } if reason == "tool_calls")));

    let requests = server.request_texts();
    assert_eq!(requests.len(), 2);
    assert!(requests[0]
        .lines()
        .next()
        .unwrap()
        .starts_with("POST /v1beta/models/gemini-4-carbon:generateContent HTTP/1.1"));
    assert!(requests[1]
        .lines()
        .next()
        .unwrap()
        .starts_with("POST /v1beta/models/gemini-4-carbon:streamGenerateContent?alt=sse HTTP/1.1"));
    for req_text in &requests {
        let lower = req_text.to_ascii_lowercase();
        assert!(lower.contains("x-goog-api-key: test-gemini-secret-key"));
        assert!(!lower.contains("authorization:"));
        assert!(!req_text.lines().next().unwrap().contains("key="));
    }

    server.shutdown().await;
}

fn signed_history_request(provider: &GeminiProvider) -> ChatRequest {
    let profile = provider.source_profile("gemini-4-carbon");
    let assistant = ChatMessage::assistant_with_tool_calls(
        Some("Checking"),
        vec![serde_json::json!({"id":"call_read","type":"function",
            "function":{"name":"read","arguments":"{\"path\":\"a.rs\"}"}})],
    )
    .with_reasoning_state(
        None,
        Some(ReasoningContinuation {
            source_provider: profile.provider,
            source_api: profile.api_family,
            source_model: profile.model_family,
            format: ReasoningFormat::GeminiThoughtSignatures,
            opaque_payload: serde_json::json!([
                {"text":"old summary","thought":true},
                {"text":"Checking"},
                {"functionCall":{"id":"call_read","name":"read","args":{"path":"a.rs"}},
                 "thoughtSignature":"OLD_SIGNATURE"}
            ]),
            fallback_text: None,
            provider_refs: Some(ProviderRefs {
                openai_response_id: None,
                replay_profile_id: Some(profile.profile_id),
            }),
        }),
        None,
    );
    ChatRequest {
        model: "fcodex/gemini-4-carbon".into(),
        messages: vec![
            ChatMessage::user("inspect a.rs"),
            assistant,
            ChatMessage::tool("call_read", "file contents"),
        ],
        ..Default::default()
    }
}

#[tokio::test]
async fn signature_400_recovers_once_and_preserves_new_signatures_with_zero_retries() {
    for streaming in [false, true] {
        let error_body = if streaming {
            r#"{"error":{"code":400,"message":"Invalid THOUGHT_SIGNATURE.","status":"INVALID_ARGUMENT"}}"#
        } else {
            r#"{"error":{"code":400,"message":"Invalid thought signature.","status":"INVALID_ARGUMENT"}}"#
        };
        let reply = r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"Done","thoughtSignature":"NEW_SIGNATURE"}]},"finishReason":"STOP"}]}"#;
        let success = if streaming {
            ScriptedHttpResponse {
                status: 200,
                headers: vec![("Content-Type".into(), "text/event-stream".into())],
                body: format!("data: {reply}\n\n"),
                delay_ms: 0,
                declared_content_length: None,
            }
        } else {
            ScriptedHttpResponse::json(200, reply)
        };
        let server =
            MockHttpServer::start(vec![ScriptedHttpResponse::json(400, error_body), success]).await;
        let provider = GeminiProvider::new(
            &gemini_entry(Some(&server.base_url)),
            &LlmRuntimeConfig {
                retry_count: 0,
                ..Default::default()
            },
            &test_credential(),
        )
        .unwrap();
        let request = signed_history_request(&provider);
        let history_before = serde_json::to_value(&request.messages).unwrap();
        let continuation = if streaming {
            let mut stream = provider.chat_stream(request.clone()).await.unwrap();
            let mut continuation = None;
            let mut text = String::new();
            while let Some(event) = stream.next().await {
                match event.unwrap() {
                    StreamEvent::ContentDelta { delta } => text.push_str(&delta),
                    StreamEvent::ReasoningSnapshot {
                        reasoning_continuation,
                        ..
                    } => continuation = reasoning_continuation,
                    _ => {}
                }
            }
            assert_eq!(text, "Done");
            continuation.unwrap()
        } else {
            let response = provider.chat(request.clone()).await.unwrap();
            assert_eq!(response.choices[0].message.text_content(), Some("Done"));
            response.choices[0]
                .message
                .reasoning_continuation
                .clone()
                .unwrap()
        };
        assert_eq!(
            continuation.opaque_payload[0]["thoughtSignature"],
            "NEW_SIGNATURE"
        );
        assert_eq!(
            serde_json::to_value(&request.messages).unwrap(),
            history_before
        );
        assert!(provider.continuity_enabled);

        let requests = server.request_texts();
        assert_eq!(requests.len(), 2);
        let bodies: Vec<Value> = requests
            .iter()
            .map(|r| serde_json::from_str(r.split_once("\r\n\r\n").unwrap().1).unwrap())
            .collect();
        assert_eq!(
            bodies[0]["contents"][1]["parts"][2]["thoughtSignature"],
            "OLD_SIGNATURE"
        );
        assert!(!requests[1].contains("OLD_SIGNATURE"));
        assert!(!requests[1].contains("old summary"));
        assert_eq!(
            bodies[1]["contents"][1]["parts"],
            serde_json::json!([
                {"text":"Checking"},
                {"functionCall":{"id":"call_read","name":"read","args":{"path":"a.rs"}},
                 "thoughtSignature":wire::SKIP_THOUGHT_SIGNATURE_VALIDATOR}
            ])
        );
        assert_eq!(bodies[0]["contents"][2], bodies[1]["contents"][2]);
        assert_eq!(bodies[0]["generationConfig"], bodies[1]["generationConfig"]);
        server.shutdown().await;
    }
}

#[tokio::test]
async fn signature_recovery_does_not_retry_other_400_or_a_failed_recovery() {
    let signature_error = r#"{"error":{"code":400,"message":"Invalid thought signature.","status":"INVALID_ARGUMENT"}}"#;
    let ordinary_error = r#"{"error":{"code":400,"message":"Invalid tool definition.","status":"INVALID_ARGUMENT"}}"#;
    for recovery_status in [None, Some(400), Some(503)] {
        let (responses, expected_status, expected_body) = match recovery_status {
            None => (
                vec![ScriptedHttpResponse::json(400, ordinary_error)],
                400,
                ordinary_error,
            ),
            Some(status) => (
                vec![
                    ScriptedHttpResponse::json(400, signature_error),
                    ScriptedHttpResponse::json(status, signature_error),
                ],
                status,
                signature_error,
            ),
        };
        let expected_count = responses.len();
        let server = MockHttpServer::start(responses).await;
        let provider = GeminiProvider::new(
            &gemini_entry(Some(&server.base_url)),
            &LlmRuntimeConfig {
                retry_count: 2,
                ..Default::default()
            },
            &test_credential(),
        )
        .unwrap();
        let error = provider
            .chat(signed_history_request(&provider))
            .await
            .unwrap_err();
        assert_eq!(
            crate::infra::error::llm_http_status(&error),
            Some(expected_status)
        );
        assert!(crate::infra::error::llm_summary(&error)
            .unwrap()
            .contains(expected_body));
        assert_eq!(server.request_count(), expected_count);
        server.shutdown().await;
    }
}

#[test]
fn base_url_falls_back_from_provider_then_api() {
    let mut by_provider = gemini_entry(None);
    by_provider.provider = "google".to_string();
    let p1 = GeminiProvider::new(
        &by_provider,
        &LlmRuntimeConfig::default(),
        &test_credential(),
    )
    .unwrap();
    assert_eq!(
        p1.api_root,
        "https://generativelanguage.googleapis.com/v1beta"
    );

    let by_api = gemini_entry(None);
    let p2 =
        GeminiProvider::new(&by_api, &LlmRuntimeConfig::default(), &test_credential()).unwrap();
    assert_eq!(
        p2.api_root,
        "https://generativelanguage.googleapis.com/v1beta"
    );
}
