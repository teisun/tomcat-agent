use super::super::*;

#[test]
fn app_error_display() {
    let e = AppError::Config("test".to_string());
    assert!(e.to_string().contains("配置错误"));
    assert!(e.to_string().contains("test"));
}

#[test]
fn app_error_from_io() {
    let io = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
    let e: AppError = io.into();
    assert!(matches!(e, AppError::Io(_)));
}

#[test]
fn app_error_apply_boundary_stale_display() {
    let e = AppError::ApplyBoundaryStale {
        covered_end_id: "e1".to_string(),
    };
    let s = e.to_string();
    assert!(s.contains("e1"));
    assert!(s.contains("apply_boundary"));
}

#[test]
fn llm_http_status_error_preserves_status_provider_and_summary() {
    let err = llm_http_status_error("openai", 503, "upstream connect error");
    assert_eq!(llm_http_status(&err), Some(503));
    assert_eq!(llm_stage(&err), None);
    assert_eq!(
        llm_summary(&err).as_deref(),
        Some("API 错误 503: upstream connect error")
    );
}

#[test]
fn llm_http_status_error_supports_uncommon_status_codes() {
    let err = llm_http_status_error("openai", 418, "teapot");
    assert_eq!(llm_http_status(&err), Some(418));
    assert_eq!(llm_summary(&err).as_deref(), Some("API 错误 418: teapot"));
}

#[test]
fn llm_http_status_error_with_stage_sets_both_fields() {
    let err = llm_http_status_error_with_stage(
        "openai",
        LlmErrorStage::Connect,
        503,
        "upstream connect error",
    );
    assert_eq!(llm_http_status(&err), Some(503));
    assert_eq!(llm_stage(&err), Some(LlmErrorStage::Connect));
}

#[test]
fn is_retryable_llm_error_matches_truth_table() {
    for status in [429, 500, 502, 503, 504] {
        assert!(
            is_retryable_llm_error(&llm_http_status_error("openai", status, "retry me")),
            "status {status} 应为可重试"
        );
    }
    for status in [400, 401, 403, 404] {
        assert!(
            !is_retryable_llm_error(&llm_http_status_error("openai", status, "fatal")),
            "status {status} 不应为可重试"
        );
    }
    for stage in [
        LlmErrorStage::Connect,
        LlmErrorStage::Send,
        LlmErrorStage::BodyRead,
        LlmErrorStage::IdleTimeout,
        LlmErrorStage::ReadTimeout,
    ] {
        assert!(
            is_retryable_llm_error(&llm_error("openai", stage, "retry")),
            "stage {stage} 应为可重试"
        );
    }
    for stage in [
        LlmErrorStage::RequestTimeout,
        LlmErrorStage::NonStreamStale,
        LlmErrorStage::Parse,
    ] {
        assert!(
            !is_retryable_llm_error(&llm_error("openai", stage, "fatal")),
            "stage {stage} 不应为可重试"
        );
    }
    assert!(!is_retryable_llm_error(&AppError::Io(
        std::io::Error::other("oops")
    )));
    assert!(!is_retryable_llm_error(&AppError::Llm(
        "API 错误 503: legacy string".to_string()
    )));
}

#[test]
fn llm_connect_or_network_matches_truth_table() {
    assert!(llm_connect_or_network(&llm_error(
        "openai",
        LlmErrorStage::Connect,
        "connect failed",
    )));
    assert!(llm_connect_or_network(&llm_error(
        "openai",
        LlmErrorStage::Send,
        "send failed",
    )));
    assert!(llm_connect_or_network(&llm_http_status_error(
        "openai",
        503,
        "upstream connect error or disconnect/reset before headers. reset reason: connection timeout",
    )));
    for status in [429, 500, 400, 401] {
        assert!(
            !llm_connect_or_network(&llm_http_status_error("openai", status, "not network")),
            "status {status} 不应被视为网络错误"
        );
    }
    assert!(!llm_connect_or_network(&llm_error(
        "openai",
        LlmErrorStage::Parse,
        "parse failed",
    )));
}

#[test]
fn is_context_overflow_matches_truth_table() {
    assert!(is_context_overflow(&llm_http_status_error(
        "openai",
        400,
        r#"{"error":{"code":"context_length_exceeded"}}"#,
    )));
    assert!(is_context_overflow(&llm_http_status_error(
        "openai",
        400,
        "maximum context length reached; please reduce the length",
    )));
    assert!(!is_context_overflow(&llm_http_status_error(
        "openai",
        400,
        r#"{"error":{"message":"invalid model"}}"#,
    )));
    assert!(!is_context_overflow(&llm_http_status_error(
        "openai",
        503,
        "context_length_exceeded",
    )));
    assert!(!is_context_overflow(&llm_http_status_error(
        "openai", 400, ""
    )));
}
