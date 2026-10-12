use super::*;

#[test]
fn failure_facts_are_typed_and_never_render_remote_secrets() {
    for (status, kind) in [
        (401, FailureKind::Authorization),
        (403, FailureKind::Forbidden),
        (404, FailureKind::Protocol),
        (503, FailureKind::Transient),
    ] {
        let error = sanitized(
            StreamableHttpError::UnexpectedServerResponse(
                "SECRET_BODY https://secret.invalid/?token=SECRET_URL".into(),
            ),
            Some(status),
        );
        let failure = McpFailure::from_error(&error, "initialize");
        assert_eq!(failure.kind, kind);
        assert_eq!(failure.http_status, Some(status));
        assert!(!format!("{error:?} {failure}").contains("SECRET"));
    }
    let expired = sanitized(StreamableHttpError::SessionExpired, Some(404));
    assert_eq!(
        McpFailure::from_error(&expired, "call").kind,
        FailureKind::SessionExpired
    );
    let auth = sanitized(
        StreamableHttpError::AuthRequired(
            rmcp::transport::streamable_http_client::AuthRequiredError::new(
                "Bearer SECRET_CHALLENGE".into(),
            ),
        ),
        Some(401),
    );
    let facts = McpFailure::from_error(&auth, "call");
    assert!(facts.challenge.unwrap().contains("SECRET_CHALLENGE"));
    assert!(!format!("{auth:?}").contains("SECRET"));
}
