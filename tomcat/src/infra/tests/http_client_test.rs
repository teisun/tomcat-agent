use crate::infra::http_client::{
    build_outbound_client, OutboundClientErrorKind, OutboundClientOptions,
};

#[test]
fn invalid_proxy_diagnostic_does_not_echo_credentials() {
    let proxy = "http://test-user:PRIVATE_PROXY_SENTINEL@proxy.invalid:bad-port";
    let error = build_outbound_client(
        OutboundClientOptions::new(Some(proxy)),
        OutboundClientErrorKind::Tool,
        "client fixture",
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("URL"), "{error}");
    assert!(!error.contains("PRIVATE_PROXY_SENTINEL"), "{error}");
    assert!(!error.contains("test-user"), "{error}");
}
