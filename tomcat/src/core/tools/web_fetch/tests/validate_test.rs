use super::super::validate::validate_input_url;

#[test]
fn url_too_long_rejected() {
    let url = format!("https://example.com/{}", "a".repeat(2_001));
    let err = validate_input_url(&url).expect_err("should reject");
    assert!(err.to_string().contains("过长"));
}

#[test]
fn url_with_credentials_rejected() {
    let err =
        validate_input_url("https://user:secret@example.com/private").expect_err("should reject");
    assert!(err.to_string().contains("credentials rejected"));
}

#[test]
fn url_invalid_scheme_rejected() {
    let err = validate_input_url("file:///tmp/demo.txt").expect_err("should reject");
    assert!(err.to_string().contains("仅允许 http/https"));
}

#[test]
fn url_single_segment_host_rejected() {
    let err = validate_input_url("https://localhost/path").expect_err("should reject");
    assert!(err.to_string().contains("local hostname rejected"));
}

#[test]
fn url_private_ip_rejected() {
    let err = validate_input_url("https://192.168.1.2/path").expect_err("should reject");
    assert!(err.to_string().contains("private or loopback IP rejected"));
}

#[test]
fn url_public_ip_literal_rejected() {
    let err = validate_input_url("https://8.8.8.8/path").expect_err("should reject");
    assert!(err.to_string().contains("IP literal host rejected"));
}

#[test]
fn http_url_upgraded_to_https_before_first_get() {
    let validated = validate_input_url("http://example.com/path?q=1").expect("validate");
    assert_eq!(validated.url.as_str(), "https://example.com/path?q=1");
}

#[test]
fn url_path_with_ask_or_task_prefix_does_not_warn_secret_prefix() {
    let validated =
        validate_input_url("https://example.com/ask-me/task-list").expect("should validate");
    assert!(!validated
        .warnings
        .iter()
        .any(|warning| warning == "secret_prefix_in_url"));
}

#[test]
fn url_query_with_secret_prefix_still_warns() {
    let validated =
        validate_input_url("https://example.com/?token=sk-xxx").expect("should validate");
    assert!(validated
        .warnings
        .iter()
        .any(|warning| warning == "secret_prefix_in_url"));
}
