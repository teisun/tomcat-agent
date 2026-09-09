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

use crate::core::llm::replay_policy::{
    plan, ProviderCompatProfile, ReplayAction, ReplayDowngradeReport,
};
use crate::core::llm::types::{
    ephemeral_tail_texts, is_ephemeral_tail, ChatMessage, ChatMessageContent,
    ChatMessageContentPart, ChatMessageRole, ChatResponse, ChatResponseChoice, FileSource,
    ImageSource, ReasoningContinuation, ReasoningFormat,
};

pub(super) const MAX_OUTPUT_TOKENS_NOTICE: &str = "达到 max_output_tokens，回答可能未完成";
const REASONING_EXHAUSTION_MIN_OUTPUT_TOKENS: u64 = 128;
const REASONING_EXHAUSTION_PERCENT: u64 = 95;

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
        let finish_reason = code
            .as_deref()
            .filter(|code| !code.is_empty())
            .map(|code| format!("error:{code}"))
            .unwrap_or_else(|| "error".to_string());
        Self {
            finish_reason: Some(finish_reason),
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

fn response_has_nonempty_output_text(response: &Value) -> bool {
    response
        .get("output")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("message"))
        .filter_map(|item| item.get("content").and_then(Value::as_array))
        .flatten()
        .any(|part| {
            part.get("type").and_then(Value::as_str) == Some("output_text")
                && part
                    .get("text")
                    .and_then(Value::as_str)
                    .is_some_and(|text| !text.trim().is_empty())
        })
}

fn completed_empty_reasoning_likely_exhausted(response: &Value) -> bool {
    let output_tokens = response
        .get("usage")
        .and_then(|usage| usage.get("output_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let reasoning_tokens = response
        .get("usage")
        .and_then(|usage| usage.get("output_tokens_details"))
        .and_then(|details| details.get("reasoning_tokens"))
        .or_else(|| {
            response
                .get("usage")
                .and_then(|usage| usage.get("reasoning_output_tokens"))
        })
        .and_then(Value::as_u64)
        .unwrap_or_default();

    output_tokens >= REASONING_EXHAUSTION_MIN_OUTPUT_TOKENS
        && !response_has_nonempty_output_text(response)
        && reasoning_tokens.saturating_mul(100)
            >= output_tokens.saturating_mul(REASONING_EXHAUSTION_PERCENT)
}

/// 将 Responses 端点的终态统一成内部 finish reason。
///
/// 这里的“完成但空输出且几乎全是 reasoning token”是对上游 `completed` 误报的**诊断性
/// 兜底**：它改写为 `max_output_tokens`，让用户知道回复可能因输出预算耗尽而未完成。
/// 它不决定一轮对话能否成功；后者仍由 agent loop 的空回合守卫负责，且该守卫只拒绝
/// “有 thinking、无正文、无工具”的回合。因此纯工具轮与端点合法的结构化空 `end_turn`
/// 仍不会被本启发式误伤。
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

    if !has_tool_calls
        && matches!(status, Some("completed" | "done"))
        && response.is_some_and(completed_empty_reasoning_likely_exhausted)
    {
        return ResponsesTerminalMetadata::max_output_tokens();
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
/// - `EphemeralTail` → 追加到顶层 `instructions`，**不**进 input。这样它不会在下轮被新
///   assistant / tool history 插进 durable history 的中间；但它位于缓存前缀，内容变化仍会
///   改变整个 `instructions` 前缀并使 provider cache miss。缓存效果必须由 usage 指标验证，
///   不能由这个布局推断；
/// - 后续 `role=System` → 退化到 `input` 中的 `message` 项（Responses 通常允许，但少数 Codex
///   端点会拒绝；本期不做特殊处理）；
/// - `User` → `{ type: "message", role: "user", content: [input_text] }`；
/// - `Assistant` 纯文本 → `{ type: "message", role: "assistant", content: [output_text] }`；
/// - `Assistant` 带 `tool_calls` → 文本部分单独发一条 message item，每个 tool_call 翻成
///   `{ type: "function_call", call_id, name, arguments }`；
/// - `Tool` → `{ type: "function_call_output", call_id: tool_call_id, output: text }`。
pub(super) fn build_responses_input(
    messages: &[ChatMessage],
    target: &ProviderCompatProfile,
    continuity_enabled: bool,
    explicit_replay: bool,
    input_start: usize,
) -> (Option<String>, Vec<Value>) {
    let mut instructions = messages.first().and_then(|message| {
        matches!(message.role, ChatMessageRole::System)
            .then(|| extract_text(&message.content).unwrap_or_default())
    });
    let runtime_tail = ephemeral_tail_texts(messages)
        .collect::<Vec<_>>()
        .join("\n\n");
    if !runtime_tail.is_empty() {
        let mut merged = instructions.unwrap_or_default();
        if !merged.is_empty() {
            merged.push_str("\n\n");
        }
        merged.push_str(&runtime_tail);
        instructions = Some(merged);
    }
    let mut input: Vec<Value> = Vec::with_capacity(messages.len());
    let mut report = ReplayDowngradeReport::default();

    for (index, original) in messages.iter().enumerate() {
        // The runtime tail has already been promoted to top-level instructions. Keeping it
        // out of input means newly appended assistant/tool history extends the prior request
        // instead of being inserted before a disappearing user message.
        if is_ephemeral_tail(original) {
            continue;
        }
        // The leading system message is always sent as top-level instructions. When a request
        // continues from `previous_response_id`, it still must be resent, but its historical
        // message item must not be duplicated in `input`.
        if index == 0 && matches!(original.role, ChatMessageRole::System) {
            continue;
        }
        // `previous_response_id` restores all state through the selected assistant response.
        // Only items after that assistant are new input for the next response.
        if index < input_start {
            continue;
        }
        let action = if continuity_enabled {
            plan(target, original)
        } else {
            ReplayAction::StripOpaque
        };
        if continuity_enabled {
            report.record_replay_decision(target, original, &action);
        }
        let explicit_keep = matches!(action, ReplayAction::KeepOpaque);
        let msg = match action {
            ReplayAction::KeepOpaque | ReplayAction::StripOpaque => {
                original.without_completion_metadata()
            }
        };
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
                input.push(json!({
                    "type": "message",
                    "role": "system",
                    "content": [{"type": "input_text", "text": text}],
                }));
            }
            ChatMessageRole::User => {
                let parts = user_content_parts(&msg.content);
                input.push(json!({
                    "type": "message",
                    "role": "user",
                    "content": parts,
                }));
            }
            ChatMessageRole::Assistant => {
                if continuity_enabled && explicit_replay && explicit_keep {
                    if let Some(continuation) = original.reasoning_continuation.as_ref() {
                        input.extend(responses_reasoning_items(continuation));
                    }
                }
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

    if continuity_enabled {
        report.emit(target);
    }

    (instructions, input)
}

fn responses_reasoning_items(continuation: &ReasoningContinuation) -> Vec<Value> {
    if !matches!(
        continuation.format,
        ReasoningFormat::OpenaiResponsesReasoningItems
    ) {
        return Vec::new();
    }
    match &continuation.opaque_payload {
        Value::Array(items) => items
            .iter()
            .filter(|item| {
                item.get("type")
                    .and_then(Value::as_str)
                    .map(|kind| kind.contains("reasoning"))
                    .unwrap_or(false)
                    || item.get("encrypted_content").is_some()
            })
            .cloned()
            .collect(),
        Value::Object(_) => vec![continuation.opaque_payload.clone()],
        _ => Vec::new(),
    }
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
                .filter_map(|part| match part {
                    ChatMessageContentPart::InputText { text } => Some(text.clone()),
                    ChatMessageContentPart::InputReference { reference } => {
                        Some(reference.to_prompt_text())
                    }
                    ChatMessageContentPart::InputImage { .. }
                    | ChatMessageContentPart::InputImageRef { .. }
                    | ChatMessageContentPart::InputFile { .. } => None,
                })
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
/// 类型层已确保：图片/文件的 inline 与 file_id 通道二选一，不存在“半条 part”占位。
fn part_to_responses_value(p: &ChatMessageContentPart) -> Value {
    match p {
        ChatMessageContentPart::InputText { text } => {
            json!({"type": "input_text", "text": text})
        }
        ChatMessageContentPart::InputReference { reference } => {
            json!({"type": "input_text", "text": reference.to_prompt_text()})
        }
        ChatMessageContentPart::InputImage { source, detail } => {
            let mut v = json!({"type": "input_image"});
            match source {
                ImageSource::Inline(inline) => {
                    v["image_url"] =
                        Value::String(format!("data:{};base64,{}", inline.mime_type, inline.data));
                }
                ImageSource::Uploaded(uploaded) => {
                    v["file_id"] = Value::String(uploaded.file_id.clone());
                }
            }
            if let Some(d) = detail {
                v["detail"] = Value::String(d.clone());
            }
            v
        }
        ChatMessageContentPart::InputImageRef { .. } => {
            json!({"type": "input_text", "text": "[图片引用尚未物化]"})
        }
        ChatMessageContentPart::InputFile { source } => {
            let mut v = json!({"type": "input_file"});
            match source {
                FileSource::Inline(inline) => {
                    v["filename"] = Value::String(inline.filename.clone());
                    v["file_data"] =
                        Value::String(format!("data:{};base64,{}", inline.mime_type, inline.data));
                }
                FileSource::Uploaded(uploaded) => {
                    v["file_id"] = Value::String(uploaded.file_id.clone());
                }
            }
            v
        }
    }
}

/// 仅 `user` 角色调用：把文本 + 引用 flatten 成单个 `input_text`，再把附件追加为
/// `input_image` / `input_file` part。空 parts 兜底成单个空 input_text。
fn user_content_parts(content: &Option<ChatMessageContent>) -> Vec<Value> {
    match content {
        Some(ChatMessageContent::Text(s)) => {
            vec![json!({"type": "input_text", "text": s})]
        }
        Some(ChatMessageContent::Parts(parts)) => {
            let mut text = String::new();
            let mut out: Vec<Value> = Vec::with_capacity(parts.len().max(1));
            for part in parts {
                match part {
                    ChatMessageContentPart::InputText { text: chunk } => text.push_str(chunk),
                    ChatMessageContentPart::InputReference { reference } => {
                        text.push_str(&reference.to_prompt_text());
                    }
                    ChatMessageContentPart::InputImage { .. }
                    | ChatMessageContentPart::InputImageRef { .. }
                    | ChatMessageContentPart::InputFile { .. } => {
                        out.push(part_to_responses_value(part));
                    }
                }
            }
            out.insert(0, json!({"type": "input_text", "text": text}));
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
    let non_text = parts
        .iter()
        .filter(|part| {
            matches!(
                part,
                ChatMessageContentPart::InputImage { .. }
                    | ChatMessageContentPart::InputImageRef { .. }
                    | ChatMessageContentPart::InputFile { .. }
            )
        })
        .count();
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
            cache_read_tokens: u
                .get("input_tokens_details")
                .and_then(|details| details.get("cached_tokens"))
                .and_then(Value::as_u64)
                .map(|v| v as u32),
            cache_write_tokens: None,
            reasoning_tokens: u
                .get("output_tokens_details")
                .and_then(|details| details.get("reasoning_tokens"))
                .or_else(|| u.get("reasoning_output_tokens"))
                .and_then(Value::as_u64)
                .map(|v| v as u32),
            text_tokens: {
                let output = u.get("output_tokens").and_then(Value::as_u64).unwrap_or(0) as u32;
                u.get("output_tokens_details")
                    .and_then(|details| details.get("reasoning_tokens"))
                    .or_else(|| u.get("reasoning_output_tokens"))
                    .and_then(Value::as_u64)
                    .map(|reasoning| output.saturating_sub(reasoning as u32))
            },
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
