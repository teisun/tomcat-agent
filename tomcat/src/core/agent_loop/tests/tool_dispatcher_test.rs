use super::tool_parallelism_event;

#[test]
fn parallelism_metric_test() {
    assert_eq!(
        tool_parallelism_event(4, 4, false),
        serde_json::json!({
            "event": "agent.tool_parallelism",
            "tool_calls_per_turn": 4,
            "tool_results_completed": 4,
            "steered": false,
        })
    );
}

use super::super::tool_exec::media::extract_mcp_tool_result_media;
use crate::core::llm::ChatMessageContentPart;

const TINY_PNG_B64: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9Y9p8qAAAAAASUVORK5CYII=";

#[tokio::test]
async fn mcp_image_block_becomes_input_image() {
    let result = serde_json::json!({
        "content": {
            "content": [
                { "type": "text", "text": "captured" },
                { "type": "image", "mimeType": "image/png", "data": TINY_PNG_B64 }
            ]
        }
    });
    let outcome = extract_mcp_tool_result_media(&result, None).await;

    assert!(outcome.model_text.contains("captured"));
    assert_eq!(outcome.follow_up_parts.len(), 1);
    assert!(matches!(
        outcome.follow_up_parts.first(),
        Some(ChatMessageContentPart::InputImage { .. })
    ));
}

#[tokio::test]
async fn text_only_plugin_result_preserves_prior_empty_follow_up_parts_behavior() {
    let result = serde_json::json!({ "content": "plugin text" });
    let outcome = extract_mcp_tool_result_media(&result, None).await;

    assert_eq!(outcome.model_text, "plugin text");
    assert!(outcome.follow_up_parts.is_empty());
    assert!(!outcome.is_error);
}

#[tokio::test]
async fn text_block_becomes_model_text() {
    let result = serde_json::json!({
        "content": { "content": [{ "type": "text", "text": "MCP text result" }] }
    });

    let outcome = extract_mcp_tool_result_media(&result, None).await;

    assert_eq!(outcome.model_text, "MCP text result");
    assert!(outcome.follow_up_parts.is_empty());
}

#[tokio::test]
async fn unknown_block_becomes_a_text_summary() {
    let result = serde_json::json!({
        "content": { "content": [{ "type": "resource", "uri": "file:///report.txt" }] }
    });

    let outcome = extract_mcp_tool_result_media(&result, None).await;

    assert!(outcome
        .model_text
        .contains("Unsupported MCP content block 'resource'"));
    assert!(outcome.model_text.contains("file:///report.txt"));
    assert!(outcome.follow_up_parts.is_empty());
}
