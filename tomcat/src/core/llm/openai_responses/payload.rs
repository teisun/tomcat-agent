//! # `ChatRequest` ↔ `/v1/responses` 请求/响应翻译
//!
//! 本子模块只承担 **wire 协议翻译**：
//! - `build_responses_input`：把 [`ChatMessage`] 序列翻译为 Responses 的
//!   `(instructions, input items)` 二元组；
//! - `convert_tools_to_responses`：把 Chat Completions function 形状翻译为
//!   Responses 顶层 function 形状；
//! - `responses_payload_to_chat_response`：把非流式 `/v1/responses` JSON 翻译为
//!   内部 [`ChatResponse`]；
//! - 一组 `extract_text` / `part_to_responses_value` / `user_content_parts` /
//!   `warn_drop_non_text_parts` helper：仅供前两个翻译入口复用。
//!
//! 拆分前所有翻译函数与 [`super::OpenAiResponsesProvider`] 同居一文件（1056 行），
//! 拆出后 wire 翻译与 HTTP 客户端 / 流式解析解耦，单文件落 L-1。

use serde_json::{json, Value};
use tracing::warn;

use crate::core::llm::types::{
    ChatMessage, ChatMessageContent, ChatMessageContentPart, ChatMessageRole, ChatResponse,
    ChatResponseChoice,
};

pub(super) const MAX_OUTPUT_TOKENS_NOTICE: &str = "达到 max_output_tokens，回答可能未完成";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct ResponsesTerminalMetadata {
    pub finish_reason: Option<String>,
    pub error_message: Option<String>,
    pub error_code: Option<String>,
    pub notice_message: Option<String>,
}

impl ResponsesTerminalMetadata {
    fn stop() -> Self {
        Self {
            finish_reason: Some("stop".to_string()),
            ..Self::default()
        }
    }

    fn tool_calls() -> Self {
        Self {
            finish_reason: Some("tool_calls".to_string()),
            ..Self::default()
        }
    }

    fn max_output_tokens() -> Self {
        Self {
            finish_reason: Some("max_output_tokens".to_string()),
            notice_message: Some(MAX_OUTPUT_TOKENS_NOTICE.to_string()),
            ..Self::default()
        }
    }

    fn error(message: impl Into<String>, code: Option<String>) -> Self {
        let message = message.into();
        let reason_suffix = if message.is_empty() {
            code.clone().unwrap_or_else(|| "unknown".to_string())
        } else {
            message.clone()
        };
        Self {
            finish_reason: Some(format!("error:{reason_suffix}")),
            error_message: Some(message),
            error_code: code,
            notice_message: None,
        }
    }
}

fn extract_error_details(
    error: Option<&Value>,
    fallback_message: Option<&str>,
) -> Option<(String, Option<String>)> {
    let code = error
        .and_then(|err| err.get("code"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let message = error
        .and_then(|err| err.get("message"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| error.and_then(Value::as_str).map(str::to_string))
        .or_else(|| fallback_message.map(str::to_string))?;
    Some((message, code))
}

fn incomplete_metadata(reason: &str, has_tool_calls: bool) -> ResponsesTerminalMetadata {
    let normalized = reason.trim().to_ascii_lowercase();
    if normalized.contains("max_output") || normalized.contains("length") {
        return ResponsesTerminalMetadata::max_output_tokens();
    }
    if normalized.contains("content_filter") {
        return ResponsesTerminalMetadata::error(reason.trim(), None);
    }
    if normalized.contains("tool") || has_tool_calls {
        return ResponsesTerminalMetadata::tool_calls();
    }
    ResponsesTerminalMetadata::error(reason.trim(), None)
}

pub(super) fn infer_terminal_metadata(
    status_hint: Option<&str>,
    response: Option<&Value>,
    top_level_error: Option<&Value>,
    top_level_message: Option<&str>,
    has_tool_calls: bool,
) -> ResponsesTerminalMetadata {
    if let Some((message, code)) = extract_error_details(
        top_level_error.or_else(|| response.and_then(|resp| resp.get("error"))),
        top_level_message,
    ) {
        return ResponsesTerminalMetadata::error(message, code);
    }

    let status = response
        .and_then(|resp| resp.get("status"))
        .and_then(Value::as_str)
        .or(status_hint);
    let incomplete_reason = response
        .and_then(|resp| resp.get("incomplete_details"))
        .and_then(|details| details.get("reason"))
        .and_then(Value::as_str);

    if matches!(status, Some("failed")) {
        return ResponsesTerminalMetadata::error(
            top_level_message.unwrap_or("request failed"),
            None,
        );
    }

    if let Some(reason) = incomplete_reason {
        return incomplete_metadata(reason, has_tool_calls);
    }

    if has_tool_calls {
        return ResponsesTerminalMetadata::tool_calls();
    }

    match status {
        Some("completed" | "done") => ResponsesTerminalMetadata::stop(),
        Some("incomplete") => ResponsesTerminalMetadata::error("incomplete", None),
        Some(other) if !other.is_empty() => ResponsesTerminalMetadata::error(other, None),
        _ => ResponsesTerminalMetadata::default(),
    }
}

/// 把内部 [`ChatMessage`] 序列翻译为 Responses 的 `(instructions, input items)`。
///
/// 规则（与 plan §5 Phase B 表 + pi_agent_rust 同名实现一致）：
/// - 序列首条 `role=System` 文本 → 顶层 `instructions`，**不**进 input；
/// - 后续 `role=System` → 退化到 `input` 中的 `message` 项（Responses 通常允许，但少数 Codex
///   端点会拒绝；本期不做特殊处理）；
/// - `User` → `{ type: "message", role: "user", content: [input_text] }`；
/// - `Assistant` 纯文本 → `{ type: "message", role: "assistant", content: [output_text] }`；
/// - `Assistant` 带 `tool_calls` → 文本部分单独发一条 message item，每个 tool_call 翻成
///   `{ type: "function_call", call_id, name, arguments }`；
/// - `Tool` → `{ type: "function_call_output", call_id: tool_call_id, output: text }`。
pub(super) fn build_responses_input(messages: &[ChatMessage]) -> (Option<String>, Vec<Value>) {
    let mut instructions: Option<String> = None;
    let mut input: Vec<Value> = Vec::with_capacity(messages.len());
    let mut first_seen = false;

    for msg in messages {
        // System / Assistant / Tool 角色出现非 text part 时 warn 一次并丢弃非文本部分
        // （仅 User 角色透传多模态 part；见 §3.3 角色规则）。
        if !matches!(msg.role, ChatMessageRole::User) {
            if let Some(ChatMessageContent::Parts(parts)) = &msg.content {
                warn_drop_non_text_parts(msg.role.clone(), parts);
            }
        }
        match msg.role {
            ChatMessageRole::System => {
                let text = extract_text(&msg.content).unwrap_or_default();
                if !first_seen && instructions.is_none() {
                    instructions = Some(text);
                    first_seen = true;
                    continue;
                }
                first_seen = true;
                input.push(json!({
                    "type": "message",
                    "role": "system",
                    "content": [{"type": "input_text", "text": text}],
                }));
            }
            ChatMessageRole::User => {
                first_seen = true;
                let parts = user_content_parts(&msg.content);
                input.push(json!({
                    "type": "message",
                    "role": "user",
                    "content": parts,
                }));
            }
            ChatMessageRole::Assistant => {
                first_seen = true;
                let text = extract_text(&msg.content).unwrap_or_default();
                let tool_calls = msg.tool_calls.as_deref().unwrap_or(&[]);
                if tool_calls.is_empty() {
                    if !text.is_empty() {
                        input.push(json!({
                            "type": "message",
                            "role": "assistant",
                            "content": [{"type": "output_text", "text": text}],
                        }));
                    }
                } else {
                    if !text.is_empty() {
                        input.push(json!({
                            "type": "message",
                            "role": "assistant",
                            "content": [{"type": "output_text", "text": text}],
                        }));
                    }
                    for tc in tool_calls {
                        let id = tc
                            .get("id")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        let func = tc.get("function").cloned().unwrap_or(Value::Null);
                        let name = func
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        let args = func
                            .get("arguments")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        input.push(json!({
                            "type": "function_call",
                            "call_id": id,
                            "name": name,
                            "arguments": args,
                        }));
                    }
                }
            }
            ChatMessageRole::Tool => {
                first_seen = true;
                let call_id = msg.tool_call_id.clone().unwrap_or_default();
                let output = extract_text(&msg.content).unwrap_or_default();
                input.push(json!({
                    "type": "function_call_output",
                    "call_id": call_id,
                    "output": output,
                }));
            }
        }
    }

    (instructions, input)
}

/// Chat Completions 的 function tool（`{"type":"function","function":{name,description,parameters}}`）
/// → Responses 顶层 `{"type":"function","name":..,"description":..,"parameters":..}`。
/// 输入若不是 function 类型则原样保留（向前兼容用户/插件已声明的 Responses 形状）。
pub(super) fn convert_tools_to_responses(tools: &[Value]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            let kind = t.get("type").and_then(Value::as_str);
            if kind != Some("function") {
                return t.clone();
            }
            let func = match t.get("function") {
                Some(f) => f,
                None => return t.clone(),
            };
            let mut out = json!({"type": "function"});
            if let Some(name) = func.get("name").and_then(Value::as_str) {
                out["name"] = Value::String(name.to_string());
            }
            if let Some(desc) = func.get("description").and_then(Value::as_str) {
                if !desc.trim().is_empty() {
                    out["description"] = Value::String(desc.to_string());
                }
            }
            if let Some(params) = func.get("parameters") {
                out["parameters"] = params.clone();
            } else {
                out["parameters"] = json!({"type": "object"});
            }
            out
        })
        .collect()
}

/// 从 message content 抽出**纯文本**视图：仅累加 `InputText`，其它变体跳过。
///
/// 用于 token 估算与 system / assistant / tool 角色的文本字段构造；这些角色出现
/// 非文本 part 时不会进入 wire（见 `build_responses_input` 的 warn 路径）。
fn extract_text(content: &Option<ChatMessageContent>) -> Option<String> {
    match content {
        Some(ChatMessageContent::Text(s)) => Some(s.clone()),
        Some(ChatMessageContent::Parts(parts)) => {
            let s: String = parts
                .iter()
                .filter_map(|p| p.as_text().map(str::to_string))
                .collect::<Vec<_>>()
                .join("");
            if s.is_empty() {
                None
            } else {
                Some(s)
            }
        }
        None => None,
    }
}

/// 把单个 [`ChatMessageContentPart`] 翻译成 Responses 协议的 `content[i]` JSON。
///
/// `file_id` 通道优先（已上传），否则走 inline `data:` URL；两个通道都缺时退化为
/// 形状最小的占位（`{type: input_image}` / `{type: input_file}`，由上游 API 报错）。
fn part_to_responses_value(p: &ChatMessageContentPart) -> Value {
    match p {
        ChatMessageContentPart::InputText { text } => {
            json!({"type": "input_text", "text": text})
        }
        ChatMessageContentPart::InputImage {
            mime_type,
            data,
            file_id,
            detail,
        } => {
            let mut v = json!({"type": "input_image"});
            if let Some(id) = file_id {
                v["file_id"] = Value::String(id.clone());
            } else if let (Some(mt), Some(b64)) = (mime_type, data) {
                v["image_url"] = Value::String(format!("data:{};base64,{}", mt, b64));
            }
            if let Some(d) = detail {
                v["detail"] = Value::String(d.clone());
            }
            v
        }
        ChatMessageContentPart::InputFile {
            filename,
            mime_type,
            data,
            file_id,
        } => {
            let mut v = json!({"type": "input_file"});
            if let Some(name) = filename {
                v["filename"] = Value::String(name.clone());
            }
            if let Some(id) = file_id {
                v["file_id"] = Value::String(id.clone());
            } else if let (Some(b64), Some(mt)) = (data, mime_type) {
                v["file_data"] = Value::String(format!("data:{};base64,{}", mt, b64));
            }
            v
        }
    }
}

/// 仅 `user` 角色调用：把 content 翻译为 Responses 的 `content` 数组（input_text /
/// input_image / input_file）。空 parts 兜底成单个空 input_text。
fn user_content_parts(content: &Option<ChatMessageContent>) -> Vec<Value> {
    match content {
        Some(ChatMessageContent::Text(s)) => {
            vec![json!({"type": "input_text", "text": s})]
        }
        Some(ChatMessageContent::Parts(parts)) => {
            let mut out: Vec<Value> = parts.iter().map(part_to_responses_value).collect();
            if out.is_empty() {
                out.push(json!({"type": "input_text", "text": ""}));
            }
            out
        }
        None => vec![json!({"type": "input_text", "text": ""})],
    }
}

/// system / assistant / tool 角色出现非文本 part 时调用一次：warn 并丢弃非文本部分。
///
/// 设计取舍：这些角色在 Responses 协议里 wire 形态主要承载文本与 function_call，
/// 强行透传图片/文件会触发 API 4xx；warn-and-drop 可保留 wire 兼容、避免主链路中断。
fn warn_drop_non_text_parts(role: ChatMessageRole, parts: &[ChatMessageContentPart]) {
    let non_text = parts.iter().filter(|p| p.is_non_text()).count();
    if non_text > 0 {
        warn!(
            role = ?role,
            non_text_parts = non_text,
            "role={:?} 非 user 角色出现非 text part {} 个，wire 仅取文本部分；如需多模态请置于 user 消息",
            role, non_text
        );
    }
}

/// 把 Responses `POST /v1/responses` 的非流式 JSON 翻译为内部 [`ChatResponse`]，
/// 与 Completions choices[0] 形状对齐（`message.content` + `finish_reason` + `usage`）。
pub(super) fn responses_payload_to_chat_response(raw: &Value) -> ChatResponse {
    let id = raw.get("id").and_then(Value::as_str).map(str::to_string);

    // 拼合 output[].content[].text 中所有 output_text 片段，作为 assistant 的可见内容。
    let mut text_buf = String::new();
    let mut tool_calls: Vec<Value> = Vec::new();
    if let Some(items) = raw.get("output").and_then(Value::as_array) {
        for item in items {
            let kind = item.get("type").and_then(Value::as_str);
            match kind {
                Some("message") => {
                    if let Some(parts) = item.get("content").and_then(Value::as_array) {
                        for part in parts {
                            if part.get("type").and_then(Value::as_str) == Some("output_text") {
                                if let Some(t) = part.get("text").and_then(Value::as_str) {
                                    text_buf.push_str(t);
                                }
                            }
                        }
                    }
                }
                Some("function_call") => {
                    let call_id = item
                        .get("call_id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let name = item
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let args = item
                        .get("arguments")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    tool_calls.push(json!({
                        "id": call_id,
                        "type": "function",
                        "function": {
                            "name": name,
                            "arguments": args,
                        }
                    }));
                }
                _ => {}
            }
        }
    }

    let terminal = infer_terminal_metadata(None, Some(raw), None, None, !tool_calls.is_empty());

    let usage = raw
        .get("usage")
        .map(|u| crate::core::llm::types::TokenUsage {
            prompt_tokens: u.get("input_tokens").and_then(Value::as_u64).unwrap_or(0) as u32,
            completion_tokens: u.get("output_tokens").and_then(Value::as_u64).unwrap_or(0) as u32,
            total_tokens: u
                .get("total_tokens")
                .and_then(Value::as_u64)
                .map(|v| v as u32),
        });

    let message = if tool_calls.is_empty() {
        ChatMessage::assistant(text_buf)
    } else if text_buf.is_empty() {
        ChatMessage::assistant_with_tool_calls(None, tool_calls)
    } else {
        ChatMessage::assistant_with_tool_calls(Some(&text_buf), tool_calls)
    }
    .with_completion_metadata(
        terminal.finish_reason.clone(),
        terminal.error_message.clone(),
        terminal.error_code.clone(),
    );

    ChatResponse {
        id,
        choices: vec![ChatResponseChoice {
            index: 0,
            message,
            finish_reason: terminal.finish_reason,
        }],
        usage,
    }
}
