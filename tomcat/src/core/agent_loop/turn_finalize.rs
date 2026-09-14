//! # Reasoning Loop 收束分支：text-only 回合的 timing ⑤ + TurnEnd 发射
//!
//! 当 LLM 本轮**没有产出 tool_calls**（纯文本回复）时，reasoning loop 不再继续，
//! 进入"收束分支"做四步 cleanup：
//!
//! 1. `on_assistant_message_appended(content_buf.len())` + `messages.push(assistant)`
//! 2. **Timing ⑤**：preheat.try_restart_if_pending → `apply_ready_preheat`
//!    （仅 ratio ≥ 0.85；fold → apply → unfold，成功时只在 history_end 内执行 L0）
//!    → preheat.try_start（Idle → Running）
//! 3. 条件发射 `AutoCompactionStart`
//! 4. `emit_context_metrics()` + `TurnEnd { tool_results: [] }`
//!
//! 历史：原嵌在 `run.rs::run_reasoning_loop` 的 `if tool_calls.is_empty()` 分支
//! 内（约 80 行）。Phase 3 抽出为本文件的自由函数后，`run_reasoning_loop` 主体
//! 只关心"取消预检 / TurnStart / Stream 调度 / Tool Dispatch"四件事，骨架更清晰。

use std::sync::Arc;

use crate::core::llm::{
    ChatMessage, ContinuityMetadata, MessageKind, PromptCacheKeyFamily, ReasoningContinuation,
    TokenUsage,
};
use crate::core::plan_runtime::{file_store, NextAction, PlanRuntime};
use crate::infra::events::{AgentEvent, Message};

use super::{
    current_tail_guard::{apply_ready_preheat, ensure_working_message_ids},
    types::AgentLoop,
};

/// 连续注入上限。到顶后停止注入、交还用户，避免模型卡在同一个坎上无限打转。
pub(super) const MAX_COMPLETION_GUARD_INJECTIONS: u32 = 8;

/// text-only 回合的处理结果。
#[derive(Debug, PartialEq, Eq)]
pub(super) enum TurnOutcome {
    /// 回合正常结束，reasoning loop 可以返回。
    Finished,
    /// 计划还没做完，已注入继续指令，reasoning loop 应再跑一轮。
    Continue,
}

/// 计划尚未收口时，模型不能靠"回一段文字"结束回合。
///
/// 判据用计划文件的 `state` 而不是"还有没有未完成的 todo"：
/// todo 可能全勾完了但 code review 打回，此时 state 仍是 `executing`，活儿也确实没干完。
async fn completion_guard_instruction(plan_runtime: &PlanRuntime) -> Option<String> {
    let plan_id = plan_runtime.executing_plan_id()?;
    let plan = plan_runtime
        .active_plan_path()
        .and_then(|path| file_store::read_plan(&path).ok())?;

    let next_action = plan_runtime.next_action(&plan.frontmatter).await;
    if matches!(&next_action, NextAction::Done | NextAction::HandOff { .. }) {
        return None;
    }
    if plan_runtime
        .note_completion_guard_nudge(&plan_id, &next_action)
        .await
    {
        return None;
    }

    Some(format!(
        "Plan `{plan_id}`: {} Do not summarize or hand back.",
        next_action.instruction()
    ))
}

async fn should_apply_completion_guard(agent: &AgentLoop) -> Option<String> {
    if !agent.config.subagent_type.is_root() {
        return None;
    }
    let runtime = agent.config.plan_runtime.as_ref()?;
    completion_guard_instruction(runtime).await
}
/// 处理 text-only 回合的全部副作用：消息落盘、timing ⑤、收束事件发射。
///
/// **必须在 `tool_calls.is_empty()` 分支调用，且仅调用一次**——重复调用会重复
/// `on_message_appended` 计费、重复发 `TurnEnd`。
///
/// `content_buf`：本轮 delta 累积。`turn_index`：作为 `TurnEnd` 的 turn 序号。
///
/// 这是不带 usage 的测试便利包装；生产路径统一调用
/// [`finalize_turn_after_text_with_usage`]，以保留 provider usage。
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(super) async fn finalize_turn_after_text(
    agent: &mut AgentLoop,
    messages: &mut Vec<ChatMessage>,
    content_buf: &str,
    turn_index: usize,
    finish_reason: Option<String>,
    error_message: Option<String>,
    error_code: Option<String>,
    thinking_text: Option<String>,
    reasoning_continuation: Option<ReasoningContinuation>,
    continuity: Option<ContinuityMetadata>,
) -> Result<TurnOutcome, crate::infra::error::AppError> {
    finalize_turn_after_text_with_usage(
        agent,
        messages,
        content_buf,
        turn_index,
        finish_reason,
        error_message,
        error_code,
        thinking_text,
        reasoning_continuation,
        continuity,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn finalize_turn_after_text_with_usage(
    agent: &mut AgentLoop,
    messages: &mut Vec<ChatMessage>,
    content_buf: &str,
    turn_index: usize,
    finish_reason: Option<String>,
    error_message: Option<String>,
    error_code: Option<String>,
    thinking_text: Option<String>,
    reasoning_continuation: Option<ReasoningContinuation>,
    continuity: Option<ContinuityMetadata>,
    usage: Option<TokenUsage>,
) -> Result<TurnOutcome, crate::infra::error::AppError> {
    if let Some(ref mut ctx_state) = agent.context_state {
        ctx_state.on_assistant_message_appended(content_buf.len());
    }
    let forced_id = agent.take_or_mint_pending_assistant_entry_id();
    let assistant_message_id = agent.push_message_with_forced_id(
        messages,
        ChatMessage::assistant(content_buf)
            .with_completion_metadata(finish_reason, error_message, error_code)
            .with_reasoning_state(thinking_text, reasoning_continuation, continuity)
            .with_usage(usage),
        &forced_id,
    )?;

    // Completion guard：计划还没收口就想用一段文字收工时，注入继续指令并把回合续上。
    // 放在 timing ⑤ 之前 —— 这个回合根本没结束，不该走收束流程。
    if agent.completion_guard_injections < MAX_COMPLETION_GUARD_INJECTIONS {
        if let Some(instruction) = should_apply_completion_guard(agent).await {
            agent.completion_guard_injections += 1;
            let mut nudge = ChatMessage::user(&instruction);
            nudge.kind = MessageKind::Nudge;
            if let Some(ref mut ctx_state) = agent.context_state {
                ctx_state.on_message_appended(instruction.len());
            }
            agent.push_message(messages, nudge)?;
            return Ok(TurnOutcome::Continue);
        }
    }

    // Timing ⑤: try_restart → non-blocking boundary apply (including L0)
    // → try_start → metrics. Preheat snapshots the current working set because Scheme E requires
    // its covered end to be the durable transcript tail.
    let compaction_provider = agent.compaction_provider();
    let compaction_emitter = Arc::new(agent.emitter.clone());
    let control_snapshot = agent
        .config
        .plan_runtime
        .as_ref()
        .map(|rt| rt.control_snapshot(Some(agent.wire_model())));
    let compaction_cache_key = PromptCacheKeyFamily::Compaction.key_for(&agent.config.session_id);
    let mut preheat_started: Option<(usize, f64)> = None;
    if agent.context_state.is_some() {
        let first_non_system = messages
            .iter()
            .position(|message| message.role != crate::core::llm::ChatMessageRole::System)
            .unwrap_or(messages.len());

        // Step 1: restore ExhaustedPending → Running from the full working set, whose tail is
        // the transcript tail while this turn is being finalized.
        let restart_needed = agent.context_state.as_ref().is_some_and(|ctx_state| {
            ctx_state.usage_ratio() >= 0.50 && ctx_state.preheat.is_exhausted_pending()
        });
        if restart_needed {
            ensure_working_message_ids(agent, &mut messages[first_non_system..])?;
            if let Some(ctx_state) = agent.context_state.as_mut() {
                ctx_state.preheat.try_restart_if_pending(
                    ctx_state.usage_ratio(),
                    &messages[first_non_system..],
                    &ctx_state.transcript_path,
                    compaction_cache_key.clone(),
                    Arc::clone(&compaction_provider),
                    agent.config.compaction_output_limit,
                    &agent.config.context_config,
                    Arc::clone(&compaction_emitter),
                    control_snapshot.clone(),
                );
            }
        }

        // Step 2: L2 non-blocking poll + apply boundary. This is shared with the mid-turn
        // guard so the boundary is always located in a merged history-plus-tail view.
        let _boundary_applied = apply_ready_preheat(agent, messages, Some(0.85))?;

        // Step 4: Idle → Running (start new preheat if conditions met).
        let start_needed = agent.context_state.as_ref().is_some_and(|ctx_state| {
            ctx_state.usage_ratio() >= 0.50 && ctx_state.preheat.is_idle()
        });
        if start_needed {
            ensure_working_message_ids(agent, &mut messages[first_non_system..])?;
            if let Some(ctx_state) = agent.context_state.as_mut() {
                let ratio = ctx_state.usage_ratio();
                let covered_count = messages.len().saturating_sub(first_non_system);
                if ctx_state.preheat.try_start(
                    ratio,
                    &messages[first_non_system..],
                    &ctx_state.transcript_path,
                    compaction_cache_key,
                    Arc::clone(&compaction_provider),
                    agent.config.compaction_output_limit,
                    &agent.config.context_config,
                    Arc::clone(&compaction_emitter),
                    control_snapshot.clone(),
                ) {
                    preheat_started = Some((covered_count, ratio));
                }
            }
        }
    }

    if let Some((covered_count, ratio_before)) = preheat_started {
        agent.emit_event(AgentEvent::AutoCompactionStart {
            covered_count,
            ratio_before,
        });
    }

    // Timing ⑤ may have rewritten history; this is a fresh best-effort
    // estimate, not a newly returned provider Usage sample.
    agent.emit_context_metrics(false);
    agent.emit_event(AgentEvent::TurnEnd {
        turn_index,
        message: Message(serde_json::json!({})),
        tool_results: vec![],
        assistant_message_id: Some(assistant_message_id),
        tool_call_ids: vec![],
        summary_title: None,
    });
    Ok(TurnOutcome::Finished)
}
