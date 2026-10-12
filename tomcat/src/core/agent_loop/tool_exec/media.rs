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
#[path = "tests/media_test.rs"]
mod tests;
