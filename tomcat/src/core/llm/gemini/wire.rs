//! Native Google Gemini `/v1beta/models` request assembly.

use std::collections::HashMap;

use serde_json::{json, Map, Value};

use crate::core::llm::catalog::Capabilities;
use crate::core::llm::multimodal::degrade_placeholder;
use crate::core::llm::replay_policy::{
    plan, ProviderCompatProfile, ReplayAction, ReplayDowngradeReport,
};
use crate::core::llm::thinking_policy::{
    clamp_reasoning_level, gemini_thinking_level, ThinkingLevel,
};
use crate::core::llm::types::{
    ephemeral_tail_texts, is_ephemeral_tail, ChatMessage, ChatMessageContent,
    ChatMessageContentPart, ChatMessageRole, FileSource, ImageSource, ReasoningFormat,
};
use crate::infra::config::ThinkingConfig;

pub(super) const SKIP_THOUGHT_SIGNATURE_VALIDATOR: &str = "skip_thought_signature_validator";
pub(super) const TOOL_TURN_BRIDGE: &str = "[Tool results received.]";
const MAX_REF_EXPANSIONS: usize = 256;

#[derive(Clone, Copy, PartialEq, Eq)]
enum TurnKind {
    UserContent,
    ToolResponses,
    Model,
}

impl TurnKind {
    fn role(self) -> &'static str {
        match self {
            Self::UserContent | Self::ToolResponses => "user",
            Self::Model => "model",
        }
    }
}

struct WireTurn {
    kind: TurnKind,
    parts: Vec<Value>,
}

pub(super) fn build_request_body(
    request: &crate::core::llm::ChatRequest,
    target: &ProviderCompatProfile,
    thinking_cfg: &ThinkingConfig,
    supported_reasoning_levels: &[String],
    continuity_enabled: bool,
    capabilities: &Capabilities,
) -> Value {
    let (contents, system_instruction) =
        build_contents_and_system(&request.messages, target, continuity_enabled, capabilities);

    let mut body = Map::new();
    if let Some(sys) = system_instruction {
        body.insert("systemInstruction".to_string(), sys);
    }
    body.insert("contents".to_string(), Value::Array(contents));

    if let Some(tools) = request.tools.as_deref().and_then(convert_tools) {
        body.insert("tools".to_string(), tools);
    }

    let mut gen_cfg = Map::new();
    if let Some(temp) = request.temperature {
        gen_cfg.insert("temperature".to_string(), json!(temp));
    }
    if let Some(max_out) = request.resolved_output_limit.or(request.max_tokens) {
        gen_cfg.insert("maxOutputTokens".to_string(), json!(max_out));
    }
    if capabilities.reasoning && thinking_cfg.enabled {
        let (raw_level, _) = ThinkingLevel::parse_or_medium(&thinking_cfg.level);
        let effective_level = clamp_reasoning_level(raw_level, supported_reasoning_levels);
        if let Some(level_token) = gemini_thinking_level(effective_level) {
            gen_cfg.insert(
                "thinkingConfig".to_string(),
                json!({
                    "includeThoughts": true,
                    "thinkingLevel": level_token,
                }),
            );
        }
    }
    if !gen_cfg.is_empty() {
        body.insert("generationConfig".to_string(), Value::Object(gen_cfg));
    }

    Value::Object(body)
}

fn build_contents_and_system(
    messages: &[ChatMessage],
    target: &ProviderCompatProfile,
    continuity_enabled: bool,
    capabilities: &Capabilities,
) -> (Vec<Value>, Option<Value>) {
    let mut report = ReplayDowngradeReport::default();
    let mut system_parts: Vec<Value> = Vec::new();
    let mut turns: Vec<WireTurn> = Vec::new();
    let mut tool_name_by_id: HashMap<&str, &str> = HashMap::new();
    let mut tool_wire_id_by_id: HashMap<&str, Option<String>> = HashMap::new();
    let mut last_call_order: Vec<&str> = Vec::new();
    let mut pending_tool_responses: Vec<(&str, Value)> = Vec::new();

    for msg in messages {
        if is_ephemeral_tail(msg) {
            continue;
        }
        if msg.role != ChatMessageRole::Tool {
            flush_pending_tool_responses(&mut turns, &mut pending_tool_responses, &last_call_order);
        }

        let action = if continuity_enabled {
            plan(target, msg)
        } else {
            ReplayAction::StripOpaque
        };
        if continuity_enabled {
            report.record_replay_decision(target, msg, &action);
        }

        match msg.role {
            ChatMessageRole::System => {
                let text = flatten_message_text(msg);
                if !text.trim().is_empty() {
                    system_parts.push(json!({ "text": text }));
                }
            }
            ChatMessageRole::User => {
                let parts = user_parts(msg, capabilities);
                push_turn(
                    &mut turns,
                    WireTurn {
                        kind: TurnKind::UserContent,
                        parts,
                    },
                );
            }
            ChatMessageRole::Assistant => {
                let original_parts = if matches!(action, ReplayAction::KeepOpaque) {
                    msg.reasoning_continuation
                        .as_ref()
                        .filter(|c| c.format == ReasoningFormat::GeminiThoughtSignatures)
                        .and_then(|c| c.opaque_payload.as_array())
                } else {
                    None
                };
                let mut parts = original_parts
                    .cloned()
                    .unwrap_or_else(|| rebuild_assistant_parts(msg));
                if let Some(first_call) = parts
                    .iter_mut()
                    .find(|part| part.get("functionCall").is_some())
                {
                    if first_call
                        .get("thoughtSignature")
                        .and_then(Value::as_str)
                        .is_none_or(|sig| sig.is_empty())
                    {
                        first_call["thoughtSignature"] = json!(SKIP_THOUGHT_SIGNATURE_VALIDATOR);
                    }
                }
                last_call_order.clear();
                let mut wire_calls = parts.iter().filter_map(|part| part.get("functionCall"));
                for tc in msg.tool_calls.iter().flatten() {
                    let wire_call = wire_calls.next();
                    if let Some(id) = tc
                        .get("id")
                        .and_then(Value::as_str)
                        .filter(|id| !id.is_empty())
                    {
                        if let Some(name) = tc["function"]["name"].as_str() {
                            tool_name_by_id.insert(id, name);
                        }
                        tool_wire_id_by_id.insert(
                            id,
                            wire_call
                                .and_then(|call| call.get("id"))
                                .and_then(Value::as_str)
                                .map(str::to_owned),
                        );
                        last_call_order.push(id);
                    }
                }

                push_turn(
                    &mut turns,
                    WireTurn {
                        kind: TurnKind::Model,
                        parts,
                    },
                );
            }
            ChatMessageRole::Tool => {
                let id = msg.tool_call_id.as_deref().filter(|id| !id.is_empty());
                let name = id
                    .and_then(|id| tool_name_by_id.get(id).copied())
                    .or_else(|| msg.name.as_deref().filter(|name| !name.is_empty()))
                    .unwrap_or("unknown_tool");
                let wire_id = id.and_then(|id| {
                    tool_wire_id_by_id
                        .get(id)
                        .map_or(Some(id), |wire_id| wire_id.as_deref())
                });
                let part = build_function_response_part(msg, wire_id, name, capabilities);
                pending_tool_responses.push((id.unwrap_or_default(), part));
            }
        }
    }

    flush_pending_tool_responses(&mut turns, &mut pending_tool_responses, &last_call_order);

    for tail_text in ephemeral_tail_texts(messages) {
        system_parts.push(json!({ "text": tail_text }));
    }

    if continuity_enabled {
        report.emit(target);
    }

    let system_instruction = (!system_parts.is_empty()).then(|| json!({ "parts": system_parts }));
    let contents = turns
        .into_iter()
        .map(|turn| {
            json!({
                "role": turn.kind.role(),
                "parts": turn.parts,
            })
        })
        .collect();
    (contents, system_instruction)
}

/// Visible-content fallback for foreign routes, disabled continuity and unpublished
/// dictionary payloads. Only native part arrays are eligible for opaque replay.
fn rebuild_assistant_parts(message: &ChatMessage) -> Vec<Value> {
    let mut parts = Vec::new();
    let text = flatten_message_text(message);
    if !text.is_empty() {
        parts.push(json!({"text": text}));
    }
    for tc in message.tool_calls.iter().flatten() {
        let function = &tc["function"];
        let mut call = Map::new();
        if let Some(id) = tc["id"].as_str().filter(|id| !id.is_empty()) {
            call.insert("id".into(), json!(id));
        }
        call.insert(
            "name".into(),
            json!(function["name"].as_str().unwrap_or_default()),
        );
        call.insert(
            "args".into(),
            parse_args_object(function["arguments"].as_str().unwrap_or("{}")),
        );
        parts.push(json!({"functionCall": call}));
    }
    parts
}

fn flush_pending_tool_responses(
    turns: &mut Vec<WireTurn>,
    pending: &mut Vec<(&str, Value)>,
    call_order: &[&str],
) {
    if pending.is_empty() {
        return;
    }
    let mut batch = std::mem::take(pending);
    if !call_order.is_empty() {
        batch.sort_by_key(|(id, _)| {
            call_order
                .iter()
                .position(|expected| expected == id)
                .unwrap_or(usize::MAX)
        });
    }
    let parts = batch.into_iter().map(|(_, part)| part).collect();
    push_turn(
        turns,
        WireTurn {
            kind: TurnKind::ToolResponses,
            parts,
        },
    );
}

fn push_turn(turns: &mut Vec<WireTurn>, next: WireTurn) {
    if next.parts.is_empty() {
        return;
    }
    match turns.last_mut() {
        // Human content after tool results needs a model bridge so it is not
        // mistaken for a continuation of functionResponse (gemini-cli#28700).
        Some(prev)
            if prev.kind == TurnKind::ToolResponses && next.kind == TurnKind::UserContent =>
        {
            turns.push(WireTurn {
                kind: TurnKind::Model,
                parts: vec![json!({ "text": TOOL_TURN_BRIDGE })],
            });
            turns.push(next);
        }
        Some(prev) if prev.kind.role() == next.kind.role() => prev.parts.extend(next.parts),
        _ => turns.push(next),
    }
}

fn parse_args_object(raw: &str) -> Value {
    match serde_json::from_str::<Value>(raw) {
        Ok(Value::Object(map)) => Value::Object(map),
        Ok(other) => json!({ "_value": other }),
        Err(_) => json!({}),
    }
}

fn flatten_message_text(message: &ChatMessage) -> String {
    match &message.content {
        Some(ChatMessageContent::Text(text)) => text.clone(),
        Some(ChatMessageContent::Parts(parts)) => parts
            .iter()
            .filter_map(|part| match part {
                ChatMessageContentPart::InputText { text } => Some(text.clone()),
                ChatMessageContentPart::InputReference { reference } => {
                    Some(reference.to_prompt_text())
                }
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        None => String::new(),
    }
}

fn user_parts(message: &ChatMessage, capabilities: &Capabilities) -> Vec<Value> {
    match &message.content {
        Some(ChatMessageContent::Text(text)) => {
            if text.is_empty() {
                Vec::new()
            } else {
                vec![json!({ "text": text })]
            }
        }
        Some(ChatMessageContent::Parts(parts)) => parts
            .iter()
            .filter_map(|part| part_to_gemini_user_part(part, capabilities))
            .collect(),
        None => Vec::new(),
    }
}

fn part_to_gemini_user_part(
    part: &ChatMessageContentPart,
    capabilities: &Capabilities,
) -> Option<Value> {
    match part {
        ChatMessageContentPart::InputText { text } => {
            (!text.is_empty()).then(|| json!({ "text": text }))
        }
        ChatMessageContentPart::InputReference { reference } => {
            let text = reference.to_prompt_text();
            (!text.is_empty()).then(|| json!({ "text": text }))
        }
        ChatMessageContentPart::InputImage { source, .. } => {
            if !capabilities.vision {
                return Some(json!({ "text": degrade_placeholder(part) }));
            }
            match source {
                ImageSource::Inline(inline) => Some(json!({
                    "inlineData": {
                        "mimeType": inline.mime_type,
                        "data": inline.data,
                    }
                })),
                ImageSource::Uploaded(_) => Some(json!({ "text": degrade_placeholder(part) })),
            }
        }
        ChatMessageContentPart::InputImageRef { .. } => {
            Some(json!({ "text": degrade_placeholder(part) }))
        }
        ChatMessageContentPart::InputFile { source } => {
            if !capabilities.files {
                return Some(json!({ "text": degrade_placeholder(part) }));
            }
            match source {
                FileSource::Inline(inline) => Some(json!({
                    "inlineData": {
                        "mimeType": inline.mime_type,
                        "data": inline.data,
                    }
                })),
                FileSource::Uploaded(_) => Some(json!({ "text": degrade_placeholder(part) })),
            }
        }
    }
}

fn build_function_response_part(
    message: &ChatMessage,
    id: Option<&str>,
    name: &str,
    capabilities: &Capabilities,
) -> Value {
    let (texts, media): (Vec<Value>, Vec<Value>) = user_parts(message, capabilities)
        .into_iter()
        .partition(|part| part.get("text").is_some());
    let output = if texts.is_empty() && !media.is_empty() {
        "[Media attached]".to_string()
    } else {
        texts
            .iter()
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n")
    };

    // Always wrap tool text as `{"output": <string>}` without JSON parsing: Gemini 3
    // resolves `$ref` pointers inside structured `functionResponse.response` objects
    // and rejects unknown pointers with HTTP 400 (vercel/ai#14369).
    let mut fn_resp = Map::new();
    if let Some(id) = id {
        fn_resp.insert("id".to_string(), Value::String(id.to_string()));
    }
    fn_resp.insert("name".to_string(), Value::String(name.to_string()));
    fn_resp.insert("response".to_string(), json!({ "output": output }));
    if !media.is_empty() {
        fn_resp.insert("parts".to_string(), Value::Array(media));
    }

    json!({ "functionResponse": Value::Object(fn_resp) })
}

pub(super) fn convert_tools(tools: &[Value]) -> Option<Value> {
    let declarations: Vec<Value> = tools
        .iter()
        .filter_map(|tool| {
            let function = tool.get("function")?;
            let name = function.get("name")?.as_str()?.trim();
            if name.is_empty() {
                return None;
            }
            let description = function
                .get("description")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty());
            let schema = function
                .get("parameters")
                .map(prepare_gemini_tool_parameters)
                .unwrap_or_else(empty_object_schema);
            let mut decl = Map::new();
            decl.insert("name".to_string(), Value::String(name.to_string()));
            if let Some(desc) = description {
                decl.insert("description".to_string(), Value::String(desc.to_string()));
            }
            decl.insert("parametersJsonSchema".to_string(), schema);
            Some(Value::Object(decl))
        })
        .collect();

    (!declarations.is_empty()).then(|| json!([{ "functionDeclarations": declarations }]))
}

pub(super) fn prepare_gemini_tool_parameters(parameters: &Value) -> Value {
    let Some(obj) = parameters.as_object() else {
        return empty_object_schema();
    };
    if obj.is_empty() {
        return empty_object_schema();
    }
    let mut schema = Value::Object(obj.clone());
    if let Some(root_map) = schema.as_object_mut() {
        root_map.remove("$schema");
    }

    let mut budget = MAX_REF_EXPANSIONS;
    let mut stack = Vec::new();
    let inlined = match inline_local_refs(&schema, &schema, &mut budget, &mut stack) {
        Ok(resolved) => resolved,
        // Keep unresolvable / circular schemas untouched so upstream names the real error.
        Err(()) => return schema,
    };
    schema = inlined;

    if let Some(root_map) = schema.as_object_mut() {
        root_map.remove("$defs");
        root_map.remove("definitions");
        if root_map.is_empty() {
            return empty_object_schema();
        }
        if root_map.get("type").and_then(Value::as_str) == Some("object")
            && !root_map.contains_key("properties")
        {
            root_map.insert("properties".to_string(), json!({}));
        }
    }
    schema
}

fn empty_object_schema() -> Value {
    json!({
        "type": "object",
        "properties": {},
    })
}

fn resolve_local_ref<'a>(root: &'a Value, reference: &str) -> Option<&'a Value> {
    let pointer = reference.strip_prefix("#/")?;
    let mut node = root;
    for raw_part in pointer.split('/') {
        let key = raw_part.replace("~1", "/").replace("~0", "~");
        node = node.as_object()?.get(&key)?;
    }
    node.is_object().then_some(node)
}

fn inline_local_refs(
    node: &Value,
    root: &Value,
    budget: &mut usize,
    stack: &mut Vec<String>,
) -> Result<Value, ()> {
    match node {
        Value::Array(items) => items
            .iter()
            .map(|item| inline_local_refs(item, root, budget, stack))
            .collect::<Result<Vec<_>, ()>>()
            .map(Value::Array),
        Value::Object(map) => {
            let Some(ref_str) = map.get("$ref").and_then(Value::as_str) else {
                let mut out = Map::with_capacity(map.len());
                for (k, v) in map {
                    out.insert(k.clone(), inline_local_refs(v, root, budget, stack)?);
                }
                return Ok(Value::Object(out));
            };
            if stack.iter().any(|seen| seen == ref_str) || *budget == 0 {
                return Err(());
            }
            *budget -= 1;
            let target = resolve_local_ref(root, ref_str).ok_or(())?;
            stack.push(ref_str.to_string());
            let inlined_target = inline_local_refs(target, root, budget, stack);
            stack.pop();
            let mut merged = match inlined_target? {
                Value::Object(m) => m,
                other => return Ok(other),
            };
            for (k, v) in map {
                if k == "$ref" {
                    continue;
                }
                merged.insert(k.clone(), inline_local_refs(v, root, budget, stack)?);
            }
            Ok(Value::Object(merged))
        }
        other => Ok(other.clone()),
    }
}

#[cfg(test)]
#[path = "tests/wire_test.rs"]
mod tests;
