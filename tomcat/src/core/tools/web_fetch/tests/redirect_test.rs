use reqwest::Url;

use super::super::redirect::is_permitted_redirect;

#[test]
fn redirect_same_host_followed() {
    let from = Url::parse("https://example.com/start").unwrap();
    let to = Url::parse("https://example.com/landing").unwrap();
    assert!(is_permitted_redirect(&from, &to));
}

#[test]
fn redirect_apex_to_www_followed() {
    let from = Url::parse("https://lvh.me/start").unwrap();
    let to = Url::parse("https://www.lvh.me/landing").unwrap();
    assert!(is_permitted_redirect(&from, &to));
}

#[test]
fn redirect_www_to_apex_followed() {
    let from = Url::parse("https://www.lvh.me/start").unwrap();
    let to = Url::parse("https://lvh.me/landing").unwrap();
    assert!(is_permitted_redirect(&from, &to));
}

#[test]
fn redirect_to_subdomain_returns_structured() {
    let from = Url::parse("https://example.com/start").unwrap();
    let to = Url::parse("https://docs.example.com/landing").unwrap();
    assert!(!is_permitted_redirect(&from, &to));
}

#[test]
fn redirect_cross_scheme_rejected() {
    let from = Url::parse("https://example.com/start").unwrap();
    let to = Url::parse("http://example.com/landing").unwrap();
    assert!(!is_permitted_redirect(&from, &to));
}
