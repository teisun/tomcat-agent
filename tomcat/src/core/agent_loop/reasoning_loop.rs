//! # Agent Loop 第三层：Reasoning Loop
//!
//! 单 turn 内的 LLM 流式 + 工具执行 + Steering/Abort 检查的调度骨架。
//! 已把"具体动作"全部委托给同级子模块，本文件只关心**何时调用谁**：
//!
//! 1. **取消预检**：`cancel_token.is_cancelled()` → `make_aborted`（早返回）
//! 2. **TurnStart 发射**（带 unix_ts_ms 时间戳）
//! 3. **首轮 metrics**：`turn_index == 1` 时 `emit_context_metrics()` + 诊断 info!
//! 4. **Stream 消费**：`stream_handler::run_chat_stream`（同步发 Message{Start,Update,End}）
//! 5. **Stream 中断善后**：把 partial `content_buf` 落到 messages，再 `make_aborted`
//! 6. **text-only 收束**：`tool_calls.is_empty()` → `turn_finalize::finalize_turn_after_text` →
//!    立即 `Ok(final_text)`（不再发 TurnEnd，因为 finalize 内已发）
//! 7. **tool_calls 调度**：`tool_dispatcher::run_tool_calls`（统一 push assistant +
//!    block 检查 + 事件配对 + cancel 抢占）
//! 8. **TurnEnd 发射**（携带 dispatch.tool_results）
//! 9. **Steering**：`dispatch.steered == true` 立即 `continue`，跳过 max_tool_rounds 检查
//! 10. **轮次上限**：`turn_index >= max_tool_rounds` 时 `emit_context_metrics` + `Ok`
//!
//! ## 为什么是自由函数而非 `impl AgentLoop`
//!
//! Phase 3 抽 `run.rs` 瘦身时，`run.rs` 已含 Conversation/Attempt 两层共 ~200 行。
//! 把第三层留在 `run.rs` 会撞 [RUST_FILE_LINES_SPEC §A](../../../openspec/specs/guides/coding/RUST_FILE_LINES_SPEC.md)
//! 的 300 行红线。抽为本文件的 `pub(super)` 自由函数，签名 `&mut AgentLoop`，
//! 与 `stream_handler` / `tool_dispatcher` / `turn_finalize` 协议一致。

use tracing::info;

use crate::core::llm::{ChatMessage, ChatRequest};
use crate::infra::events::{AgentEvent, Message};

use super::types::{unix_ts_ms, AgentLoop, LoopError, ToolCallInfo};
use super::{current_tail_guard, stream_handler, tool_dispatcher, turn_finalize};

pub(super) async fn run_reasoning_loop(
    agent: &mut AgentLoop,
    messages: &mut Vec<ChatMessage>,
) -> Result<String, LoopError> {
    let mut final_text = String::new();
    let mut turn_index: usize = 0;

    loop {
        if agent.cancel_token.is_cancelled() {
            return Err(agent.make_aborted(messages, final_text));
        }

        if let Some(ref mut ctx_state) = agent.context_state {
            ctx_state.live.finish_reason = None;
            ctx_state.live.error_message = None;
            ctx_state.live.error_code = None;
        }

        turn_index += 1;
        agent.emit_event(AgentEvent::TurnStart {
            session_id: agent.config.session_id.clone(),
            turn_index,
            timestamp: unix_ts_ms(),
        });

        let req = ChatRequest {
            messages: messages.clone(),
            model: agent.config.model.clone(),
            temperature: None,
            max_tokens: None,
            stream: Some(true),
            model_override: None,
            tools: Some(agent.config.tool_definitions.clone()),
        };

        // context_metrics_update：单次 run_reasoning_loop 内仅在首次 LLM 请求前发一次（中间 tool round 不发）。
        if turn_index == 1 {
            agent.emit_context_metrics();
            if let Some(ref ctx_state) = agent.context_state {
                info!(
                    target: "tomcat_chat_diag",
                    phase = "emit_context_metrics_turn1",
                    turn_index,
                    input_tokens_used = ctx_state.live.input_tokens_used,
                    context_utilization_ratio = ctx_state.live.context_utilization_ratio,
                    compaction_count = ctx_state.session_obs.compaction_count
                );
            }
        }

        // Stream 消费（含 LLM connect + MessageStart/Update/End 发射 + cancel 抢占）
        // 整块委托给 stream_handler::run_chat_stream；aborted / Err 路径均已
        // 先发 MessageEnd，调用方仅需补 partial assistant 落盘与 make_aborted。
        let outcome = stream_handler::run_chat_stream(agent, req).await?;
        let super::types::StreamOutcome {
            content_buf,
            tool_calls_buf,
            finish_reason,
            error_message,
            error_code,
            thinking_text,
            reasoning_continuation,
            continuity,
            aborted,
        } = outcome;

        if let Some(ref mut ctx_state) = agent.context_state {
            ctx_state.live.finish_reason = finish_reason.clone();
            ctx_state.live.error_message = error_message.clone();
            ctx_state.live.error_code = error_code.clone();
        }

        // stream 被取消：把 partial content_buf 作为 partial assistant 落到 messages，
        // 让 ctx_state 也把它计入消息预算；再返回 Aborted 携带 partial。
        if aborted {
            if let Some(ref mut ctx_state) = agent.context_state {
                ctx_state.live.finish_reason = None;
                ctx_state.live.error_message = None;
                ctx_state.live.error_code = None;
            }
            if !content_buf.is_empty() {
                if let Some(ref mut ctx_state) = agent.context_state {
                    ctx_state.on_message_appended(content_buf.len());
                }
                messages.push(ChatMessage::assistant(&content_buf));
                final_text.push_str(&content_buf);
            }
            return Err(agent.make_aborted(messages, final_text));
        }

        final_text.push_str(&content_buf);

        let tool_calls: Vec<ToolCallInfo> = tool_calls_buf
            .into_iter()
            .filter(|tc| !tc.name.is_empty())
            .map(|tc| ToolCallInfo {
                id: tc.id,
                name: tc.name,
                arguments: tc.arguments,
            })
            .collect();

        if tool_calls.is_empty() {
            // 收束分支：text-only 回合的 timing ⑤ 与 TurnEnd 由 turn_finalize 处理。
            turn_finalize::finalize_turn_after_text(
                agent,
                messages,
                &content_buf,
                turn_index,
                finish_reason.clone(),
                error_message.clone(),
                error_code.clone(),
                thinking_text.clone(),
                reasoning_continuation.clone(),
                continuity.clone(),
            )
            .map_err(LoopError::Fatal)?;
            return Ok(final_text);
        }

        // tool_calls 调度（block / steering / cancel / 事件配对 / 计费 / push）
        // 整块委托给 tool_dispatcher::run_tool_calls；函数内部严格保持原事件顺序：
        // ToolExecutionStart → ExtensionEvent::ToolCall → execute_tool →
        // ExtensionEvent::ToolResult → ToolExecutionEnd；cancel 抢占点均保留
        // "先发 End 让 UI 配对再 make_aborted" 的原语义。
        let dispatch = tool_dispatcher::run_tool_calls(
            agent,
            messages,
            &tool_calls,
            &content_buf,
            &final_text,
            finish_reason.clone(),
            error_message.clone(),
            error_code.clone(),
            thinking_text.clone(),
            reasoning_continuation.clone(),
            continuity.clone(),
        )
        .await?;

        // No synchronous cascade here; L0/L1/L2 handled at timing ⑤
        agent.emit_event(AgentEvent::TurnEnd {
            session_id: agent.config.session_id.clone(),
            turn_index,
            message: Message(serde_json::json!({})),
            tool_results: dispatch.tool_results,
        });

        if dispatch.steered {
            current_tail_guard::maybe_reduce_before_next_llm(agent, messages)
                .await
                .map_err(LoopError::Fatal)?;
            continue;
        }

        if turn_index >= agent.config.max_tool_rounds {
            agent.emit_context_metrics();
            return Ok(final_text);
        }

        current_tail_guard::maybe_reduce_before_next_llm(agent, messages)
            .await
            .map_err(LoopError::Fatal)?;
    }
}
