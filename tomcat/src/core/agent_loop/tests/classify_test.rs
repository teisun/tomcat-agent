//! # `error_classifier::classify_error` 焦小测
//!
//! 验证错误分类的四个等价类（429 retry / 401 fatal / 400+context_overflow
//! retry / 400 generic fatal）是否落在正确的 LoopError 分支。

use crate::core::agent_loop::error_classifier::classify_error;
use crate::core::agent_loop::LoopError;
use crate::infra::error::AppError;

#[test]
fn classify_error_retryable_429() {
    let e = AppError::Llm("API 错误 429: rate limit".to_string());
    let r = classify_error(&e);
    assert!(matches!(r, LoopError::Retryable(_)));
}

#[test]
fn classify_error_fatal_401() {
    let e = AppError::Llm("API 错误 401: unauthorized".to_string());
    let r = classify_error(&e);
    assert!(matches!(r, LoopError::Fatal(_)));
}

#[test]
fn classify_error_context_length_400_is_retryable() {
    let body = r#"{"error":{"message":"Input tokens exceed limit","type":"invalid_request_error","param":"messages","code":"context_length_exceeded"}}"#;
    let e = AppError::Llm(format!("API 错误 400: {}", body));
    let r = classify_error(&e);
    assert!(
        matches!(r, LoopError::Retryable(_)),
        "OpenAI 400 context_length_exceeded must be Retryable so L3 trim can run"
    );
}

#[test]
fn classify_error_generic_400_stays_fatal() {
    let e = AppError::Llm(
        r#"API 错误 400: {"error":{"message":"invalid model","type":"invalid_request_error"}}"#
            .to_string(),
    );
    let r = classify_error(&e);
    assert!(matches!(r, LoopError::Fatal(_)));
}
