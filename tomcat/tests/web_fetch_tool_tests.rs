use serial_test::serial;
use tomcat::core::tools::web_fetch::types::{WebFetchArgs, WebFetchOutput};
use tomcat::core::tools::web_fetch::WebFetchRuntime;
use tomcat::AppConfig;

#[test]
fn public_output_roundtrip_preserves_fields() {
    let output = WebFetchOutput {
        url: "https://example.com".to_string(),
        code: 200,
        code_text: "OK".to_string(),
        content_type: "text/html; charset=utf-8".to_string(),
        bytes: 123,
        result: "# Example".to_string(),
        total_chars: 9,
        duration_ms: 42,
        cached: false,
        persisted_output_path: None,
        redirect: None,
        truncated: false,
        warnings: vec!["prompt_ignored_mvp".to_string()],
    };

    let json = serde_json::to_string(&output).expect("serialize");
    let decoded: WebFetchOutput = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(decoded, output);
}

#[tokio::test]
#[serial]
async fn live_example_fetch_smoke() {
    if std::env::var("PI_LIVE_WEB_FETCH").ok().as_deref() != Some("1") {
        return;
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let cfg = AppConfig::default();
    let runtime =
        WebFetchRuntime::new(&cfg, dir.path().join("tool-results")).expect("build runtime");
    let output = runtime
        .fetch(WebFetchArgs {
            url: "https://example.com/".to_string(),
            prompt: None,
            format: Some("markdown".to_string()),
        })
        .await
        .expect("live web_fetch");

    assert_eq!(output.code, 200);
    assert!(
        !output.result.is_empty() || output.persisted_output_path.is_some(),
        "expected inline markdown or persisted output when PI_LIVE_WEB_FETCH=1"
    );
}
