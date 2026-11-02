//! Translate native events only after the connection writer applies backpressure.
//! Native lifecycle/control frames never become ACP extension notifications.

use agent_client_protocol::schema::v1 as acp;
use serde_json::Value;

pub(super) fn notification(
    session_id: &str,
    update: acp::SessionUpdate,
) -> Result<String, crate::AppError> {
    let params = acp::SessionNotification::new(session_id.to_owned(), update);
    Ok(serde_json::to_string(&serde_json::json!({
        "jsonrpc": "2.0", "method": "session/update", "params": params,
    }))?)
}

pub(super) fn text_update(text: String, thought: bool) -> acp::SessionUpdate {
    let chunk = acp::ContentChunk::new(text.into());
    if thought {
        acp::SessionUpdate::AgentThoughtChunk(chunk)
    } else {
        acp::SessionUpdate::AgentMessageChunk(chunk)
    }
}

pub(super) fn event_update(event: &Value) -> Option<acp::SessionUpdate> {
    match event["type"].as_str()? {
        "message_update" => {
            let message = &event["assistantMessageEvent"];
            let thought = match message["kind"].as_str()? {
                "content_delta" => false,
                "thinking_delta" => true,
                _ => return None,
            };
            Some(text_update(message["delta"].as_str()?.to_owned(), thought))
        }
        "tool_execution_start" => {
            let name = event["toolName"].as_str().unwrap_or("tool");
            Some(acp::SessionUpdate::ToolCall(
                acp::ToolCall::new(event["toolCallId"].as_str()?.to_owned(), name)
                    .name(name.to_owned())
                    .kind(tool_kind(name))
                    .status(acp::ToolCallStatus::InProgress)
                    .raw_input(event.get("args").cloned()),
            ))
        }
        "tool_execution_update" | "tool_execution_end" => {
            let finished = event["type"] == "tool_execution_end";
            let result = if finished {
                &event["result"]
            } else {
                &event["partialResult"]
            };
            let status = if !finished {
                acp::ToolCallStatus::InProgress
            } else if event["isError"] == true {
                acp::ToolCallStatus::Failed
            } else {
                acp::ToolCallStatus::Completed
            };
            let mut fields = acp::ToolCallUpdateFields::new().status(status);
            if !result.is_null() {
                fields = fields
                    .raw_output(result.clone())
                    .content(vec![acp::ToolCallContent::from(render_result(result))]);
            }
            Some(acp::SessionUpdate::ToolCallUpdate(
                acp::ToolCallUpdate::new(event["toolCallId"].as_str()?.to_owned(), fields),
            ))
        }
        _ => None,
    }
}

/// Replay persisted chat messages; provider continuity blobs and internal session
/// records are deliberately not presented as conversation content.
pub(super) fn history_updates(entry: &Value) -> Vec<acp::SessionUpdate> {
    let message = &entry["message"];
    let role = message["role"].as_str().unwrap_or("");
    let mut updates = Vec::new();
    if role == "assistant" {
        if let Some(thought) = message["thinking_text"].as_str().filter(|s| !s.is_empty()) {
            updates.push(text_update(thought.to_owned(), true));
        }
    }
    let content = &message["content"];
    let text = if let Some(text) = content.as_str() {
        text.to_owned()
    } else if let Some(parts) = content.as_array() {
        parts
            .iter()
            .map(|part| {
                if let Some(text) = part["text"].as_str() {
                    text.to_owned()
                } else {
                    format!(
                        "[Saved attachment: {}]",
                        part["filename"]
                            .as_str()
                            .or_else(|| part["label"].as_str())
                            .or_else(|| part["type"].as_str())
                            .unwrap_or("attachment")
                    )
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        String::new()
    };
    match role {
        "user" if !text.is_empty() => updates.push(acp::SessionUpdate::UserMessageChunk(
            acp::ContentChunk::new(text.into()),
        )),
        "assistant" => {
            if !text.is_empty() {
                updates.push(text_update(text, false));
            }
            if let Some(calls) = message["tool_calls"].as_array() {
                for call in calls {
                    if let (Some(id), Some(name)) =
                        (call["id"].as_str(), call["function"]["name"].as_str())
                    {
                        let arguments = &call["function"]["arguments"];
                        let arguments = arguments
                            .as_str()
                            .and_then(|text| serde_json::from_str(text).ok())
                            .unwrap_or_else(|| arguments.clone());
                        updates.push(acp::SessionUpdate::ToolCall(
                            acp::ToolCall::new(id.to_owned(), name)
                                .name(name.to_owned())
                                .kind(tool_kind(name))
                                .raw_input(arguments),
                        ));
                    }
                }
            }
        }
        "tool" => {
            if let Some(id) = message["tool_call_id"].as_str() {
                let status = message["is_error"].as_bool().map(|error| {
                    if error {
                        acp::ToolCallStatus::Failed
                    } else {
                        acp::ToolCallStatus::Completed
                    }
                });
                let fields = acp::ToolCallUpdateFields::new()
                    .status(status)
                    .raw_output(content.clone())
                    .content(vec![acp::ToolCallContent::from(text)]);
                updates.push(acp::SessionUpdate::ToolCallUpdate(
                    acp::ToolCallUpdate::new(id.to_owned(), fields),
                ));
            }
        }
        _ => {}
    }
    updates
}

fn render_result(result: &Value) -> String {
    if let Some(text) = result.as_str().or_else(|| result["output"].as_str()) {
        text.to_owned()
    } else {
        result.to_string()
    }
}

fn tool_kind(name: &str) -> acp::ToolKind {
    match name {
        "read" | "list_dir" => acp::ToolKind::Read,
        "write" | "edit" | "hashline_edit" => acp::ToolKind::Edit,
        "bash" | "task_output" | "task_stop" => acp::ToolKind::Execute,
        "search_files" | "web_search" => acp::ToolKind::Search,
        "web_fetch" => acp::ToolKind::Fetch,
        _ => acp::ToolKind::Other,
    }
}
