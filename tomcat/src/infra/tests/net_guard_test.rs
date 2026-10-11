use super::super::net_guard::{
    validate_http_url, HttpSchemePolicy, PublicIpDnsResolver, UrlValidationOptions,
};
use crate::infra::i18n::{tr_in, Locale};
use reqwest::dns::Resolve;
use std::str::FromStr;

const MAX_URL_LEN: usize = 2048;

fn options(scheme_policy: HttpSchemePolicy) -> UrlValidationOptions {
    UrlValidationOptions {
        max_url_length: MAX_URL_LEN,
        error_prefix: "net_guard_test",
        scheme_policy,
    }
}

#[test]
fn validate_http_url_rejects_localhost_style_hostnames() {
    for url in [
        "https://localhost/path",
        "https://svc.internal/path",
        "https://printer.local/path",
        "https://router.home.arpa/path",
    ] {
        let err = validate_http_url(url, options(HttpSchemePolicy::RequireHttps))
            .expect_err("reserved local hostname should be rejected");
        assert!(
            err.to_string().contains(&tr_in(
                Locale::En,
                "net.localHost",
                &[("operation", "net_guard_test")]
            )),
            "{err}"
        );
    }
}

#[test]
fn validate_http_url_rejects_link_local_and_private_ip_literals() {
    for url in [
        "https://169.254.10.20/path",
        "https://192.168.1.2/path",
        "https://[fe80::1]/path",
    ] {
        let err = validate_http_url(url, options(HttpSchemePolicy::RequireHttps))
            .expect_err("local ip literal should be rejected");
        assert!(
            err.to_string().contains(&tr_in(
                Locale::En,
                "net.privateIp",
                &[("operation", "net_guard_test")]
            )),
            "{err}"
        );
    }
}

#[test]
fn validate_http_url_rejects_http_when_https_required() {
    let err = validate_http_url(
        "http://example.com/path",
        options(HttpSchemePolicy::RequireHttps),
    )
    .expect_err("http should be rejected");
    assert!(err.to_string().contains(&tr_in(
        Locale::En,
        "net.httpsRequired",
        &[("operation", "net_guard_test")]
    )));
}

#[test]
fn validate_http_url_can_upgrade_http_when_policy_allows() {
    let validated = validate_http_url(
        "http://example.com/path?q=1",
        options(HttpSchemePolicy::UpgradeToHttps),
    )
    .expect("http should upgrade to https");
    assert_eq!(validated.url.as_str(), "https://example.com/path?q=1");
}

#[tokio::test]
async fn public_ip_dns_resolver_rejects_localhost_resolution() {
    let resolver = PublicIpDnsResolver;
    let err = resolver
        .resolve(reqwest::dns::Name::from_str("localhost").expect("valid name"))
        .await
        .err()
        .expect("localhost should resolve to disallowed local IP");
    assert!(err.to_string().contains(
        tr_in(
            Locale::En,
            "net.dnsPrivate",
            &[("host", "localhost"), ("ip", "")]
        )
        .trim_end_matches('`')
    ));
}
