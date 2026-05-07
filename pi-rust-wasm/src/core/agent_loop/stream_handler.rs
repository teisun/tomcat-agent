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

use std::sync::Arc;

use tokio_stream::StreamExt;
use tracing::info;

use crate::core::llm::{ChatRequest, StreamEvent};
use crate::infra::events::{AgentEvent, AssistantMessageEvent, Message};

use super::error_classifier::classify_error;
use super::types::{AgentLoop, LoopError, StreamOutcome, ToolCallAccumulator};

/// 调用 LLM 流式接口并消费 delta，直到 `FinishReason` / 流末尾 / `Err` / cancel 之一。
///
/// ## 事件时序保证
///
/// - 建连**之前**不发 `Message*`；若 `cancel_token` 在建连 await 阶段触发，
///   返回 `Ok(StreamOutcome { aborted: true, content_buf: "", tool_calls_buf: [] })`，
///   **不发** `MessageStart` / `MessageEnd`（因 UI 从未看到消息开始）。
/// - 建连成功后发 `MessageStart`；以下 3 条 return 路径前**必发** `MessageEnd`：
///   1. 正常收敛（`FinishReason` 或 stream 返回 `None`）
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
                    aborted: true,
                });
            }
            conn = connect => match conn {
                Ok(s) => s,
                Err(e) => {
                    let snippet: String = e.to_string().chars().take(200).collect();
                    info!(
                        target: "pi_wasm_chat_diag",
                        phase = "reasoning_chat_stream_connect_err",
                        snippet = %snippet
                    );
                    return Err(classify_error(&e));
                }
            }
        }
    };

    let mut content_buf = String::new();
    let mut tool_calls_buf: Vec<ToolCallAccumulator> = Vec::new();
    let mut aborted_during_stream = false;

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
                agent.emit_event(AgentEvent::MessageUpdate {
                    message: Message(serde_json::json!({})),
                    assistant_message_event: AssistantMessageEvent(
                        serde_json::json!({ "delta": delta }),
                    ),
                });
            }
            // P1 阶段：Thinking 事件先静默落地以保证 match 穷尽；
            // P3（T2-P0-006 phase1-p3）会替换为带 `kind=thinking_delta` 的 MessageUpdate。
            Ok(StreamEvent::Thinking { .. }) => {}
            Ok(StreamEvent::ToolCallDelta {
                index,
                id,
                name,
                arguments_delta,
            }) => {
                while tool_calls_buf.len() <= index as usize {
                    tool_calls_buf.push(ToolCallAccumulator::default());
                }
                let acc = &mut tool_calls_buf[index as usize];
                if let Some(id_val) = id {
                    acc.id = id_val;
                }
                if let Some(name_val) = name {
                    acc.name = name_val;
                }
                if let Some(args) = arguments_delta {
                    acc.arguments.push_str(&args);
                }
            }
            Ok(StreamEvent::FinishReason { .. }) => break,
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
                return Err(classify_error(&e));
            }
        }
    }

    agent.emit_event(AgentEvent::MessageEnd {
        message: Message(serde_json::json!({})),
    });

    Ok(StreamOutcome {
        content_buf,
        tool_calls_buf,
        aborted: aborted_during_stream,
    })
}
