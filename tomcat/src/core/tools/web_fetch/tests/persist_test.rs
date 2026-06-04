use super::super::persist::{effective_content_type, persist_binary, persist_text};

#[tokio::test]
async fn pdf_persisted_to_tool_results() {
    let dir = tempfile::tempdir().unwrap();
    let url = reqwest::Url::parse("https://example.com/file.pdf").unwrap();
    let body = b"%PDF-1.7\nhello";
    let path = persist_binary(dir.path(), &url, "application/pdf", body)
        .await
        .expect("persist");
    assert!(path.ends_with(".pdf"));
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(bytes, body);
}

#[tokio::test]
async fn png_persisted_to_tool_results() {
    let dir = tempfile::tempdir().unwrap();
    let url = reqwest::Url::parse("https://example.com/file.png").unwrap();
    let body = b"\x89PNG\r\n\x1a\npayload";
    let path = persist_binary(dir.path(), &url, "image/png", body)
        .await
        .expect("persist");
    assert!(path.ends_with(".png"));
}

#[tokio::test]
async fn markdown_text_persist_uses_requested_extension() {
    let dir = tempfile::tempdir().unwrap();
    let url = reqwest::Url::parse("https://example.com/article").unwrap();
    let path = persist_text(dir.path(), &url, "md", "# Title\n")
        .await
        .expect("persist");
    assert!(path.ends_with(".md"));
}

#[test]
fn magic_overrides_content_type_when_mismatch() {
    let content_type = effective_content_type("text/plain", b"%PDF-1.7\nhello");
    assert_eq!(content_type, "application/pdf");
}
