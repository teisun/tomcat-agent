//! # Agent Loop 第三层：Reasoning Loop
//!
//! 单 turn 内的 LLM 流式 + 工具执行 + Steering/Abort 检查的调度骨架。
//! 已把"具体动作"全部委托给同级子模块，本文件只关心**何时调用谁**：
//!
//! 1. **取消预检**：`cancel_token.is_cancelled()` → `make_aborted`（早返回）
//! 2. **请求前 guard**：每次请求前 `maybe_reduce_before_next_llm`，必要时减负或折叠
//! 3. **TurnStart 发射**（带 unix_ts_ms 时间戳）
//! 4. **首轮 metrics**：`turn_index == 1` 时 `emit_context_metrics()` + 诊断 info!
//! 5. **Stream 消费**：`stream_handler::run_chat_stream`（同步发 Message{Start,Update,End}）
//! 6. **Stream 中断善后**：把 partial `content_buf` 落到 messages，再 `make_aborted`
//! 7. **text-only 收束**：`tool_calls.is_empty()` → `turn_finalize::finalize_turn_after_text` →
//!    立即 `Ok(final_text)`（不再发 TurnEnd，因为 finalize 内已发）
//! 8. **tool_calls 调度**：`tool_dispatcher::run_tool_calls`（统一 push assistant +
//!    block 检查 + 事件配对 + cancel 抢占）
//! 9. **TurnEnd 发射**（携带 dispatch.tool_results）
//! 10. **Steering**：`dispatch.steered == true` 立即 `continue`，跳过 follow-up / max_tool_rounds
//! 11. **FollowUp**：非 steered 且后续仍要继续请求时，先 drain `follow_up_queue`
//! 12. **轮次上限**：`turn_index >= max_tool_rounds` 时 `emit_context_metrics` + `Ok`
//!
//! ## 为什么是自由函数而非 `impl AgentLoop`
//!
//! Phase 3 抽 `run.rs` 瘦身时，`run.rs` 已含 Conversation/Attempt 两层共 ~200 行。
//! 把第三层留在 `run.rs` 会撞 [RUST_FILE_LINES_SPEC §A](../../../docs/openspec/specs/guides/coding/RUST_FILE_LINES_SPEC.md)
//! 的 300 行红线。抽为本文件的 `pub(super)` 自由函数，签名 `&mut AgentLoop`，
//! 与 `stream_handler` / `tool_dispatcher` / `turn_finalize` 协议一致。

use tracing::info;

use crate::core::llm::{
    ChatMessage, ChatMessageRole, ChatRequest, MessageKind, PromptCacheKeyFamily,
};
use crate::infra::events::{AgentEvent, Message};

use super::steering_injection::inject_follow_up_messages;
use super::types::{unix_ts_ms, AgentLoop, LoopError, ToolCallInfo};
use super::{current_tail_guard, stream_handler, tool_dispatcher, turn_finalize, turn_summary};

pub(crate) fn with_ephemeral_tail(messages: &[ChatMessage], agent: &AgentLoop) -> Vec<ChatMessage> {
    let mut request_messages = messages.to_vec();
    let Some(provider) = agent.config.ephemeral_tail_provider.as_ref() else {
        return request_messages;
    };
    let tail = provider.render_ephemeral_tail();
    if !tail.trim().is_empty() {
        let mut tail = ChatMessage::user(tail);
        tail.kind = MessageKind::EphemeralTail;
        request_messages.push(tail);
        return request_messages;
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

fn is_output_truncation_finish_reason(reason: Option<&str>) -> bool {
    matches!(
        reason.map(str::trim),
        Some("max_tokens")
            | Some("max_output_tokens")
            | Some("max_completion_tokens")
            | Some("length")
            | Some("output_length")
    )
}

fn prompt_prefix_fingerprint_enabled() -> bool {
    std::env::var("TOMCAT_PROMPT_PREFIX_FINGERPRINT")
        .ok()
        .is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
}

pub(super) async fn run_reasoning_loop(
    agent: &mut AgentLoop,
    messages: &mut Vec<ChatMessage>,
    attempt: u32,
    max_attempts: u32,
) -> Result<String, LoopError> {
    agent.reasoning_turn_budget_exhausted = false;
    let mut final_text = String::new();
    let mut turn_index: usize = 0;

    loop {
        if agent.cancel_token.is_cancelled() {
            return Err(agent.make_aborted(messages, final_text));
        }
        current_tail_guard::maybe_reduce_before_next_llm(agent, messages)
            .await
            .map_err(LoopError::Fatal)?;
        if crate::core::session::has_dangling_tool_calls_in_messages(messages) {
            return Err(LoopError::Fatal(crate::infra::error::AppError::invariant(
                "llm_request",
                "refusing to send a transcript with unpaired tool calls; hydrate or resolve the pending tool result first",
            )));
        }
        if !matches!(
            messages.last().map(|message| &message.role),
            Some(ChatMessageRole::User | ChatMessageRole::Tool)
        ) {
            return Err(LoopError::Fatal(crate::infra::error::AppError::invariant(
                "llm_request",
                "refusing to send a transcript whose tail is not a user input or completed tool result",
            )));
        }

        if let Some(ref mut ctx_state) = agent.context_state {
            ctx_state.live.finish_reason = None;
            ctx_state.live.error_message = None;
            ctx_state.live.error_code = None;
        }

        turn_index += 1;
        agent.emit_event(AgentEvent::TurnStart {
            turn_index,
            timestamp: unix_ts_ms(),
        });

        let request_messages = with_ephemeral_tail(messages, agent);
        let ephemeral_tail = request_messages
            .iter()
            .rev()
            .find(|message| matches!(message.kind, MessageKind::EphemeralTail))
            .and_then(ChatMessage::text_content);
        if let Some(ctx_state) = agent.context_state.as_mut() {
            ctx_state.session_obs.observe_ephemeral_tail(ephemeral_tail);
        }
        let mut req = ChatRequest {
            messages: request_messages,
            model: agent.wire_model().to_string(),
            temperature: None,
            max_tokens: None,
            resolved_output_limit: None,
            diagnostic_request_id: prompt_prefix_fingerprint_enabled()
                .then(|| format!("{}:{}", agent.config.session_id, turn_index)),
            stream: Some(true),
            model_override: None,
            thinking_level: agent.config.thinking_level,
            cache_key: cache_key_for(agent),
            tools: Some(agent.config.tool_definitions.clone()),
        };
        let wire_limit_source = agent.binding.apply_resolved_output_limit(&mut req);
        let request_max_tokens = req.max_tokens;
        let wire_output_limit = req.resolved_output_limit;
        let thinking_budget = agent
            .binding
            .thinking_budget_for_request(req.thinking_level, wire_output_limit);
        let diagnostic_request_id = req.diagnostic_request_id.clone();
        info!(
            target: "tomcat_chat_diag",
            phase = "llm_request_resolved",
            model = %agent.wire_model(),
            api = %agent.binding.api,
            model_limit = ?agent.binding.limits.model_max_output_tokens,
            request_limit = ?request_max_tokens,
            wire_limit = ?wire_output_limit,
            context_window = agent.binding.limits.context_window,
            input_budget = agent.binding.limits.input_budget_tokens,
            context_source = agent.binding.limits.context_source.as_str(),
            output_source = agent.binding.limits.output_source.as_str(),
            wire_limit_source = wire_limit_source.as_str(),
        );

        // 首次请求前只发一条 fallback 估算。每次 provider Usage 到达时，
        // stream_handler 会立即另发一条实测 metrics，不必等整个工具链结束。
        if turn_index == 1 {
            agent.emit_context_metrics(false);
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

        // Capture the local prediction immediately before this request. The
        // provider's Usage event below is the corresponding ground truth.
        let estimated_prompt_tokens = agent
            .context_state
            .as_ref()
            .map(|state| state.estimated_token_count());

        // Stream 消费（含 LLM connect + MessageStart/Update/End 发射 + cancel 抢占）
        // 整块委托给 stream_handler::run_chat_stream；aborted / Err 路径均已
        // 先发 MessageEnd，调用方仅需补 partial assistant 落盘与 make_aborted。
        let outcome = stream_handler::run_chat_stream(agent, req, attempt, max_attempts).await?;
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

        if let Some(ref mut ctx_state) = agent.context_state {
            ctx_state.live.finish_reason = finish_reason.clone();
            ctx_state.live.error_message = error_message.clone();
            ctx_state.live.error_code = error_code.clone();
        }
        if let Some(usage) = usage.as_ref() {
            info!(
                target: "tomcat_chat_diag",
                phase = "llm_usage",
                model = %agent.wire_model(),
                prompt_tokens = usage.prompt_tokens,
                completion_tokens = usage.completion_tokens,
                cache_read_tokens = ?usage.cache_read_tokens,
                cache_write_tokens = ?usage.cache_write_tokens,
            );
            if let Some(request_id) = diagnostic_request_id.as_deref() {
                // These names intentionally match Anthropic's response fields,
                // even though TokenUsage stores the normalized values. The
                // paired request-side fingerprint has the same request id.
                info!(
                    target: "tomcat_chat_diag",
                    phase = "prompt_prefix_result",
                    request_id,
                    cache_read_input_tokens = ?usage.cache_read_tokens,
                    cache_creation_input_tokens = ?usage.cache_write_tokens,
                );
            }
            if let Some(estimated_prompt_tokens) = estimated_prompt_tokens {
                let actual_prompt_tokens = usage.prompt_tokens as usize;
                let absolute_error_tokens = estimated_prompt_tokens.abs_diff(actual_prompt_tokens);
                let relative_error = if actual_prompt_tokens == 0 {
                    0.0
                } else {
                    absolute_error_tokens as f64 / actual_prompt_tokens as f64
                };
                info!(
                    target: "tomcat_chat_diag",
                    phase = "context_estimate_check",
                    request_id = ?diagnostic_request_id,
                    estimated_prompt_tokens,
                    actual_prompt_tokens,
                    absolute_error_tokens,
                    relative_error,
                );
            }
        }
        if is_output_truncation_finish_reason(finish_reason.as_deref()) {
            info!(
                target: "tomcat_chat_diag",
                phase = "output_truncated",
                model = %agent.wire_model(),
                request_max_tokens = ?request_max_tokens,
                wire_max_tokens = ?wire_output_limit,
                thinking_budget = ?thinking_budget,
                completion_tokens = ?usage.as_ref().map(|usage| usage.completion_tokens),
                prompt_tokens = ?usage.as_ref().map(|usage| usage.prompt_tokens),
                usage_ratio = agent.context_state.as_ref().map(|state| state.usage_ratio()),
                has_reasoning_continuation = reasoning_continuation.is_some(),
            );
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
                    // A cancelled stream may never emit Usage. In that case
                    // excluding the persisted partial response from the
                    // post-usage delta hides it from the next timing-2
                    // boundary check. Prefer this one-shot overestimate to
                    // risking a missed overflow safeguard.
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
            .filter(|tc| !tc.name.is_empty())
            .map(|tc| ToolCallInfo {
                id: tc.id,
                name: tc.name,
                arguments: tc.arguments,
            })
            .collect();

        // “只思考、不回答”不是成功回合。不能只看 thinking_text：Anthropic 可能
        // 加密它，OpenAI Responses 也可能因 display 配置省略它。截断终态与
        // reasoning continuation 是能区分隐藏推理和合法空 end_turn 的事实信号。
        // `completion_tokens` 只说明本轮消耗过输出配额；合法结构化收尾也会消耗
        // token，不能拿它当失败判据。若某个 provider 发生“隐藏推理 + stop”却
        // 不提供 reasoning_continuation，必须由该 provider 把终态适配为截断类，
        // 而不是在这里重新引入 usage 猜测。
        let thinking_only_or_truncated = thinking_text.as_deref().is_some_and(|thinking| {
            let content = content_buf.trim();
            !thinking.trim().is_empty()
                && (content.is_empty()
                    || content == thinking
                    || (thinking.starts_with(content)
                        && content.len().saturating_mul(2) < thinking.len()))
        });
        let has_no_visible_output = content_buf.trim().is_empty();
        let output_truncated = is_output_truncation_finish_reason(finish_reason.as_deref());
        let has_hidden_output = reasoning_continuation.is_some();
        let empty_turn_failure = tool_calls.is_empty()
            && (thinking_only_or_truncated
                || (has_no_visible_output && (output_truncated || has_hidden_output)));
        if empty_turn_failure {
            let thinking_chars = thinking_text.as_deref().map(str::len).unwrap_or_default();
            let failure_kind = if output_truncated {
                "output_truncated"
            } else if has_hidden_output {
                "hidden_output"
            } else {
                "thinking_only"
            };
            let _ = agent.persist_custom_entry_if_needed(serde_json::json!({
                "event": "empty_turn",
                "turn_index": turn_index,
                "finish_reason": finish_reason.as_deref(),
                "thinking_chars": thinking_chars,
                "has_reasoning_continuation": reasoning_continuation.is_some(),
                "completion_tokens": usage.as_ref().map(|usage| usage.completion_tokens),
                "failure_kind": failure_kind,
            }));
            agent.clear_pending_assistant_entry_id();
            let message = match failure_kind {
                "output_truncated" => {
                    "本轮输出在达到上限前没有产生可见回答。请使用 Resume 重试，或换一个模型后重试。"
                }
                "hidden_output" => {
                    "本轮产生了不可显示的推理、没有可见回答。请使用 Resume 重试，或换一个模型后重试。"
                }
                _ => "本轮只产生了思考、没有产生回答。请使用 Resume 重试，或换一个模型后重试。",
            };
            return Err(LoopError::Fatal(crate::infra::error::AppError::Llm(
                message.to_string(),
            )));
        }

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
