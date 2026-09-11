use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::Utc;
use tracing::{info, warn};

use crate::core::compaction::apply::{check_after_reply, BoundaryEnv};
use crate::core::compaction::preheat::{generate_summary_with_output_limit, SummaryRequestOptions};
use crate::core::compaction::{
    compact_tool_results, is_persisted_tool_result_text, persist_tool_result_text,
    TOOL_RESULT_PLACEHOLDER,
};
use crate::core::llm::{
    ChatMessage, ChatMessageRole, LlmProvider, MessageKind, PromptCacheKeyFamily,
};
use crate::core::plan_runtime::PlanRuntime;
use crate::core::session::manager::{
    build_context_from_state, compound_turn_id, estimate_msg_chars, estimated_tokens_from_chars,
    generate_entry_id, CompactionResult, ContextState,
};
use crate::core::session::transcript::{
    append_entry, entry_id, insert_entry_after_message_id, read_entries_tail,
    rewrite_message_text_entries_by_id, BranchSummaryEntry, MessageTextRewrite, TranscriptEntry,
};
use crate::core::session::user_message_sidecar::ensure_user_message_sidecar_current;

use crate::infra::error::AppError;

use super::types::AgentLoop;

const COMPACTABLE_TOOLS: &[&str] = &["read", "search_files", "bash", "task_output"];
/// Build-mode folds the prefix captured at the 50% waterline, retaining the later half as the
/// agent's working set. This is deliberately not a user configuration knob: its value is a
/// policy trade-off, and cache/compaction observations decide whether it should become 0.25.
const MIDTURN_FOLD_KEEP_RATIO: f64 = 0.5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GuardRoute {
    Fits,
    Reduce,
    Collapse,
}

impl GuardRoute {
    fn as_str(self) -> &'static str {
        match self {
            Self::Fits => "fits",
            Self::Reduce => "reduce",
            Self::Collapse => "collapse",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GuardRouteReason {
    Fits,
    PreheatShortcut,
    ReducibleEnough,
    NotEnoughReducible,
    MissingContextState,
}

impl GuardRouteReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::Fits => "fits",
            Self::PreheatShortcut => "preheat_shortcut",
            Self::ReducibleEnough => "reducible_enough",
            Self::NotEnoughReducible => "not_enough_reducible",
            Self::MissingContextState => "missing_context_state",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AggregatePrecheckDecision {
    pub route: GuardRoute,
    pub route_reason: GuardRouteReason,
    pub working_tokens: usize,
    pub budget_tokens: usize,
    pub overflow_tokens: usize,
    pub max_reducible: usize,
    pub candidate_count: usize,
    pub yellow_lamp_only: bool,
    pub after_each_wave: Vec<usize>,
    pub after_collapse: Option<usize>,
}

struct TailCandidate {
    msg_idx: usize,
    message_id: Option<String>,
}

#[derive(Debug, Default)]
struct ReductionResult {
    mutated: bool,
    freed_chars: usize,
    after_each_wave: Vec<usize>,
}

#[derive(Debug, Default)]
struct TailReductionResult {
    freed_chars: usize,
    after_each_wave: Vec<usize>,
}

#[doc(hidden)]
#[derive(Debug, Clone)]
pub struct CollapseSummaryArtifacts {
    pub summary_text: String,
    pub summary_message: ChatMessage,
    pub transcript_entry: TranscriptEntry,
    pub covered_start_id: String,
    pub covered_end_id: String,
    pub entry_id: String,
}

pub(super) async fn maybe_reduce_before_next_llm(
    agent: &mut AgentLoop,
    messages: &mut Vec<ChatMessage>,
) -> Result<(), AppError> {
    let _ = maybe_reduce_before_next_llm_inner(agent, messages).await?;
    Ok(())
}

async fn maybe_reduce_before_next_llm_inner(
    agent: &mut AgentLoop,
    messages: &mut Vec<ChatMessage>,
) -> Result<Option<AggregatePrecheckDecision>, AppError> {
    let Some(ctx_state) = agent.context_state.as_ref() else {
        return Ok(None);
    };
    let working_tokens = ctx_state.estimated_token_count();
    let budget_tokens = ctx_state.context_budget_tokens;
    if budget_tokens == 0 {
        return Ok(None);
    }

    let mut decision = build_precheck_decision(agent, messages, working_tokens, budget_tokens);
    if decision.yellow_lamp_only {
        info!(
            target: "tomcat_chat_diag",
            phase = "mid_turn_precheck_yellow",
            ratio = ctx_state.usage_ratio(),
            working_tokens = decision.working_tokens,
            budget_tokens = decision.budget_tokens
        );
    }
    if matches!(decision.route, GuardRoute::Fits) {
        start_midturn_preheat_if_needed(agent, messages)?;
        log_aggregate_precheck_decision(&decision);
        return Ok(Some(decision));
    }

    let reduction = if matches!(decision.route, GuardRoute::Reduce) {
        reduce_before_next_llm(agent, messages)?
    } else {
        ReductionResult::default()
    };
    decision.after_each_wave = reduction.after_each_wave;

    let mut mutated = reduction.mutated;
    let still_over_budget = agent
        .context_state
        .as_ref()
        .is_some_and(ContextState::is_over_budget);
    if matches!(decision.route, GuardRoute::Collapse) || still_over_budget {
        collapse_to_branch_summary(agent, messages).await?;
        decision.after_collapse = agent
            .context_state
            .as_ref()
            .map(ContextState::estimated_token_count);
        mutated = true;
    }

    log_aggregate_precheck_decision(&decision);

    if mutated {
        // The reduction changed the request payload, so publish the new
        // best-effort estimate. It is not a fresh provider Usage sample.
        agent.emit_context_metrics(false);
    }
    Ok(Some(decision))
}

#[cfg(test)]
pub(super) async fn maybe_reduce_before_next_llm_capture_decision(
    agent: &mut AgentLoop,
    messages: &mut Vec<ChatMessage>,
) -> Result<Option<AggregatePrecheckDecision>, AppError> {
    maybe_reduce_before_next_llm_inner(agent, messages).await
}

pub(super) fn build_precheck_decision(
    agent: &AgentLoop,
    messages: &[ChatMessage],
    working_tokens: usize,
    budget_tokens: usize,
) -> AggregatePrecheckDecision {
    let yellow_lamp_only = budget_tokens > 0
        && working_tokens <= budget_tokens
        && ctx_ratio(working_tokens, budget_tokens) >= 0.90;
    let overflow_tokens = working_tokens.saturating_sub(budget_tokens);
    if working_tokens <= budget_tokens {
        return AggregatePrecheckDecision {
            route: GuardRoute::Fits,
            route_reason: GuardRouteReason::Fits,
            working_tokens,
            budget_tokens,
            overflow_tokens,
            max_reducible: 0,
            candidate_count: 0,
            yellow_lamp_only,
            after_each_wave: Vec::new(),
            after_collapse: None,
        };
    }

    let needed_tokens = overflow_tokens
        .saturating_add(256)
        .max(((overflow_tokens as f64) * 1.2).ceil() as usize);
    let Some(ctx_state) = agent.context_state.as_ref() else {
        return AggregatePrecheckDecision {
            route: GuardRoute::Collapse,
            route_reason: GuardRouteReason::MissingContextState,
            working_tokens,
            budget_tokens,
            overflow_tokens,
            max_reducible: 0,
            candidate_count: 0,
            yellow_lamp_only,
            after_each_wave: Vec::new(),
            after_collapse: None,
        };
    };
    let candidate_count = collect_tail_candidates(
        messages,
        agent.start_idx.min(messages.len()),
        agent
            .config
            .context_config
            .current_tail_compactable_min_chars,
    )
    .len();
    let max_reducible = estimate_history_reduction_tokens(ctx_state, &agent.config.context_config)
        + estimate_tail_reduction_tokens(
            messages,
            agent.start_idx.min(messages.len()),
            agent
                .config
                .context_config
                .current_tail_compactable_min_chars,
        );
    let (route, route_reason) = if ctx_state.preheat.is_finished() {
        // D3：预热收益一旦就绪，就优先尝试整条 Reduce 链路
        // （apply 历史 -> 历史再压 -> tail reduction），而不是先用纯理论
        // `max_reducible >= needed` 再做一次严格裁决。
        (GuardRoute::Reduce, GuardRouteReason::PreheatShortcut)
    } else if max_reducible >= needed_tokens {
        (GuardRoute::Reduce, GuardRouteReason::ReducibleEnough)
    } else {
        (GuardRoute::Collapse, GuardRouteReason::NotEnoughReducible)
    };
    AggregatePrecheckDecision {
        route,
        route_reason,
        working_tokens,
        budget_tokens,
        overflow_tokens,
        max_reducible,
        candidate_count,
        yellow_lamp_only,
        after_each_wave: Vec::new(),
        after_collapse: None,
    }
}

fn reduce_before_next_llm(
    agent: &mut AgentLoop,
    messages: &mut Vec<ChatMessage>,
) -> Result<ReductionResult, AppError> {
    let mut result = ReductionResult::default();
    if apply_midturn_preheat(agent, messages)? {
        result.mutated = true;
        if !context_is_over_budget(agent) {
            return Ok(result);
        }
    }
    let boundary_env = BoundaryEnv {
        config: &agent.config.context_config,
        work_dir: Path::new(&agent.config.agent_trail_dir),
        session_id: &agent.config.session_id,
        read_file_state: agent.config.read_file_state.as_ref(),
    };

    let applied_history = {
        let Some(ctx_state) = agent.context_state.as_mut() else {
            return Ok(result);
        };
        check_after_reply(ctx_state, &agent.emitter, &boundary_env)
    };
    if applied_history {
        rebuild_messages_from_context(agent, messages);
        result.mutated = true;
        if !context_is_over_budget(agent) {
            return Ok(result);
        }
    }

    let history_reduced = {
        let Some(ctx_state) = agent.context_state.as_mut() else {
            return Ok(result);
        };
        let placeholder = compact_tool_results(ctx_state, &agent.config.context_config);
        if placeholder.chars_freed > 0 {
            ctx_state.invalidate_api_usage();
        }
        for tool_call_id in &placeholder.tool_call_ids {
            agent
                .config
                .read_file_state
                .invalidate_tool_call(tool_call_id);
        }
        placeholder.chars_freed
    };
    if history_reduced > 0 {
        result.freed_chars += history_reduced;
        rebuild_messages_from_context(agent, messages);
        result.mutated = true;
        if !context_is_over_budget(agent) {
            return Ok(result);
        }
    }

    let tail_result = reduce_current_tail_messages(agent, messages)?;
    if tail_result.freed_chars > 0 {
        result.freed_chars += tail_result.freed_chars;
        result.after_each_wave = tail_result.after_each_wave;
        result.mutated = true;
    }

    if result.freed_chars > 0 {
        if let Some(ctx_state) = agent.context_state.as_mut() {
            ctx_state.session_obs.compaction_count =
                ctx_state.session_obs.compaction_count.saturating_add(1);
            ctx_state.session_obs.compaction_tokens_freed +=
                estimated_tokens_from_chars(result.freed_chars);
        }
    }

    Ok(result)
}

/// A build's tool loop has no normal turn boundary, so its preheat must start from the guard's
/// Fits path rather than `turn_finalize`. The snapshot ends at the current tool round; later
/// tool calls stay raw when this preheated prefix is applied.
fn start_midturn_preheat_if_needed(
    agent: &mut AgentLoop,
    messages: &mut [ChatMessage],
) -> Result<(), AppError> {
    let Some(ctx_state) = agent.context_state.as_ref() else {
        return Ok(());
    };
    let usage_ratio = ctx_state.usage_ratio();
    if usage_ratio < MIDTURN_FOLD_KEEP_RATIO || !ctx_state.preheat.is_idle() {
        return Ok(());
    }

    let first_non_system = messages
        .iter()
        .position(|message| message.role != ChatMessageRole::System)
        .unwrap_or(messages.len());
    if first_non_system == messages.len() {
        return Ok(());
    }
    ensure_working_message_ids(agent, &mut messages[first_non_system..])?;
    let snapshot = messages[first_non_system..].to_vec();
    let transcript_path = ctx_state.transcript_path.clone();
    let compaction_provider = agent.compaction_provider();
    let cache_key = PromptCacheKeyFamily::Compaction.key_for(&agent.config.session_id);
    let control_snapshot = agent
        .config
        .plan_runtime
        .as_ref()
        .map(|runtime| runtime.control_snapshot(Some(agent.wire_model())));
    let emitter = std::sync::Arc::new(agent.emitter.clone());
    let covered_count = snapshot.len();

    let started = agent.context_state.as_mut().is_some_and(|ctx_state| {
        ctx_state.preheat.try_start(
            usage_ratio,
            &snapshot,
            &transcript_path,
            cache_key,
            compaction_provider,
            agent.config.compaction_output_limit,
            &agent.config.context_config,
            emitter,
            control_snapshot,
        )
    });
    if started {
        agent.emit_event(crate::infra::events::AgentEvent::AutoCompactionStart {
            covered_count,
            ratio_before: usage_ratio,
        });
    }
    Ok(())
}

/// Apply a ready build-mode preheat to exactly its recorded `covered_end_id`, leaving all tool
/// calls made after the waterline in the live tail. The shared apply path writes the completed
/// body as an append-only row; a missing boundary is stale, so the regular Reduce/Collapse
/// fallbacks handle the current context.
fn apply_midturn_preheat(
    agent: &mut AgentLoop,
    messages: &mut Vec<ChatMessage>,
) -> Result<bool, AppError> {
    let Some(result) = (match agent.context_state.as_mut() {
        Some(ctx_state) if ctx_state.preheat.is_finished() => match ctx_state.preheat.poll_result()
        {
            crate::core::compaction::preheat::PreheatOutcome::Completed(result) => Some(result),
            _ => None,
        },
        _ => None,
    }) else {
        return Ok(false);
    };

    let boundary_env = BoundaryEnv {
        config: &agent.config.context_config,
        work_dir: Path::new(&agent.config.agent_trail_dir),
        session_id: &agent.config.session_id,
        read_file_state: agent.config.read_file_state.as_ref(),
    };
    // Build-mode keeps the current tool-round working set in `messages`; ContextState can still
    // lag behind it while the loop is between tool calls. Synchronize it before handing off to
    // the same boundary application primitive used by normal turn boundaries, so the covered
    // prefix is replaced while the later tool round remains in the raw tail.
    let state_start = usize::from(
        messages
            .first()
            .is_some_and(|message| message.role == ChatMessageRole::System),
    );
    let applied = agent.context_state.as_mut().is_some_and(|ctx_state| {
        ctx_state.messages = messages[state_start..].to_vec();
        ctx_state.estimate_context_chars = ctx_state.messages.iter().map(estimate_msg_chars).sum();
        ctx_state.invalidate_api_usage();
        crate::core::compaction::apply::apply_and_emit_boundary(
            ctx_state,
            result,
            ctx_state.usage_ratio(),
            false,
            &agent.emitter,
            &boundary_env,
        )
    });
    if !applied {
        return Ok(false);
    }
    // The applied prefix and the live suffix now form one persisted context. Rebuild from this
    // authoritative state, leaving the next assistant/tool pair as the new current tail.
    agent.start_idx = messages.len();
    agent.context_tail_start = messages.len();
    rebuild_messages_from_context(agent, messages);
    Ok(true)
}

fn reduce_current_tail_messages(
    agent: &mut AgentLoop,
    messages: &mut [ChatMessage],
) -> Result<TailReductionResult, AppError> {
    let Some(ctx_state) = agent.context_state.as_mut() else {
        return Ok(TailReductionResult::default());
    };
    let tail_start = agent.start_idx.min(messages.len());
    let config = &agent.config.context_config;
    let work_dir = Path::new(&agent.config.agent_trail_dir);
    // Step 0 changes a tool result into a durable file reference, so it still needs a targeted
    // rewrite. Placeholder waves only change what the current LLM request sees.
    let mut transcript_rewrites = Vec::new();
    let mut result = TailReductionResult::default();
    let mut step0_reduced = false;
    // 正文被换成引用/占位符的那些工具调用：这一轮结束前必须让它们的 read stamp 失效。
    let mut evicted_tool_call_ids: Vec<String> = Vec::new();

    let initial_candidates = collect_tail_candidates(
        messages,
        tail_start,
        config.current_tail_compactable_min_chars,
    );
    for candidate in &initial_candidates {
        let Some(content) = messages[candidate.msg_idx]
            .text_content()
            .map(str::to_string)
        else {
            continue;
        };
        if content.len() < config.current_tail_single_result_max_chars {
            continue;
        }
        let Some(tool_call_id) = messages[candidate.msg_idx].tool_call_id.clone() else {
            continue;
        };
        if let Some(text) = text_content_mut(&mut messages[candidate.msg_idx]) {
            if let Some((persisted, freed)) = persist_tool_result_text(
                text,
                &tool_call_id,
                work_dir,
                &agent.config.session_id,
                config.current_tail_single_result_max_chars,
            ) {
                result.freed_chars += freed;
                step0_reduced = true;
                evicted_tool_call_ids.push(tool_call_id.clone());
                ctx_state.rewrite_local_tail_chars(content.len(), text.len());
                ctx_state.session_obs.tool_result_chars_persisted += persisted.original_chars;
                if let Some(message_id) = &candidate.message_id {
                    transcript_rewrites.push(MessageTextRewrite {
                        message_id: message_id.clone(),
                        new_content: text.clone(),
                    });
                }
            }
        }
    }

    let candidates_after_step0 = collect_tail_candidates(
        messages,
        tail_start,
        config.current_tail_compactable_min_chars,
    );
    if candidates_after_step0.len() > 2 {
        apply_placeholder_wave(
            ctx_state,
            messages,
            candidates_after_step0,
            &mut result.freed_chars,
            &mut evicted_tool_call_ids,
        );
        result
            .after_each_wave
            .push(ctx_state.estimated_token_count());
    } else if step0_reduced {
        result
            .after_each_wave
            .push(ctx_state.estimated_token_count());
    }

    loop {
        if !ctx_state.is_over_budget() {
            break;
        }
        let candidates = collect_tail_candidates(
            messages,
            tail_start,
            config.current_tail_compactable_min_chars,
        );
        if candidates.is_empty() || candidates.len() <= 2 {
            break;
        }
        apply_placeholder_wave(
            ctx_state,
            messages,
            candidates,
            &mut result.freed_chars,
            &mut evicted_tool_call_ids,
        );
        result
            .after_each_wave
            .push(ctx_state.estimated_token_count());
    }

    rewrite_transcript_best_effort(&ctx_state.transcript_path, transcript_rewrites);
    for tool_call_id in &evicted_tool_call_ids {
        agent
            .config
            .read_file_state
            .invalidate_tool_call(tool_call_id);
    }
    Ok(result)
}

fn collect_tail_candidates(
    messages: &[ChatMessage],
    tail_start: usize,
    min_chars: usize,
) -> Vec<TailCandidate> {
    let mut tool_names = HashMap::<String, String>::new();
    for msg in messages.iter().skip(tail_start) {
        if let Some(tool_calls) = &msg.tool_calls {
            for tool_call in tool_calls {
                let id = tool_call.get("id").and_then(|v| v.as_str());
                let name = tool_call
                    .get("function")
                    .and_then(|v| v.get("name"))
                    .and_then(|v| v.as_str());
                if let (Some(id), Some(name)) = (id, name) {
                    tool_names.insert(id.to_string(), name.to_string());
                }
            }
        }
    }

    messages
        .iter()
        .enumerate()
        .skip(tail_start)
        .filter_map(|(idx, msg)| {
            if msg.role != ChatMessageRole::Tool {
                return None;
            }
            let tool_name = tool_names.get(msg.tool_call_id.as_deref().unwrap_or(""))?;
            if !COMPACTABLE_TOOLS.contains(&tool_name.as_str()) {
                return None;
            }
            let text = msg.text_content()?;
            if text.len() < min_chars
                || text == TOOL_RESULT_PLACEHOLDER
                || is_persisted_tool_result_text(text)
            {
                return None;
            }
            Some(TailCandidate {
                msg_idx: idx,
                message_id: msg.msg_id.clone(),
            })
        })
        .collect()
}

fn estimate_history_reduction_tokens(
    state: &ContextState,
    config: &crate::infra::config::ContextConfig,
) -> usize {
    let protected_start = find_protected_turn_start(&state.messages, config.keep_recent_turns);
    let reducible_chars: usize = state.messages[..protected_start]
        .iter()
        .filter(|msg| msg.role == ChatMessageRole::Tool)
        .filter_map(|msg| msg.text_content())
        .filter(|text| text.len() > config.layer0_placeholder_threshold_chars)
        .filter(|text| *text != TOOL_RESULT_PLACEHOLDER && !is_persisted_tool_result_text(text))
        .map(|text| text.len().saturating_sub(TOOL_RESULT_PLACEHOLDER.len()))
        .sum();
    estimated_tokens_from_chars(reducible_chars)
}

fn estimate_tail_reduction_tokens(
    messages: &[ChatMessage],
    tail_start: usize,
    min_chars: usize,
) -> usize {
    let reducible_chars: usize = collect_tail_candidates(messages, tail_start, min_chars)
        .into_iter()
        .filter_map(|candidate| messages[candidate.msg_idx].text_content())
        .map(|text| text.len().saturating_sub(TOOL_RESULT_PLACEHOLDER.len()))
        .sum();
    estimated_tokens_from_chars(reducible_chars)
}

fn find_protected_turn_start(messages: &[ChatMessage], keep_recent_turns: usize) -> usize {
    if keep_recent_turns == 0 {
        return messages.len();
    }
    let turn_starts: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter(|(_, msg)| msg.starts_logical_turn())
        .map(|(idx, _)| idx)
        .collect();
    if turn_starts.len() <= keep_recent_turns {
        return 0;
    }
    turn_starts[turn_starts.len() - keep_recent_turns]
}

fn rebuild_messages_from_context(agent: &mut AgentLoop, messages: &mut Vec<ChatMessage>) {
    let Some(ctx_state) = agent.context_state.as_ref() else {
        return;
    };
    let tail = messages[agent.start_idx.min(messages.len())..].to_vec();
    let mut rebuilt = Vec::new();
    if messages
        .first()
        .is_some_and(|msg| msg.role == ChatMessageRole::System)
    {
        rebuilt.push(messages[0].clone());
    }
    rebuilt.extend(build_context_from_state(ctx_state));
    let new_tail_start = rebuilt.len();
    rebuilt.extend(tail);
    *messages = rebuilt;
    agent.start_idx = new_tail_start;
    agent.context_tail_start = new_tail_start;
}

fn rewrite_transcript_best_effort(path: &Path, rewrites: Vec<MessageTextRewrite>) {
    if path.as_os_str().is_empty() || rewrites.is_empty() {
        return;
    }
    let mut latest = HashMap::<String, String>::new();
    for rewrite in rewrites {
        latest.insert(rewrite.message_id, rewrite.new_content);
    }
    let merged: Vec<MessageTextRewrite> = latest
        .into_iter()
        .map(|(message_id, new_content)| MessageTextRewrite {
            message_id,
            new_content,
        })
        .collect();
    if let Err(err) = rewrite_message_text_entries_by_id(path, &merged) {
        warn!(error = %err, "mid-turn transcript rewrite failed");
    }
}

pub(super) async fn collapse_to_branch_summary(
    agent: &mut AgentLoop,
    messages: &mut Vec<ChatMessage>,
) -> Result<(), AppError> {
    let plan_runtime = agent.config.plan_runtime.clone();
    let session_model = agent.wire_model().to_string();
    let mut working: Vec<ChatMessage> = messages
        .iter()
        .filter(|msg| msg.role != ChatMessageRole::System)
        .cloned()
        .collect();
    ensure_working_message_ids(agent, &mut working)?;
    let transcript_path = agent
        .context_state
        .as_ref()
        .map(|state| state.transcript_path.clone())
        .unwrap_or_default();

    let compaction_provider = agent.compaction_provider();
    let cache_key = PromptCacheKeyFamily::Compaction.key_for(&agent.config.session_id);
    let artifacts = build_collapse_summary_artifacts(
        &working,
        compaction_provider.as_ref(),
        &agent.config.context_config.compaction_model,
        CollapseSummaryRequest {
            plan_runtime: plan_runtime.as_deref(),
            session_model: Some(session_model.as_str()),
            cache_key: cache_key.as_deref(),
            resolved_output_limit: agent.config.compaction_output_limit,
            transcript_path: (!transcript_path.as_os_str().is_empty())
                .then_some(transcript_path.as_path()),
        },
    )
    .await?;
    let Some(ctx_state) = agent.context_state.as_mut() else {
        return Ok(());
    };
    match maybe_write_collapse_entry(
        &ctx_state.transcript_path,
        &artifacts.covered_end_id,
        &artifacts.transcript_entry,
    ) {
        Ok(()) => {
            let _ = ensure_user_message_sidecar_current(&ctx_state.transcript_path).await;
        }
        Err(err) => warn!(error = %err, "collapse branch_summary transcript write failed"),
    }

    let summary_msg = artifacts.summary_message;
    let new_chars = estimate_msg_chars(&summary_msg);
    let saved_chars = ctx_state.estimate_context_chars.saturating_sub(new_chars);
    ctx_state.messages = vec![summary_msg.clone()];
    ctx_state.estimate_context_chars = new_chars;
    ctx_state.invalidate_api_usage();
    ctx_state.preheat.abort();
    ctx_state.session_obs.compaction_count =
        ctx_state.session_obs.compaction_count.saturating_add(1);
    ctx_state.session_obs.compaction_tokens_freed += estimated_tokens_from_chars(saved_chars);

    let mut rebuilt = Vec::new();
    if messages
        .first()
        .is_some_and(|msg| msg.role == ChatMessageRole::System)
    {
        rebuilt.push(messages[0].clone());
    }
    rebuilt.push(summary_msg);
    *messages = rebuilt;
    agent.start_idx = messages.len().saturating_sub(1);
    agent.context_tail_start = agent.start_idx;
    Ok(())
}

/// 生成 collapse 摘要所需的可选运行时资料，集中传递避免 helper 位置参数继续增长。
struct CollapseSummaryRequest<'a> {
    plan_runtime: Option<&'a PlanRuntime>,
    session_model: Option<&'a str>,
    cache_key: Option<&'a str>,
    resolved_output_limit: Option<u32>,
    transcript_path: Option<&'a Path>,
}

#[doc(hidden)]
pub async fn build_collapse_summary_artifacts_for_test(
    messages: &[ChatMessage],
    llm: &dyn LlmProvider,
    compaction_model: &str,
    plan_runtime: Option<&PlanRuntime>,
    session_model: Option<&str>,
) -> Result<CollapseSummaryArtifacts, AppError> {
    build_collapse_summary_artifacts(
        messages,
        llm,
        compaction_model,
        CollapseSummaryRequest {
            plan_runtime,
            session_model,
            cache_key: None,
            resolved_output_limit: None,
            transcript_path: None,
        },
    )
    .await
}

async fn build_collapse_summary_artifacts(
    messages: &[ChatMessage],
    llm: &dyn LlmProvider,
    compaction_model: &str,
    request: CollapseSummaryRequest<'_>,
) -> Result<CollapseSummaryArtifacts, AppError> {
    let working: Vec<ChatMessage> = messages
        .iter()
        .filter(|msg| msg.role != ChatMessageRole::System)
        .cloned()
        .collect();
    let (covered_start_id, covered_end_id) = collapse_bounds(&working)
        .ok_or_else(|| AppError::Config("collapse 缺少 message 锚点".to_string()))?;
    // 控制态与用户原话由 generate_summary 内的 machine_block 统一拼接，
    // recent-files 也在同一个入口生成，避免不同 compaction 路径漏掉其中一块。
    let control = request
        .plan_runtime
        .map(|rt| rt.control_snapshot(request.session_model));
    let summary_text = generate_summary_with_output_limit(
        &working,
        None,
        llm,
        compaction_model,
        control.as_ref(),
        SummaryRequestOptions {
            cache_key: request.cache_key,
            resolved_output_limit: request.resolved_output_limit,
            transcript_path: request.transcript_path,
        },
    )
    .await?;
    let entry_id = compound_turn_id(&covered_start_id, &covered_end_id);
    let covered_count = working
        .iter()
        .filter(|msg| msg.kind != MessageKind::CompactionSummary)
        .count();
    let transcript_entry = TranscriptEntry::BranchSummary(BranchSummaryEntry {
        id: Some(entry_id.clone()),
        parent_id: None,
        timestamp: Utc::now().to_rfc3339(),
        summary: Some(summary_text.clone()),
        covered_start_id: Some(covered_start_id.clone()),
        covered_end_id: Some(covered_end_id.clone()),
        covered_count: Some(covered_count),
        is_boundary: Some(true),
        preheat_compaction_id: None,
        estimated_covered_tokens_before: None,
        estimated_summary_tokens: None,
        estimated_tokens_saved: None,
        error: None,
        attempts: None,
    });
    let summary_message = apply_collapse_summary(
        &working,
        &summary_text,
        &covered_start_id,
        &covered_end_id,
        &entry_id,
    )?;
    Ok(CollapseSummaryArtifacts {
        summary_text,
        summary_message,
        transcript_entry,
        covered_start_id,
        covered_end_id,
        entry_id,
    })
}

fn maybe_write_collapse_entry(
    path: &Path,
    anchor_id: &str,
    entry: &TranscriptEntry,
) -> Result<(), AppError> {
    if path.as_os_str().is_empty() {
        return Ok(());
    }
    // Collapse normally covers through the current durable tail, so its boundary can be appended
    // in O(1), like /compact and Scheme E preheat. Retain anchor insertion only for an extreme
    // interleaving where another transcript entry arrived while the summary LLM call was running.
    let tail_is_anchor = read_entries_tail(path, 1)?
        .last()
        .and_then(entry_id)
        .is_some_and(|id| id == anchor_id);
    if tail_is_anchor {
        append_entry(path, entry)
    } else {
        insert_entry_after_message_id(path, anchor_id, entry)
    }
}

fn apply_collapse_summary(
    working: &[ChatMessage],
    summary_text: &str,
    covered_start_id: &str,
    covered_end_id: &str,
    entry_id: &str,
) -> Result<ChatMessage, AppError> {
    let total_chars: usize = working.iter().map(estimate_msg_chars).sum();
    let mut temp = ContextState {
        messages: working.to_vec(),
        estimate_context_chars: total_chars,
        context_budget_chars: total_chars,
        context_budget_tokens: 1,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        latest_plan_event: None,
        resume_control: Default::default(),
        preheat: crate::core::compaction::preheat::Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    };
    temp.apply_boundary(CompactionResult {
        summary_text: summary_text.to_string(),
        covered_start_id: covered_start_id.to_string(),
        covered_end_id: covered_end_id.to_string(),
        covered_count: working.len(),
        transcript_compaction_entry_id: Some(entry_id.to_string()),
        estimated_covered_tokens_before: None,
        estimated_summary_tokens: None,
        estimated_tokens_saved: None,
        preheat_elapsed_ms: 0,
    })?;
    temp.messages
        .into_iter()
        .next()
        .ok_or_else(|| AppError::internal("collapse summary missing"))
}

fn collapse_bounds(working: &[ChatMessage]) -> Option<(String, String)> {
    let start = working
        .iter()
        .find(|msg| msg.kind != MessageKind::CompactionSummary)
        .and_then(|msg| msg.msg_id.clone())?;
    let end = working
        .iter()
        .rev()
        .find(|msg| msg.kind != MessageKind::CompactionSummary)
        .and_then(|msg| msg.msg_id.clone())?;
    Some((start, end))
}

fn ensure_working_message_ids(
    agent: &AgentLoop,
    working: &mut [ChatMessage],
) -> Result<(), AppError> {
    for msg in working {
        if msg.msg_id.is_some() {
            continue;
        }
        agent.persist_message_if_needed(msg)?;
        if msg.msg_id.is_none() {
            msg.msg_id = Some(generate_entry_id());
        }
    }
    Ok(())
}

fn apply_placeholder_wave(
    ctx_state: &mut ContextState,
    messages: &mut [ChatMessage],
    candidates: Vec<TailCandidate>,
    freed_chars: &mut usize,
    evicted_tool_call_ids: &mut Vec<String>,
) {
    let wave = std::cmp::max(1, candidates.len() / 2);
    let chars_before = *freed_chars;
    let mut rewritten_messages = Vec::new();
    for candidate in candidates.into_iter().take(wave) {
        if let Some(id) = messages[candidate.msg_idx].tool_call_id.clone() {
            evicted_tool_call_ids.push(id);
        }
        let Some(text) = text_content_mut(&mut messages[candidate.msg_idx]) else {
            continue;
        };
        let old_len = text.len();
        *text = TOOL_RESULT_PLACEHOLDER.to_string();
        *freed_chars += old_len.saturating_sub(text.len());
        ctx_state.rewrite_local_tail_chars(old_len, text.len());
        rewritten_messages.push(
            candidate
                .message_id
                .unwrap_or_else(|| "<unpersisted>".to_string()),
        );
    }
    let chars_freed = (*freed_chars).saturating_sub(chars_before);
    if chars_freed > 0 {
        info!(
            target: "tomcat_chat_diag",
            phase = "tool_results_marked_compacted",
            operation = "apply_placeholder_wave",
            chars_freed,
            rewritten_messages = ?rewritten_messages,
        );
    }
}

fn context_is_over_budget(agent: &AgentLoop) -> bool {
    agent
        .context_state
        .as_ref()
        .is_some_and(ContextState::is_over_budget)
}

fn ctx_ratio(working_tokens: usize, budget_tokens: usize) -> f64 {
    if budget_tokens == 0 {
        return f64::MAX;
    }
    working_tokens as f64 / budget_tokens as f64
}

fn log_aggregate_precheck_decision(decision: &AggregatePrecheckDecision) {
    info!(
        target: "tomcat_chat_diag",
        phase = "mid_turn_precheck",
        route = decision.route.as_str(),
        route_reason = decision.route_reason.as_str(),
        working_tokens = decision.working_tokens,
        budget_tokens = decision.budget_tokens,
        overflow = decision.overflow_tokens,
        max_reducible = decision.max_reducible,
        candidate_count = decision.candidate_count,
        yellow_lamp_only = decision.yellow_lamp_only,
        after_each_wave = ?decision.after_each_wave,
        after_collapse = ?decision.after_collapse
    );
}

fn text_content_mut(msg: &mut ChatMessage) -> Option<&mut String> {
    match msg.content.as_mut() {
        Some(crate::core::llm::ChatMessageContent::Text(text)) => Some(text),
        _ => None,
    }
}
