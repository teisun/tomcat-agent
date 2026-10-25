//! # Agent Loop 第二层：Reasoning Loop
//!
//! 调度模型与工具回合，跨单请求重试保留已接受正文和工具轮次。
//!
//! ```text
//! request_retry::run_request → 可接受回复 → 提交正文 / 调度工具 → 下一请求
//! ```
//!
//! 请求前 guard、TurnStart、首轮 metrics、流消费与响应判定由 request_retry 负责。
//! 本层处理流中断后的 partial 落盘；text-only 收束委托 turn_finalize，
//! 按结果结束或继续；工具执行委托 tool_dispatcher，再发布携带工具结果的 TurnEnd。
//! Steering 直接进入下一轮；其他情况先检查工具轮次上限，再消费 FollowUp。
//! 工具执行和正文提交始终在请求重试之外，避免重放已完成工具。
//!
//! ## 函数边界
//!
//! Conversation 在 run.rs，单请求的尝试与有限重试在 request_retry.rs。
//! 本文件还提供多个调用方共用的请求组装、缓存 key 和消息结构校验助手。
//! 使用 pub(super) 自由函数，与同级 stream/tool/finalize 模块保持一致。

use tracing::warn;

use crate::core::llm::{ChatMessage, ChatMessageRole, MessageKind, PromptCacheKeyFamily};
use crate::infra::events::{AgentEvent, Message};

use super::steering_injection::inject_follow_up_messages;
use super::types::{AgentLoop, LoopError, ToolCallInfo};
use super::{tool_dispatcher, turn_finalize, turn_summary};

/// Creates the provider request without changing the persisted conversation.
///
/// `system_prompt` belongs to the stable request envelope, persisted messages
/// stay in their chronological order, and the mutable ephemeral instruction is
/// always last. Keeping these three domains separate lets compaction operate on
/// only durable messages.
pub(crate) fn assemble_request_messages(
    messages: &[ChatMessage],
    agent: &AgentLoop,
) -> Vec<ChatMessage> {
    let system_prompt = agent
        .config
        .system_prompt
        .as_deref()
        .filter(|prompt| !prompt.trim().is_empty());
    let ephemeral_tail = agent
        .config
        .ephemeral_tail_provider
        .as_ref()
        .map(|provider| provider.render_ephemeral_tail())
        .filter(|tail| !tail.trim().is_empty());

    let mut request_messages = Vec::with_capacity(
        messages.len()
            + usize::from(system_prompt.is_some())
            + usize::from(ephemeral_tail.is_some()),
    );
    if let Some(system_prompt) = system_prompt {
        request_messages.push(ChatMessage::system(system_prompt));
    }
    request_messages.extend_from_slice(messages);
    if let Some(ephemeral_tail) = ephemeral_tail {
        let mut tail = ChatMessage::user(ephemeral_tail);
        tail.kind = MessageKind::EphemeralTail;
        request_messages.push(tail);
    }
    request_messages
}

pub(crate) fn cache_key_for(agent: &AgentLoop) -> Option<String> {
    let family = match agent.config.subagent_type {
        super::types::SubagentType::User => PromptCacheKeyFamily::Main,
        kind => PromptCacheKeyFamily::Subagent(kind.as_str()),
    };
    family.key_for(&agent.config.session_id)
}

/// Removes leading tool results whose declaration is absent from the request history.
///
/// Context recovery can intentionally start from a recent anchor. A tool result
/// without its earlier assistant function call is invalid provider input and
/// cannot be reconstructed safely, so a leading orphan is discarded before
/// request assembly. Older compacted history may intentionally retain tool
/// rows without their original declaration, so only the recovered-prefix shape
/// is repaired here.
pub(super) fn remove_orphan_tool_results(messages: &mut Vec<ChatMessage>) -> Vec<String> {
    let mut orphaned_call_ids = Vec::new();
    let mut at_recovered_prefix = true;
    messages.retain(|message| {
        if message.role != ChatMessageRole::Tool {
            at_recovered_prefix = false;
            return true;
        }
        if !at_recovered_prefix {
            return true;
        }
        let Some(call_id) = message.tool_call_id.as_deref() else {
            orphaned_call_ids.push("<missing tool_call_id>".to_string());
            return false;
        };
        orphaned_call_ids.push(call_id.to_string());
        false
    });
    orphaned_call_ids
}

/// Validates the durable message sequence immediately before sending it.
///
/// Orphaned tool results can safely be discarded because their declaration is
/// outside the recovered context. A dangling assistant tool call or invalid
/// tail cannot be repaired here, so the request is rejected explicitly.
pub(super) fn validate_request_shape(messages: &mut Vec<ChatMessage>) -> Result<(), LoopError> {
    let orphaned_call_ids = remove_orphan_tool_results(messages);
    if !orphaned_call_ids.is_empty() {
        warn!(
            orphaned_call_ids = ?orphaned_call_ids,
            "discarded tool results whose function calls are absent from the request history"
        );
    }
    if crate::core::session::has_dangling_tool_calls_in_messages(messages) {
        return Err(LoopError::Fatal(crate::infra::error::AppError::invariant(
            "llm_request",
            crate::infra::i18n::tr("agentLoop.unpairedTools", &[]),
        )));
    }
    if !matches!(
        messages.last().map(|message| &message.role),
        Some(ChatMessageRole::User | ChatMessageRole::Tool)
    ) {
        return Err(LoopError::Fatal(crate::infra::error::AppError::invariant(
            "llm_request",
            crate::infra::i18n::tr("agentLoop.invalidTail", &[]),
        )));
    }
    Ok(())
}

/// Model/tool rounds keep their accepted text and tool budget across request retries.
pub(super) async fn run_reasoning_loop(
    agent: &mut AgentLoop,
    messages: &mut Vec<ChatMessage>,
) -> Result<String, LoopError> {
    agent.reasoning_turn_budget_exhausted = false;
    let mut final_text = String::new();
    let mut turn_index = 0usize;
    loop {
        if agent.cancel_token.is_cancelled() {
            return Err(agent.make_aborted(messages, final_text));
        }
        turn_index += 1;
        let outcome = match super::request_retry::run_request(agent, messages, turn_index).await {
            Ok(outcome) => outcome,
            Err(LoopError::Aborted {
                partial_text,
                partial_messages,
            }) => {
                final_text.push_str(&partial_text);
                return Err(LoopError::Aborted {
                    partial_text: final_text,
                    partial_messages,
                });
            }
            Err(error) => return Err(error),
        };
        let super::types::StreamOutcome {
            content_buf,
            tool_calls_buf,
            finish_reason,
            error_message,
            error_code,
            thinking_text,
            reasoning_continuation,
            continuity,
            usage,
            aborted,
        } = outcome;
        if aborted {
            if let Some(ref mut ctx_state) = agent.context_state {
                ctx_state.live.finish_reason = None;
                ctx_state.live.error_message = None;
                ctx_state.live.error_code = None;
            }
            if !content_buf.is_empty() {
                if let Some(ref mut ctx_state) = agent.context_state {
                    // Cancelled streams may have no Usage; account for the committed partial once.
                    ctx_state.on_message_appended(content_buf.len());
                }
                let forced_id = agent.take_or_mint_pending_assistant_entry_id();
                agent
                    .push_message_with_forced_id(
                        messages,
                        ChatMessage::assistant(&content_buf),
                        &forced_id,
                    )
                    .map_err(LoopError::Fatal)?;
                final_text.push_str(&content_buf);
            } else {
                agent.clear_pending_assistant_entry_id();
            }
            return Err(agent.make_aborted(messages, final_text));
        }
        final_text.push_str(&content_buf);
        let tool_calls: Vec<ToolCallInfo> = tool_calls_buf
            .into_iter()
            .filter(|tc| !tc.name.trim().is_empty())
            .map(|tc| ToolCallInfo {
                id: tc.id,
                name: tc.name,
                arguments: tc.arguments,
            })
            .collect();

        if tool_calls.is_empty() {
            // 收束分支：text-only 回合的 timing ⑤ 与 TurnEnd 由 turn_finalize 处理。
            // completion guard 命中时回合并未结束，继续下一轮而不是返回。
            let outcome = turn_finalize::finalize_turn_after_text_with_usage(
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
                usage.clone(),
            )
            .await
            .map_err(LoopError::Fatal)?;
            match outcome {
                turn_finalize::TurnOutcome::Finished => return Ok(final_text),
                turn_finalize::TurnOutcome::Continue => continue,
            }
        }

        // tool_calls 调度（block / steering / cancel / 事件配对 / 计费 / push）
        // 整块委托给 tool_dispatcher::run_tool_calls；函数内部严格保持原事件顺序：
        // ToolExecutionStart → ExtensionEvent::ToolCall → execute_tool →
        // ExtensionEvent::ToolResult → ToolExecutionEnd；cancel 抢占点均保留
        // "先发 End 让 UI 配对再 make_aborted" 的原语义。
        let dispatch = tool_dispatcher::run_tool_calls_with_usage(
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
            usage,
        )
        .await?;

        let summary_title = turn_summary::resolve_turn_summary_title(&tool_calls);
        let tool_call_ids: Vec<String> = tool_calls.iter().map(|tc| tc.id.clone()).collect();
        turn_summary::maybe_spawn_turn_summary_update(
            agent,
            dispatch.assistant_message_id.as_deref(),
            turn_index,
            thinking_text.clone(),
            &tool_calls,
            summary_title.as_deref(),
        );

        // Do not synchronously compact mid-tool-loop. The final-reply path
        // handles L1 preheat and a ready L2 boundary; L0 follows only a
        // successful boundary application.
        agent.emit_event(AgentEvent::TurnEnd {
            turn_index,
            message: Message(serde_json::json!({})),
            tool_results: dispatch.tool_results,
            assistant_message_id: dispatch.assistant_message_id,
            tool_call_ids,
            summary_title,
        });

        if dispatch.steered {
            continue;
        }

        if turn_index >= agent.config.max_tool_rounds {
            agent.reasoning_turn_budget_exhausted = true;
            agent.emit_context_metrics(false);
            return Ok(final_text);
        }

        inject_follow_up_messages(agent, messages).map_err(LoopError::Fatal)?;
    }
}
