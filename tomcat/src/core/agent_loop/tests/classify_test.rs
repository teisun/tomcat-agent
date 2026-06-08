//! # `error_classifier::classify_error` 焦小测
//!
//! 验证错误分类的四个等价类（429 retry / 401 fatal / 400+context_overflow
//! retry / 400 generic fatal）是否落在正确的 LoopError 分支。

use crate::core::agent_loop::error_classifier::classify_error;
use crate::core::agent_loop::LoopError;
use crate::infra::error::{llm_error, llm_http_status_error, LlmErrorStage};

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
