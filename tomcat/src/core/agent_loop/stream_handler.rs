//! # Agent Loop Stream 消费子模块
//!
//! 职责单一：消费 [`LlmProvider::chat_stream`] 返回的 delta 流，把
//! `ContentDelta` 拼成 `content_buf`、把 `ToolCallDelta` 按 `index` 对齐累积到
//! `Vec<ToolCallAccumulator>`。期间发射 `MessageStart` → `MessageUpdate*` →
//! `MessageEnd` 事件对，并在流末尾 / Err / cancel 三条路径上**均先发 `MessageEnd`
//! 再返回**，保证 UI 配对不丢。
//!
//! 不负责：
//! - partial assistant 落到 `messages`（由调用方 `run_reasoning_loop` Step 5 处理）
//! - 构造 `LoopError::Aborted`（同上，`make_aborted` 需要 `messages` 所有权）
//!
//! 历史：原嵌在 `run.rs:478-575` 的 100 行流消费代码整块搬入本文件，职责
//! 内聚后 T2-P0-003（`stream_timeout_sec`）/ T2-P0-006（Thinking delta 透出）
//! 只需在此函数内加 `tokio::time::timeout` / `StreamEvent::Reasoning` 分支。

use std::sync::{Arc, OnceLock};

use regex::Regex;
use tokio_stream::StreamExt;
use tracing::info;

use crate::core::llm::{ChatRequest, StreamEvent};
use crate::infra::error::{llm_source_chain, llm_stage, llm_summary};
use crate::infra::events::{AgentEvent, AssistantMessageEvent, Message};

use super::error_classifier::classify_error;
use super::types::{AgentLoop, LoopError, StreamOutcome, ToolCallAccumulator};

/// 调用 LLM 流式接口并消费 delta，直到 `FinishReason` / 流末尾 / `Err` / cancel 之一。
///
/// ## 事件时序保证
///
/// - 建连**之前**不发 `Message*`；若 `cancel_token` 在建连 await 阶段触发，
///   返回 `Ok(StreamOutcome { aborted: true, content_buf: "", tool_calls_buf: [], finish_reason: None })`，
///   **不发** `MessageStart` / `MessageEnd`（因 UI 从未看到消息开始）。
/// - 建连成功后发 `MessageStart`；以下 3 条 return 路径前**必发** `MessageEnd`：
///   1. 正常收敛（记录 `FinishReason` 后继续消费 trailing `Usage`，或 stream 返回 `None`）
///   2. Stream item `Err(AppError)` → `Err(classify_error(...))`
///   3. Cancel 触发（`aborted_during_stream = true` break 后发 `MessageEnd`）
/// - `MessageUpdate` 仅在 `StreamEvent::ContentDelta` 分支发射，`delta` 字段等于
///   当次 delta 原文。
///
/// ## 借用 / async 边界
///
/// - 函数入口先 `let cancel = agent.cancel_token.clone();` / `Arc::clone(&agent.llm)`，
///   解除对 `agent` 的长期借用，保证 `tokio::select!` 分支内可自由借 `&mut agent`。
/// - `stream` 为函数内本地变量，与 `agent.context_state` 借用**分段隔离**：
///   每次 `stream.next()` await 结束后 `item` 已是 owned value，后续 match
///   分支访问 `&mut agent.context_state` 不与流借用冲突。
/// - 无 `block_on`、无嵌套 `spawn_blocking`；所有 await 点都被 `tokio::select!`
///   包裹，`biased;` 让 cancel 优先被轮询。
pub(crate) fn extract_path_from_partial_args(args: &str) -> Option<String> {
    static PATH_RE: OnceLock<Regex> = OnceLock::new();
    let re = PATH_RE.get_or_init(|| {
        Regex::new(r#""path"\s*:\s*"((?:\\.|[^"\\])*)""#)
            .expect("path preview regex should compile")
    });
    let captures = re.captures(args)?;
    let raw = captures.get(1)?.as_str();
    serde_json::from_str::<String>(&format!("\"{}\"", raw)).ok()
}

pub(super) async fn run_chat_stream(
    agent: &mut AgentLoop,
    req: ChatRequest,
) -> Result<StreamOutcome, LoopError> {
    // 提前 clone 跨 await 持有的 Arc / token，解除 &mut agent 借用。
    let cancel = agent.cancel_token.clone();
    let llm = Arc::clone(&agent.llm);

    // ── LLM connect：chat_stream 建连 await 可被取消 ──
    let mut stream = {
        let connect = llm.chat_stream(req);
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                // 建连阶段被取消：UI 尚未收到 MessageStart，不发 MessageEnd。
                return Ok(StreamOutcome {
                    content_buf: String::new(),
                    tool_calls_buf: Vec::new(),
                    finish_reason: None,
                    error_message: None,
                    error_code: None,
                    thinking_text: None,
                    reasoning_continuation: None,
                    continuity: None,
                    aborted: true,
                });
            }
            conn = connect => match conn {
                Ok(s) => s,
                Err(e) => {
                    let snippet: String = e.to_string().chars().take(200).collect();
                    let summary = llm_summary(&e).unwrap_or_else(|| snippet.clone());
                    let source_chain = llm_source_chain(&e).join(" <- ");
                    info!(
                        target: "tomcat_chat_diag",
                        phase = "reasoning_chat_stream_connect_err",
                        stage = ?llm_stage(&e),
                        summary = %summary,
                        source_chain = %source_chain,
                        snippet = %snippet
                    );
                    return Err(classify_error(e));
                }
            }
        }
    };

    let mut content_buf = String::new();
    let mut tool_calls_buf: Vec<ToolCallAccumulator> = Vec::new();
    let mut finish_reason: Option<String> = None;
    let mut error_message: Option<String> = None;
    let mut error_code: Option<String> = None;
    let mut thinking_text: Option<String> = None;
    let mut reasoning_continuation: Option<crate::core::llm::ReasoningContinuation> = None;
    let mut continuity: Option<crate::core::llm::ContinuityMetadata> = None;
    let mut pending_notice: Option<(String, String)> = None;
    let mut aborted_during_stream = false;
    let mut streaming_announced: Vec<bool> = Vec::new();

    agent.emit_event(AgentEvent::MessageStart {
        message: Message(serde_json::json!({})),
    });

    loop {
        let next = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                aborted_during_stream = true;
                break;
            }
            item = stream.next() => item,
        };
        let Some(item) = next else {
            break;
        };
        match item {
            Ok(StreamEvent::ContentDelta { delta }) => {
                content_buf.push_str(&delta);
                // 兼容老订阅者：`delta` 字段保留为正文增量；
                // 新增 `kind=content_delta`，让单订阅者（CLI/TUI）能与 thinking_delta 分流。
                agent.emit_event(AgentEvent::MessageUpdate {
                    message: Message(serde_json::json!({})),
                    assistant_message_event: AssistantMessageEvent(serde_json::json!({
                        "kind": "content_delta",
                        "delta": delta,
                    })),
                });
            }
            // P3：Thinking 透传——单独走 thinking_delta 通道，不写 content_buf，
            // 不进入 transcript 主体；上层（CLI/TUI/扩展）按 thinking display/source 决定如何渲染。
            Ok(StreamEvent::Thinking {
                delta,
                source,
                signature,
            }) => {
                let mut payload = serde_json::json!({
                    "kind": "thinking_delta",
                    "delta": delta,
                    "source": match source {
                        crate::core::llm::ThinkingSource::Summary => "summary",
                        crate::core::llm::ThinkingSource::Raw => "raw",
                    },
                });
                if let Some(sig) = signature {
                    payload["signature"] = serde_json::Value::String(sig);
                }
                agent.emit_event(AgentEvent::MessageUpdate {
                    message: Message(serde_json::json!({})),
                    assistant_message_event: AssistantMessageEvent(payload),
                });
            }
            Ok(StreamEvent::ToolCallDelta {
                index,
                id,
                name,
                arguments_delta,
            }) => {
                let idx = index as usize;
                while tool_calls_buf.len() <= idx {
                    tool_calls_buf.push(ToolCallAccumulator::default());
                    streaming_announced.push(false);
                }
                let mut streaming_event: Option<(String, String, serde_json::Value)> = None;
                {
                    let acc = &mut tool_calls_buf[idx];
                    if let Some(id_val) = id {
                        acc.id = id_val;
                    }
                    if let Some(name_val) = name {
                        acc.name = name_val;
                    }
                    if let Some(args) = arguments_delta {
                        acc.arguments.push_str(&args);
                    }
                    if !streaming_announced[idx]
                        && !acc.id.is_empty()
                        && matches!(acc.name.as_str(), "write" | "edit" | "hashline_edit")
                    {
                        let args_preview = extract_path_from_partial_args(&acc.arguments)
                            .map(|path| serde_json::json!({ "path": path }))
                            .unwrap_or(serde_json::Value::Null);
                        streaming_event = Some((acc.id.clone(), acc.name.clone(), args_preview));
                        streaming_announced[idx] = true;
                    }
                }
                if let Some((tool_call_id, tool_name, args_preview)) = streaming_event {
                    agent.emit_event(AgentEvent::ToolCallStreaming {
                        tool_call_id,
                        tool_name,
                        args_preview,
                    });
                }
            }
            Ok(StreamEvent::FinishReason { reason }) => {
                // Responses 流里 `FinishReason` 可能早于 trailing `Usage` 到达；
                // 这里只记录终局语义，不提前 break，继续把流尾账目吃完。
                finish_reason = Some(reason);
            }
            Ok(StreamEvent::LlmError {
                reason,
                message,
                code,
            }) => {
                if finish_reason.is_none() {
                    finish_reason = Some(reason.clone());
                }
                error_message = Some(message.clone());
                error_code = code.clone();
                agent.emit_event(AgentEvent::LlmError {
                    reason,
                    error_code: code,
                    error_message: message,
                });
            }
            Ok(StreamEvent::LlmNotice {
                finish_reason: notice_reason,
                message,
            }) => {
                if finish_reason.is_none() {
                    finish_reason = Some(notice_reason.clone());
                }
                pending_notice = Some((notice_reason, message));
            }
            Ok(StreamEvent::ReasoningSnapshot {
                thinking_text: snapshot_thinking_text,
                reasoning_continuation: snapshot_reasoning_continuation,
                continuity: snapshot_continuity,
            }) => {
                if snapshot_thinking_text.is_some() {
                    thinking_text = snapshot_thinking_text;
                }
                if snapshot_reasoning_continuation.is_some() {
                    reasoning_continuation = snapshot_reasoning_continuation;
                }
                if snapshot_continuity.is_some() {
                    continuity = snapshot_continuity;
                }
            }
            Ok(StreamEvent::Usage {
                prompt_tokens,
                completion_tokens,
                ..
            }) => {
                if let Some(ref mut ctx_state) = agent.context_state {
                    ctx_state.update_api_usage(prompt_tokens, completion_tokens);
                }
            }
            Err(e) => {
                // Err 分支先发 MessageEnd 再返回，保证 UI 配对。
                agent.emit_event(AgentEvent::MessageEnd {
                    message: Message(serde_json::json!({})),
                });
                let summary = llm_summary(&e).unwrap_or_else(|| e.to_string());
                let source_chain = llm_source_chain(&e).join(" <- ");
                info!(
                    target: "tomcat_chat_diag",
                    phase = "reasoning_chat_stream_item_err",
                    provider = agent.llm.provider_name(),
                    stage = ?llm_stage(&e),
                    summary = %summary,
                    source_chain = %source_chain,
                );
                return Err(classify_error(e));
            }
        }
    }

    agent.emit_event(AgentEvent::MessageEnd {
        message: Message(serde_json::json!({})),
    });
    if let Some((notice_reason, message)) = pending_notice {
        agent.emit_event(AgentEvent::LlmNotice {
            finish_reason: notice_reason,
            message,
        });
    }

    Ok(StreamOutcome {
        content_buf,
        tool_calls_buf,
        finish_reason,
        error_message,
        error_code,
        thinking_text,
        reasoning_continuation,
        continuity,
        aborted: aborted_during_stream,
    })
}
