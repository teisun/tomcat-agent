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

#[test]
fn loopback_urls_are_literal_hosts_not_lookalike_domains_or_lan_addresses() {
    for host in ["127.0.0.1", "127.0.0.2", "[::1]", "localhost"] {
        let url = reqwest::Url::parse(&format!("http://{host}:1234/v1")).unwrap();
        assert!(crate::infra::http_client::is_loopback_url(&url), "{url}");
    }
    for host in ["10.0.0.1", "127.0.0.1.example.invalid", "localhost.example.invalid", "[::2]"] {
        let url = reqwest::Url::parse(&format!("http://{host}/")).unwrap();
        assert!(!crate::infra::http_client::is_loopback_url(&url), "{url}");
    }
}
