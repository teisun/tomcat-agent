use crate::core::{ChatMessage, ChatRequest, Tool};
use crate::infra::error::AppError;

/// `agent.sendMessage` → 当前会话 transcript  wire 格式（role + content）。
pub(super) fn agent_send_message_wire(
    params: &serde_json::Value,
) -> Result<serde_json::Value, AppError> {
    let opts = params.get("options").and_then(|v| v.as_object());
    let role_default = opts
        .and_then(|o| o.get("role"))
        .and_then(|v| v.as_str())
        .unwrap_or("user");
    let message = params
        .get("message")
        .ok_or_else(|| AppError::Plugin("sendMessage: missing message".into()))?;
    if let Some(obj) = message.as_object() {
        let role = obj
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or(role_default);
        let content = obj
            .get("content")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        return Ok(serde_json::json!({ "role": role, "content": content }));
    }
    if let Some(s) = message.as_str() {
        return Ok(serde_json::json!({ "role": role_default, "content": s }));
    }
    Ok(serde_json::json!({ "role": role_default, "content": message }))
}

/// 规整 TypeBox / 包装型 `parameters` 为 JSON Schema 风格，便于 LLM tools。
pub(super) fn normalize_tool_parameters(params: &serde_json::Value) -> serde_json::Value {
    match params {
        serde_json::Value::Null => serde_json::json!({
            "type": "object",
            "properties": {}
        }),
        serde_json::Value::Object(map) => {
            if map.len() == 1 {
                if let Some(inner) = map.get("schema") {
                    return normalize_tool_parameters(inner);
                }
            }
            let mut out = params.clone();
            if let Some(o) = out.as_object_mut() {
                o.remove("default");
                let has_shape = o.contains_key("type")
                    || o.contains_key("properties")
                    || o.contains_key("anyOf")
                    || o.contains_key("oneOf")
                    || o.contains_key("allOf")
                    || o.contains_key("items")
                    || o.contains_key("enum")
                    || o.contains_key("const");
                if has_shape {
                    return out;
                }
                if o.is_empty() {
                    return serde_json::json!({ "type": "object", "properties": {} });
                }
                return serde_json::json!({ "type": "object", "properties": out.clone() });
            }
            serde_json::json!({ "type": "object", "properties": {} })
        }
        _ => serde_json::json!({ "type": "object", "properties": {} }),
    }
}

/// Extract the `id` field from any [`TranscriptEntry`] variant.
pub(super) fn transcript_entry_id(entry: &crate::core::session::TranscriptEntry) -> Option<&str> {
    use crate::core::session::TranscriptEntry;
    match entry {
        TranscriptEntry::Message(e) => e.id.as_deref(),
        TranscriptEntry::ModelChange(e) => e.id.as_deref(),
        TranscriptEntry::ThinkingLevelChange(e) => e.id.as_deref(),
        TranscriptEntry::ThinkingTrace(e) => e.id.as_deref(),
        TranscriptEntry::BranchSummary(e) => e.id.as_deref(),
        TranscriptEntry::Label(e) => e.id.as_deref(),
        TranscriptEntry::SessionInfo(e) => e.id.as_deref(),
        TranscriptEntry::Custom(e) => e.id.as_deref(),
    }
}

pub(super) fn parse_chat_request(params: &serde_json::Value) -> Result<ChatRequest, AppError> {
    let messages: Vec<ChatMessage> = params
        .get("messages")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    let model = params
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("default")
        .to_string();
    Ok(ChatRequest {
        messages,
        model,
        temperature: params
            .get("temperature")
            .and_then(|v| v.as_f64())
            .map(|f| f as f32),
        max_tokens: params
            .get("maxTokens")
            .or_else(|| params.get("max_tokens"))
            .and_then(|v| v.as_u64())
            .map(|u| u as u32),
        stream: params.get("stream").and_then(|v| v.as_bool()),
        model_override: None,
        tools: None,
    })
}

pub(super) fn parse_tool(params: &serde_json::Value, plugin_id: &str) -> Result<Tool, AppError> {
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AppError::Plugin("registerTool: missing name".to_string()))?
        .to_string();
    let label = params
        .get("label")
        .and_then(|v| v.as_str())
        .unwrap_or(&name)
        .to_string();
    let description = params
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let raw_params = params
        .get("parameters")
        .cloned()
        .unwrap_or(serde_json::json!({}));
    let parameters = normalize_tool_parameters(&raw_params);
    let created_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Ok(Tool {
        name,
        label,
        description,
        parameters,
        plugin_id: plugin_id.to_string(),
        is_enabled: true,
        created_at,
    })
}

pub(super) fn plugin_id_from_instance(instance_id: &str) -> &str {
    instance_id
        .rsplit_once('/')
        .map(|(_, plugin_id)| plugin_id)
        .unwrap_or(instance_id)
}
