use super::extract_mcp_tool_result_media;
use crate::core::llm::ChatMessageContentPart;

const TINY_PNG_B64: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9Y9p8qAAAAAASUVORK5CYII=";

#[derive(Debug)]
struct NeverUpload;
#[async_trait::async_trait]
impl crate::core::llm::files_api::FilesApiAdapter for NeverUpload {
    async fn upload(
        &self,
        _: crate::core::llm::openai_files::FilePurpose,
        _: &str,
        _: &str,
        _: &[u8],
    ) -> Result<crate::core::llm::openai_files::OpenAiFileMeta, crate::infra::AppError> {
        panic!("normal tool traffic must never upload");
    }
    async fn delete(&self, _: &str) -> Result<(), crate::infra::AppError> {
        panic!("no upload means no delete");
    }
    fn expires_after_seconds(&self) -> u64 {
        0
    }
}

#[tokio::test]
async fn tool_owned_media_mcp_inline_even_with_files_runtime_and_clear_oversize() {
    use base64::Engine as _;
    let temp = tempfile::tempdir().unwrap();
    let runtime = std::sync::Arc::new(
        crate::core::llm::openai_files::OpenAiFilesRuntime::new_with_adapter(
            std::sync::Arc::new(NeverUpload),
            temp.path().join("files.json"),
        ),
    );
    for size in [1024 * 1024 + 1, crate::core::llm::IMAGE_MAX_BYTES + 1] {
        let data = base64::engine::general_purpose::STANDARD.encode(vec![0u8; size]);
        let out = extract_mcp_tool_result_media(
            &serde_json::json!({"content":[{"type":"image","mimeType":"image/png","data":data}]}),
            Some(&runtime),
        )
        .await;
        if size <= crate::core::llm::IMAGE_MAX_BYTES {
            assert!(matches!(
                &out.follow_up_parts[0],
                ChatMessageContentPart::InputImage {
                    source: crate::core::llm::ImageSource::Inline(_),
                    ..
                }
            ));
        } else {
            assert!(out.follow_up_parts.is_empty());
            assert!(out.model_text.contains("IMAGE_MAX_BYTES"));
        }
    }
}

#[tokio::test]
async fn direct_mcp_error_result_preserves_is_error() {
    let outcome = extract_mcp_tool_result_media(&serde_json::json!({ "content": [{ "type": "text", "text": "the deferred call failed" }], "isError": true }), None).await;
    assert!(outcome.is_error);
    assert_eq!(outcome.model_text, "the deferred call failed");
}

#[tokio::test]
async fn registry_wrapped_mcp_error_result_preserves_is_error() {
    let outcome = extract_mcp_tool_result_media(&serde_json::json!({ "content": { "content": [{ "type": "text", "text": "the registry call failed" }], "isError": true } }), None).await;
    assert!(outcome.is_error);
    assert_eq!(outcome.model_text, "the registry call failed");
}

#[tokio::test]
async fn absent_or_false_is_error_remains_successful_for_both_shapes() {
    for result in [
        serde_json::json!({"content": [{"type":"text", "text":"direct false"}], "isError": false}),
        serde_json::json!({"content": [{"type":"text", "text":"direct absent"}]}),
        serde_json::json!({"content": {"content": [{"type":"text", "text":"wrapped false"}], "isError": false}}),
        serde_json::json!({"content": {"content": [{"type":"text", "text":"wrapped absent"}]}}),
    ] {
        assert!(
            !extract_mcp_tool_result_media(&result, None).await.is_error,
            "a missing or false MCP isError must remain successful: {result}"
        );
    }
}

#[tokio::test]
async fn media_and_nonstandard_blocks_are_normalized_without_losing_text() {
    let outcome = extract_mcp_tool_result_media(
        &serde_json::json!({"content": [
            {"type": "text", "text": "capture complete"},
            {"type": "image", "mimeType": "image/png", "data": TINY_PNG_B64},
            {"type": "resource", "resource": {"uri": "file:///report.txt"}},
            {"untagged": "data"},
        ]}),
        None,
    )
    .await;
    assert!(!outcome.is_error);
    assert_eq!(outcome.follow_up_parts.len(), 1);
    assert!(matches!(
        outcome.follow_up_parts.as_slice(),
        [ChatMessageContentPart::InputImage { .. }]
    ));
    assert!(outcome.model_text.contains("capture complete"));
    assert!(outcome.model_text.contains("[Image attached]"));
    assert!(outcome
        .model_text
        .contains("[Unsupported MCP content block 'resource':"));
    assert!(outcome.model_text.contains(r#"{"untagged":"data"}"#));
}
