//! MCP 工具结果的媒体分流。
//!
//! 所有抵达 agent loop 的 MCP 结果，无论来自历史上的 registry 直连、v2 的
//! `tool_call`，还是 `tool_run_code`，都必须经过这一处：图片先抽成
//! `InputImage`，文本才可能被后续路径截断。

use crate::core::llm::{openai_files::OpenAiFilesRuntime, ChatMessageContentPart};

use super::ToolExecOutcome;

pub(in crate::core::agent_loop) async fn extract_mcp_tool_result_media(
    result: &serde_json::Value,
    _files_runtime: Option<&std::sync::Arc<OpenAiFilesRuntime>>,
) -> ToolExecOutcome {
    let outer_content = result.get("content").unwrap_or(result);
    // Plugin tools historically return a string directly. MCP's CallToolResult
    // is wrapped by DefaultToolRegistry as { content: { content: [...] } }, so
    // unwrap exactly one nested content field without changing text-only plugins.
    let content = outer_content.get("content").unwrap_or(outer_content);
    let mut text = Vec::new();
    let mut follow_up_parts = Vec::new();

    match content {
        serde_json::Value::String(value) => text.push(value.clone()),
        serde_json::Value::Array(blocks) => {
            for block in blocks {
                match block.get("type").and_then(serde_json::Value::as_str) {
                    Some("text") => {
                        if let Some(value) = block.get("text").and_then(serde_json::Value::as_str) {
                            text.push(value.to_string());
                        }
                    }
                    Some("image") => match mcp_image_part(block) {
                        Ok(part) => {
                            follow_up_parts.push(part);
                            text.push("[Image attached]".to_string());
                        }
                        Err(error) => text.push(format!("[MCP image omitted: {error}]")),
                    },
                    Some(kind) => text.push(format!(
                        "[Unsupported MCP content block '{kind}': {}]",
                        serde_json::to_string(block).unwrap_or_else(|_| "{}".to_string())
                    )),
                    None => {
                        text.push(serde_json::to_string(block).unwrap_or_else(|_| "{}".to_string()))
                    }
                }
            }
        }
        other => text.push(serde_json::to_string(other).unwrap_or_else(|_| "{}".to_string())),
    }

    ToolExecOutcome {
        model_text: text.join("\n"),
        is_error: outer_content
            .get("isError")
            .or_else(|| result.get("isError"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        follow_up_parts,
        display: None,
    }
}

fn mcp_image_part(block: &serde_json::Value) -> Result<ChatMessageContentPart, String> {
    let mime_type = block
        .get("mimeType")
        .or_else(|| block.get("mime_type"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "image block is missing mimeType".to_string())?;
    let data = block
        .get("data")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "image block is missing base64 data".to_string())?;
    // Inline only in actual tool traffic. Files APIs remain available to explicit callers.

    ChatMessageContentPart::image_base64_data(mime_type, data).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
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
        ) -> Result<crate::core::llm::openai_files::OpenAiFileMeta, crate::infra::AppError>
        {
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
            let out=extract_mcp_tool_result_media(&serde_json::json!({"content":[{"type":"image","mimeType":"image/png","data":data}]}),Some(&runtime)).await;
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
        let outcome = extract_mcp_tool_result_media(
            &serde_json::json!({
                "content": [{ "type": "text", "text": "the deferred call failed" }],
                "isError": true,
            }),
            None,
        )
        .await;

        assert!(outcome.is_error);
        assert_eq!(outcome.model_text, "the deferred call failed");
    }

    #[tokio::test]
    async fn registry_wrapped_mcp_error_result_preserves_is_error() {
        let outcome = extract_mcp_tool_result_media(
            &serde_json::json!({
                "content": {
                    "content": [{ "type": "text", "text": "the registry call failed" }],
                    "isError": true,
                },
            }),
            None,
        )
        .await;

        assert!(outcome.is_error);
        assert_eq!(outcome.model_text, "the registry call failed");
    }

    #[tokio::test]
    async fn absent_or_false_is_error_remains_successful_for_both_shapes() {
        for result in [
            serde_json::json!({
                "content": [{ "type": "text", "text": "direct false" }],
                "isError": false,
            }),
            serde_json::json!({
                "content": [{ "type": "text", "text": "direct absent" }],
            }),
            serde_json::json!({
                "content": {
                    "content": [{ "type": "text", "text": "wrapped false" }],
                    "isError": false,
                },
            }),
            serde_json::json!({
                "content": {
                    "content": [{ "type": "text", "text": "wrapped absent" }],
                },
            }),
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
            &serde_json::json!({
                "content": [
                    { "type": "text", "text": "capture complete" },
                    { "type": "image", "mimeType": "image/png", "data": TINY_PNG_B64 },
                    { "type": "resource", "resource": { "uri": "file:///report.txt" } },
                    { "untagged": "data" },
                ],
            }),
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
}
