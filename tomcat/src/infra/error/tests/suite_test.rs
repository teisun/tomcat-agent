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
fn llm_stream_terminal_error_preserves_code_without_status_or_stage() {
    let err = llm_stream_terminal_error("fcodex", "boom", Some("invalid_request_error".into()));
    assert_eq!(llm_http_status(&err), None);
    assert_eq!(llm_stage(&err), None);
    assert_eq!(llm_summary(&err).as_deref(), Some("boom"));
    assert_eq!(llm_code(&err).as_deref(), Some("invalid_request_error"));
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
    for stage in [LlmErrorStage::NonStreamStale, LlmErrorStage::Parse] {
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
    let empty = llm_empty_response_error("openai-responses", "visible output missing");
    assert_eq!(
        classify_llm_failure(&empty).kind,
        LlmFailureKind::EmptyResponse
    );
    assert!(
        is_retryable_llm_error(&empty),
        "typed empty responses must enter the ordinary retry policy"
    );
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
        400,
        "invalid model"
    )));
    assert!(is_context_overflow(&llm_stream_terminal_error(
        "fcodex",
        "request exceeds the context window",
        Some("invalid_request_error".to_string()),
    )));
    assert!(is_context_overflow(&llm_http_status_error(
        "anthropic",
        413,
        "request entity too large",
    )));
}

#[test]
fn context_overflow_text_corpus_is_specific() {
    for sample in [
        "exceeds the context window",
        "prompt is too long",
        "input length 200000 plus max_tokens exceeds the limit",
        "model_context_window_exceeded",
    ] {
        assert!(
            is_context_overflow_text(sample),
            "context overflow corpus must match {sample:?}"
        );
    }
    for sample in [
        "context is available",
        "rate limit exceeded for this context",
        "token limit has reset",
    ] {
        assert!(
            !is_context_overflow_text(sample),
            "broad context/token wording must not falsely match {sample:?}"
        );
    }
}

#[test]
fn llm_failure_classification_obeys_billing_before_status() {
    let cases = [
        (
            llm_stream_terminal_error(
                "openai",
                r#"{"error":{"code":"insufficient_quota","type":"insufficient_quota"}}"#,
                Some("insufficient_quota".to_string()),
            ),
            LlmFailureKind::Billing,
            FailureDomain::Account,
        ),
        (
            llm_http_status_error(
                "anthropic",
                403,
                r#"{"error":{"type":"billing_error","message":"insufficient credits"}}"#,
            ),
            LlmFailureKind::Billing,
            FailureDomain::Account,
        ),
        (
            llm_http_status_error("fcodex", 403, "forbidden"),
            LlmFailureKind::Authentication,
            FailureDomain::Account,
        ),
        (
            llm_http_status_error("deepseek", 429, "SPEND_LIMIT reached"),
            LlmFailureKind::Billing,
            FailureDomain::Account,
        ),
        (
            llm_http_status_error("openai", 429, "rate limit exceeded"),
            LlmFailureKind::RateLimit,
            FailureDomain::Transport,
        ),
        (
            llm_http_status_error(
                "transit",
                400,
                r#"{"error":{"code":"upstream_error","message":"Upstream request failed"}}"#,
            ),
            LlmFailureKind::UpstreamTransient,
            FailureDomain::Transport,
        ),
    ];
    for (error, kind, domain) in cases {
        let failure = classify_llm_failure(&error);
        assert_eq!(failure.kind, kind);
        assert_eq!(failure.domain, domain);
    }
}

#[test]
fn billing_labeled_429_is_not_retryable() {
    let error = llm_http_status_error(
        "openai",
        429,
        r#"{"error":{"code":"insufficient_quota","message":"insufficient credits"}}"#,
    );

    assert_eq!(classify_llm_failure(&error).kind, LlmFailureKind::Billing);
    assert!(
        !is_retryable_llm_error(&error),
        "a quota-shaped 429 must not inherit generic rate-limit retryability"
    );
}

#[test]
fn llm_failure_classifies_402_without_body_evidence_as_billing() {
    let failure = classify_llm_failure(&llm_http_status_error("transit", 402, "Payment Required"));

    assert_eq!(failure.kind, LlmFailureKind::Billing);
    assert_eq!(failure.domain, FailureDomain::Account);
}

#[test]
fn is_unsupported_multimodal_text_matches_truth_table() {
    for sample in [
        "[OneOfParam] [input[0].content[1]] [invalid_enum_value] Invalid value: 'input_file'. Supported values are: 'input_text'.",
        "[invalid_enum_value] Invalid value: 'input_image'. Supported values are: 'input_text'.",
        "The selected model does not support image input for this request.",
        "unsupported content type",
    ] {
        assert!(
            is_unsupported_multimodal_text(sample),
            "sample 应命中 unsupported multimodal: {sample}"
        );
    }

    for sample in [
        "context_length_exceeded",
        r#"{"error":{"message":"invalid model"}}"#,
        "",
        "content_filter",
    ] {
        assert!(
            !is_unsupported_multimodal_text(sample),
            "sample 不应命中 unsupported multimodal: {sample}"
        );
    }
}

#[test]
fn is_deterministic_stream_refusal_text_matches_truth_table() {
    assert!(is_deterministic_stream_refusal_text("content_filter"));
    assert!(is_deterministic_stream_refusal_text(
        "response incomplete because of content_filter policy"
    ));

    for sample in [
        "[OneOfParam] [input[0].content[1]] [invalid_enum_value] Invalid value: 'input_file'. Supported values are: 'input_text'.",
        "unsupported content type",
        "context_length_exceeded",
        r#"{"error":{"message":"invalid model"}}"#,
        "",
    ] {
        assert!(
            !is_deterministic_stream_refusal_text(sample),
            "sample 不应命中 deterministic refusal: {sample}"
        );
    }
}
