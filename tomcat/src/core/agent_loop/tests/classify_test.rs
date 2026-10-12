//! # `error_classifier::classify_error` 焦小测
//!
//! 验证错误分类的四个等价类（429 retry / 401 fatal / 400+context_overflow
//! retry / 400 generic fatal）是否落在正确的 LoopError 分支。

use crate::core::agent_loop::error_classifier::classify_error;
use crate::core::agent_loop::LoopError;
use crate::infra::error::{
    llm_error, llm_http_status_error, llm_stream_interrupted_error, llm_stream_terminal_error,
    LlmErrorStage,
};

#[test]
fn classify_error_retryable_429() {
    let e = llm_http_status_error("openai", 429, "rate limit");
    let r = classify_error(e);
    assert!(matches!(r, LoopError::Retryable(_)));
}

#[test]
fn classify_error_fatal_401() {
    let e = llm_http_status_error("openai", 401, "unauthorized");
    let r = classify_error(e);
    assert!(matches!(r, LoopError::Fatal(_)));
}

#[test]
fn classify_error_context_length_400_is_retryable() {
    let body = r#"{"error":{"message":"Input tokens exceed limit","type":"invalid_request_error","param":"messages","code":"context_length_exceeded"}}"#;
    let e = llm_http_status_error("openai", 400, body);
    let r = classify_error(e);
    assert!(
        matches!(r, LoopError::Retryable(_)),
        "OpenAI 400 context_length_exceeded must be Retryable so L3 trim can run"
    );
}

#[test]
fn classify_error_generic_400_stays_fatal() {
    let e = llm_http_status_error(
        "openai",
        400,
        r#"{"error":{"message":"invalid model","type":"invalid_request_error"}}"#,
    );
    let r = classify_error(e);
    assert!(matches!(r, LoopError::Fatal(_)));
}

#[test]
fn classify_error_retryable_503() {
    let e = llm_http_status_error(
        "openai",
        503,
        "upstream connect error or disconnect/reset before headers",
    );
    let r = classify_error(e);
    assert!(matches!(r, LoopError::Retryable(_)));
}

#[test]
fn classify_error_retryable_504() {
    let e = llm_http_status_error("openai", 504, "gateway timeout");
    let r = classify_error(e);
    assert!(matches!(r, LoopError::Retryable(_)));
}

#[test]
fn classify_error_retryable_500() {
    let e = llm_http_status_error("openai", 500, "internal error");
    let r = classify_error(e);
    assert!(matches!(r, LoopError::Retryable(_)));
}

#[test]
fn classify_error_fatal_403() {
    let e = llm_http_status_error("openai", 403, "forbidden");
    let r = classify_error(e);
    assert!(matches!(r, LoopError::Fatal(_)));
}

#[test]
fn classify_error_billing_is_fatal_even_when_403_or_429() {
    for (status, body) in [
        (403, r#"{"error":{"code":"insufficient_quota"}}"#),
        (429, r#"{"error":{"message":"SPEND_LIMIT reached"}}"#),
    ] {
        let e = llm_http_status_error("transit", status, body);
        assert!(
            matches!(classify_error(e), LoopError::Fatal(_)),
            "billing payload with HTTP {status} must not blind-retry"
        );
    }
}

#[test]
fn classify_error_stream_context_overflow_is_retryable_without_status() {
    let e = llm_stream_terminal_error(
        "transit",
        r#"{"error":{"message":"request exceeds the context window","code":"context_length_exceeded"}}"#,
        Some("context_length_exceeded".to_string()),
    );
    assert!(matches!(classify_error(e), LoopError::Retryable(_)));
}

#[test]
fn classify_error_upstream_400_is_retryable_but_invalid_request_is_fatal() {
    let upstream = llm_http_status_error(
        "transit",
        400,
        r#"{"error":{"code":"upstream_error","message":"Upstream request failed"}}"#,
    );
    assert!(matches!(classify_error(upstream), LoopError::Retryable(_)));

    let invalid = llm_http_status_error(
        "openai",
        400,
        r#"{"error":{"message":"invalid model","type":"invalid_request_error"}}"#,
    );
    assert!(matches!(classify_error(invalid), LoopError::Fatal(_)));
}

#[test]
fn classify_error_uses_captured_upstream_400_fixture() {
    let body = include_str!("../../../../tests/fixtures/llm_failure/upstream_400.json");
    assert!(
        matches!(
            classify_error(llm_http_status_error("transit", 400, body)),
            LoopError::Retryable(_)
        ),
        "the captured upstream 400 payload must retain its transport retry route"
    );
}

#[test]
fn classify_error_uses_captured_stream_overflow_fixture() {
    let body = include_str!("../../../../tests/fixtures/llm_failure/stream_context_overflow.json");
    assert!(
        matches!(
            classify_error(llm_stream_terminal_error(
                "transit",
                body,
                Some("context_length_exceeded".to_string()),
            )),
            LoopError::Retryable(_)
        ),
        "the captured stream overflow payload must take the trimming route"
    );
}

#[test]
fn classify_error_uses_captured_keepalive_fixture() {
    let keepalive = include_str!("../../../../tests/fixtures/llm_failure/sse_keepalive.txt");
    assert_eq!(
        keepalive.trim(),
        ":",
        "fixture must remain an SSE comment frame"
    );
    assert!(
        matches!(
            classify_error(llm_stream_interrupted_error(
                "transit",
                format!("stream ended after keepalive frame {keepalive:?}"),
            )),
            LoopError::Retryable(_)
        ),
        "a stream ending after a keepalive is a transport interruption, not a fatal protocol error"
    );
}

#[test]
fn classify_error_idle_timeout_stage_is_retryable() {
    let e = llm_error("openai", LlmErrorStage::IdleTimeout, "流式空闲超时");
    let r = classify_error(e);
    assert!(matches!(r, LoopError::Retryable(_)));
}

#[test]
fn classify_error_read_timeout_stage_is_retryable() {
    let e = llm_error("openai", LlmErrorStage::ReadTimeout, "读取响应超时");
    let r = classify_error(e);
    assert!(matches!(r, LoopError::Retryable(_)));
}

#[test]
fn classify_error_stream_terminal_error_is_retryable() {
    let e = llm_stream_terminal_error(
        "fcodex",
        "[OneOfParam] [input[0].content[1]] [invalid_enum_value] Invalid value: 'input_file'. Supported values are: 'input_text'.",
        Some("invalid_request_error".into()),
    );
    let r = classify_error(e);
    assert!(matches!(r, LoopError::Retryable(_)));
}

#[test]
fn classify_error_content_filter_stream_terminal_stays_fatal() {
    let e = llm_stream_terminal_error("openai", "content_filter", None);
    let r = classify_error(e);
    assert!(matches!(r, LoopError::Fatal(_)));
}

#[test]
fn classify_error_context_overflow_still_wins_before_generic_400() {
    let body = r#"{"error":{"message":"maximum context length reached; please reduce the length","type":"invalid_request_error","code":"context_length_exceeded"}}"#;
    let e = llm_http_status_error("openai", 400, body);
    let r = classify_error(e);
    assert!(matches!(r, LoopError::Retryable(_)));
}

#[test]
fn classify_error_invalid_model_400_stays_fatal() {
    let e = llm_http_status_error(
        "openai",
        400,
        r#"{"error":{"message":"invalid model","type":"invalid_request_error"}}"#,
    );
    let r = classify_error(e);
    assert!(matches!(r, LoopError::Fatal(_)));
}
