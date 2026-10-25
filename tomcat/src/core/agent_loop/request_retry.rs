//! Bounded retries for one logical model response, never for its subsequent tools.
//!
//! Each call owns its budget. Compression, attachment fallback and empty replies
//! consume that same budget; all exits converge before reporting retry completion.
//!
//! Owns request preparation, stream consumption and terminal-response checks as
//! well as retries. Accepted/partial message publication and tool execution stay
//! in the reasoning loop, outside this request's retry budget.

use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{info, warn};

use super::error_classifier::{handle_overflow_retry, handle_unsupported_multimodal_retry};
use super::reasoning_loop::{assemble_request_messages, cache_key_for, validate_request_shape};
use super::types::{unix_ts_ms, AgentLoop, LoopError, StreamOutcome};
use super::{current_tail_guard, stream_handler};
use crate::core::llm::{ChatMessage, ChatRequest, MessageKind};
use crate::infra::config::DEFAULT_AGENT_MAX_ATTEMPTS;
use crate::infra::error::{
    is_retryable_llm_error, is_unsupported_multimodal_text, llm_empty_response_error, llm_http_status,
    llm_retry_after_ms, llm_stage, llm_summary, AppError,
};
use crate::infra::events::{wire, AgentEvent};

const INTERACTIVE_RETRY_DELAY_CAP_MS: u64 = 8_000;
const UNATTENDED_RETRY_DELAY_CAP_MS: u64 = 30_000;
const UNATTENDED_TRANSIENT_MAX_ATTEMPTS: u32 = 10;

fn retry_attempt_budget(unattended: bool, interactive_max_attempts: u32) -> u32 {
    if unattended {
        UNATTENDED_TRANSIENT_MAX_ATTEMPTS
    } else {
        interactive_max_attempts
    }
}

#[cfg(test)]
pub(super) fn compute_retry_delay_ms(base_delay_ms: u64, attempt: u32, jitter_seed: u64) -> u64 {
    compute_retry_delay_with_cap_ms(
        base_delay_ms,
        attempt,
        jitter_seed,
        INTERACTIVE_RETRY_DELAY_CAP_MS,
    )
}

fn compute_retry_delay_with_cap_ms(
    base_delay_ms: u64,
    attempt: u32,
    jitter_seed: u64,
    cap_ms: u64,
) -> u64 {
    if attempt <= 1 || base_delay_ms == 0 {
        return 0;
    }
    let exp = base_delay_ms.saturating_mul(2u64.saturating_pow(attempt.saturating_sub(2)));
    let jitter_pct = 80 + (jitter_seed % 41);
    (exp.saturating_mul(jitter_pct) / 100).min(cap_ms)
}

fn next_retry_delay_ms(base_delay_ms: u64, attempt: u32, cap_ms: u64) -> u64 {
    let jitter_seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::from(d.subsec_nanos()))
        .unwrap_or(20);
    compute_retry_delay_with_cap_ms(base_delay_ms, attempt, jitter_seed, cap_ms)
}

pub(super) async fn run_request(
    agent: &mut AgentLoop,
    messages: &mut Vec<ChatMessage>,
    turn_index: usize,
) -> Result<StreamOutcome, LoopError> {
    let unattended = agent.config.unattended_retry
        || agent
            .config
            .plan_runtime
            .as_ref()
            .and_then(|runtime| runtime.executing_plan_id())
            .is_some();
    let max_attempts = retry_attempt_budget(unattended, agent.config.max_attempts);
    let delay_cap = if unattended {
        UNATTENDED_RETRY_DELAY_CAP_MS
    } else {
        INTERACTIVE_RETRY_DELAY_CAP_MS
    };
    let mut last_err: Option<AppError> = None;
    let mut unsupported_multimodal_hits = 0u32;
    let mut attempt = 1;
    let mut retry_started = false;
    let result = loop {
        if attempt > max_attempts {
            break Err(LoopError::Fatal(AppError::Llm(crate::infra::i18n::tr(
                "llm.retriesExhausted",
                &[],
            ))));
        }
        if attempt > 1 {
            let delay_ms = last_err
                .as_ref()
                .and_then(llm_retry_after_ms)
                .unwrap_or_else(|| {
                    next_retry_delay_ms(agent.config.retry_base_delay_ms, attempt, delay_cap)
                });
            let error_message = last_err
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_else(|| crate::infra::i18n::tr("agentLoop.retry", &[]));
            retry_started = true;
            agent.emit_event(AgentEvent::AutoRetryStart {
                attempt,
                max_attempts,
                delay_ms,
                error_message: error_message.clone(),
            });
            if let Err(error) = agent.persist_custom_entry_if_needed(serde_json::json!({
                "event": wire::WIRE_AUTO_RETRY_START, "attempt": attempt,
                "max_attempts": max_attempts, "delay_ms": delay_ms, "error_message": error_message,
            })) {
                warn!(%error, "auto retry start transcript write failed");
            }
            let cancel = agent.cancel_token.clone();
            tokio::select! {
                biased;
                _ = cancel.cancelled() => break Err(agent.make_aborted(messages, String::new())),
                _ = tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)) => {}
            }
        }
        // Stream consumption and compaction retain their commit-aware cancellation.
        // The request owns no assistant/tool publication; those happen after this returns.
        match run_request_once(agent, messages, turn_index, attempt, max_attempts).await {
            Ok(outcome) => break Ok(outcome),
            Err(error @ (LoopError::Aborted { .. } | LoopError::Fatal(_))) => break Err(error),
            Err(LoopError::Retryable(error)) => {
                if unattended
                    && !is_retryable_llm_error(&error)
                    && attempt >= DEFAULT_AGENT_MAX_ATTEMPTS
                {
                    break Err(LoopError::Fatal(error));
                }
                let overflow = crate::infra::error::is_context_overflow(&error);
                let stats = handle_overflow_retry(agent, messages, attempt, &error).await;
                if agent.cancel_token.is_cancelled() {
                    break Err(agent.make_aborted(messages, String::new()));
                }
                if overflow && !stats.applied {
                    warn!(target: "tomcat_chat_diag", phase = "context_overflow_no_progress", attempt,
                        "context overflow recovery did not shrink the payload; stop retrying");
                    break Err(LoopError::Fatal(error));
                }
                let unsupported = matches!(&error, AppError::LlmDetailed(_))
                    && llm_stage(&error).is_none()
                    && llm_http_status(&error).is_none()
                    && llm_summary(&error)
                        .as_deref()
                        .is_some_and(is_unsupported_multimodal_text);
                if unsupported {
                    unsupported_multimodal_hits = unsupported_multimodal_hits.saturating_add(1);
                    if unsupported_multimodal_hits >= 2 && attempt < max_attempts {
                        handle_unsupported_multimodal_retry(agent, messages, attempt, &error);
                    }
                } else {
                    unsupported_multimodal_hits = 0;
                }
                if attempt == max_attempts {
                    break Err(LoopError::Fatal(error));
                }
                last_err = Some(error);
                attempt += 1;
            }
        }
    };
    if retry_started {
        let (success, final_error) = match &result {
            Ok(outcome) if !outcome.aborted => (true, None),
            Ok(_) | Err(LoopError::Aborted { .. }) => (false, Some("interrupted".to_string())),
            Err(LoopError::Fatal(error) | LoopError::Retryable(error)) => {
                (false, Some(error.to_string()))
            }
        };
        agent.emit_event(AgentEvent::AutoRetryEnd {
            success,
            attempt,
            final_error: final_error.clone(),
        });
        if let Err(error) = agent.persist_custom_entry_if_needed(serde_json::json!({
            "event": wire::WIRE_AUTO_RETRY_END, "success": success,
            "attempt": attempt, "final_error": final_error,
        })) {
            warn!(%error, "auto retry end transcript write failed");
        }
    }
    result
}

fn is_normal_stop_finish_reason(reason: Option<&str>) -> bool {
    matches!(reason.map(str::trim), Some("stop" | "end_turn"))
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

pub(super) async fn run_request_once(
    agent: &mut AgentLoop,
    messages: &mut Vec<ChatMessage>,
    turn_index: usize,
    attempt: u32,
    max_attempts: u32,
) -> Result<super::types::StreamOutcome, LoopError> {
    if agent.cancel_token.is_cancelled() {
        return Err(agent.make_aborted(messages, String::new()));
    }
    // Join compaction publication before observing cancellation.
    let reduced = current_tail_guard::maybe_reduce_before_next_llm(agent, messages).await;
    if agent.cancel_token.is_cancelled() {
        return Err(agent.make_aborted(messages, String::new()));
    }
    reduced.map_err(LoopError::Fatal)?;
    validate_request_shape(messages)?;

    if let Some(ref mut ctx_state) = agent.context_state {
        ctx_state.live.finish_reason = None;
        ctx_state.live.error_message = None;
        ctx_state.live.error_code = None;
    }
    if attempt == 1 {
        agent.emit_event(AgentEvent::TurnStart {
            turn_index,
            timestamp: unix_ts_ms(),
        });
    }

    let request_messages = assemble_request_messages(messages, agent);
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
        speed: agent.config.speed,
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
    if turn_index == 1 && attempt == 1 {
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
        ref content_buf,
        ref tool_calls_buf,
        ref finish_reason,
        ref error_message,
        ref error_code,
        ref thinking_text,
        ref reasoning_continuation,
        ref usage,
        aborted,
        ..
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

    // Partial-message publication belongs to the reasoning loop, outside retries.
    if aborted {
        return Ok(outcome);
    }

    let has_malformed_tool_call = tool_calls_buf
        .iter()
        .any(|tool_call| tool_call.name.trim().is_empty());
    let has_tool_calls = tool_calls_buf.iter().any(|tc| !tc.name.trim().is_empty());

    // “只思考、不回答”不是成功回合。不能只看 thinking_text：provider 可加密或
    // 因 display 配置省略它。终态、reasoning continuation 和 provider 明确报告的
    // reasoning token 都是事实信号；provider 适配层只翻译上游终态，不依据 usage
    // 猜测或改写 finish reason。`completion_tokens` 仅表示输出配额消耗，合法结构化
    // 收尾也会消耗它，不能作为失败判据。
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
    let has_hidden_output = reasoning_continuation.is_some()
        || usage
            .as_ref()
            .and_then(|usage| usage.reasoning_tokens)
            .is_some_and(|tokens| tokens > 0);
    let response_id = reasoning_continuation
        .as_ref()
        .and_then(|continuation| continuation.provider_refs.as_ref())
        .and_then(|refs| refs.openai_response_id.as_deref());
    let clean_stop = !has_tool_calls
        && !has_malformed_tool_call
        && error_code.is_none()
        && error_message.is_none()
        && is_normal_stop_finish_reason(finish_reason.as_deref());
    let silent_reply = clean_stop && has_no_visible_output;
    let silent_reply_accepted = silent_reply
        && agent.silent_reply_allowed
        && (agent.last_reply_silent || attempt >= max_attempts);
    agent.last_reply_silent = silent_reply;
    let empty_turn_failure = !silent_reply_accepted
        && !has_tool_calls
        && (has_malformed_tool_call
            || thinking_only_or_truncated
            || (has_no_visible_output && (output_truncated || has_hidden_output)));
    if empty_turn_failure {
        let thinking_chars = thinking_text.as_deref().map(str::len).unwrap_or_default();
        let failure_kind = if output_truncated {
            "output_truncated"
        } else if has_hidden_output {
            "hidden_output"
        } else if has_malformed_tool_call {
            "malformed_tool_call"
        } else {
            "thinking_only"
        };
        let _ = agent.persist_custom_entry_if_needed(serde_json::json!({
            "event": "empty_turn",
            "turn_index": turn_index,
            "finish_reason": finish_reason.as_deref(),
            "thinking_chars": thinking_chars,
            "thinking_text": thinking_text.as_deref(),
            "has_reasoning_continuation": reasoning_continuation.is_some(),
            "completion_tokens": usage.as_ref().map(|usage| usage.completion_tokens),
            "reasoning_tokens": usage.as_ref().and_then(|usage| usage.reasoning_tokens),
            "text_tokens": usage.as_ref().and_then(|usage| usage.text_tokens),
            "failure_kind": failure_kind,
            "attempt": attempt,
            "response_id": response_id,
        }));
        agent.clear_pending_assistant_entry_id();
        let message = match failure_kind {
            "output_truncated" => crate::infra::i18n::tr("agentLoop.outputTruncated", &[]),
            "hidden_output" => {
                if attempt >= max_attempts {
                    crate::infra::i18n::tr(
                        "agentLoop.hiddenExhausted",
                        &[("count", &attempt.saturating_sub(1).to_string())],
                    )
                } else {
                    crate::infra::i18n::tr("agentLoop.hiddenRetry", &[])
                }
            }
            "malformed_tool_call" => {
                if attempt >= max_attempts {
                    crate::infra::i18n::tr(
                        "agentLoop.malformedExhausted",
                        &[("count", &attempt.saturating_sub(1).to_string())],
                    )
                } else {
                    crate::infra::i18n::tr("agentLoop.malformedRetry", &[])
                }
            }
            _ => {
                if attempt >= max_attempts {
                    crate::infra::i18n::tr(
                        "agentLoop.thinkingExhausted",
                        &[("count", &attempt.saturating_sub(1).to_string())],
                    )
                } else {
                    crate::infra::i18n::tr("agentLoop.thinkingRetry", &[])
                }
            }
        };
        if failure_kind == "output_truncated" {
            return Err(LoopError::Fatal(crate::infra::error::AppError::Llm(
                message,
            )));
        }
        return Err(LoopError::Retryable(llm_empty_response_error(
            agent.llm.provider_name(),
            message,
        )));
    }

    if clean_stop && !has_no_visible_output {
        agent.silent_reply_allowed = true;
    }

    Ok(outcome)
}

#[cfg(test)]
#[path = "tests/request_retry_test.rs"]
mod tests;
