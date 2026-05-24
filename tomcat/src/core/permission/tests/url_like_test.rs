use super::super::url_like::is_url_like;

#[test]
fn detects_http_and_https() {
    assert!(is_url_like("http://127.0.0.1:4173/"));
    assert!(is_url_like("https://example.com/api"));
    assert!(is_url_like(" HTTPS://EXAMPLE.COM "));
}

#[test]
fn rejects_plain_paths_and_other_strings() {
    assert!(!is_url_like("/tmp/file"));
    assert!(!is_url_like("./src/main.rs"));
    assert!(!is_url_like("localhost:4173"));
    assert!(!is_url_like(""));
}
