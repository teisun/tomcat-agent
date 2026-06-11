use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::Utc;
use tracing::{info, warn};

use crate::core::compaction::apply::check_after_reply;
use crate::core::compaction::preheat::generate_summary;
use crate::core::compaction::{
    compact_tool_results, is_persisted_tool_result_text, persist_tool_result_text,
    TOOL_RESULT_PLACEHOLDER,
};
use crate::core::llm::{ChatMessage, ChatMessageRole, LlmProvider, MessageKind};
use crate::core::plan_runtime::file_store::{read_plan, TodoItem, TodoStatus};
use crate::core::plan_runtime::PlanRuntime;
use crate::core::session::manager::{
    build_context_from_state, compound_turn_id, estimate_msg_chars, estimated_tokens_from_chars,
    generate_entry_id, CompactionResult, ContextState, PlanEventRef,
};
use crate::core::session::transcript::{
    insert_entry_after_message_id, rewrite_message_text_entries_by_id, BranchSummaryEntry,
    MessageTextRewrite, TranscriptEntry,
};
use crate::infra::error::AppError;

use super::types::AgentLoop;

const COMPACTABLE_TOOLS: &[&str] = &["read", "search_files", "bash", "task_output"];

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
        agent.emit_context_metrics();
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

    let applied_history = {
        let Some(ctx_state) = agent.context_state.as_mut() else {
            return Ok(result);
        };
        check_after_reply(ctx_state, &agent.emitter)
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
        let reduced = compact_tool_results(ctx_state, &agent.config.context_config);
        if reduced > 0 {
            ctx_state.invalidate_api_usage();
        }
        reduced
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
    let mut transcript_rewrites = Vec::new();
    let mut result = TailReductionResult::default();
    let mut step0_reduced = false;

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
            &mut transcript_rewrites,
            &mut result.freed_chars,
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
            &mut transcript_rewrites,
            &mut result.freed_chars,
        );
        result
            .after_each_wave
            .push(ctx_state.estimated_token_count());
    }

    rewrite_transcript_best_effort(&ctx_state.transcript_path, transcript_rewrites);
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
        .filter(|(_, msg)| {
            (msg.role == ChatMessageRole::User && msg.kind != MessageKind::Steering)
                || msg.kind == MessageKind::CompactionSummary
        })
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

async fn collapse_to_branch_summary(
    agent: &mut AgentLoop,
    messages: &mut Vec<ChatMessage>,
) -> Result<(), AppError> {
    let plan_runtime = agent.config.plan_runtime.clone();
    let latest_plan_event = agent
        .context_state
        .as_ref()
        .and_then(|state| state.latest_plan_event.clone());
    let mut working: Vec<ChatMessage> = messages
        .iter()
        .filter(|msg| msg.role != ChatMessageRole::System)
        .cloned()
        .collect();
    ensure_working_message_ids(agent, &mut working)?;
    let compaction_provider = agent.compaction_provider();
    let artifacts = build_collapse_summary_artifacts_for_test(
        &working,
        compaction_provider.as_ref(),
        &agent.config.context_config.compaction_model,
        plan_runtime.as_deref(),
        latest_plan_event.as_ref(),
    )
    .await?;
    let Some(ctx_state) = agent.context_state.as_mut() else {
        return Ok(());
    };
    if let Err(err) = maybe_write_collapse_entry(
        &ctx_state.transcript_path,
        &artifacts.covered_end_id,
        &artifacts.transcript_entry,
    ) {
        warn!(error = %err, "collapse branch_summary transcript write failed");
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

#[doc(hidden)]
pub async fn build_collapse_summary_artifacts_for_test(
    messages: &[ChatMessage],
    llm: &dyn LlmProvider,
    compaction_model: &str,
    plan_runtime: Option<&PlanRuntime>,
    latest_plan_event: Option<&PlanEventRef>,
) -> Result<CollapseSummaryArtifacts, AppError> {
    let working: Vec<ChatMessage> = messages
        .iter()
        .filter(|msg| msg.role != ChatMessageRole::System)
        .cloned()
        .collect();
    let (covered_start_id, covered_end_id) = collapse_bounds(&working)
        .ok_or_else(|| AppError::Config("collapse 缺少 message 锚点".to_string()))?;
    let summary = generate_summary(&working, None, llm, compaction_model).await?;
    let summary_text = format!(
        "## Structured Summary\n{}\n\n## Execution Keepalive\n{}",
        summary.trim(),
        build_keepalive_snapshot(plan_runtime, latest_plan_event)
    );
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
    insert_entry_after_message_id(path, anchor_id, entry)
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
    transcript_rewrites: &mut Vec<MessageTextRewrite>,
    freed_chars: &mut usize,
) {
    let wave = std::cmp::max(1, candidates.len() / 2);
    for candidate in candidates.into_iter().take(wave) {
        let Some(text) = text_content_mut(&mut messages[candidate.msg_idx]) else {
            continue;
        };
        let old_len = text.len();
        *text = TOOL_RESULT_PLACEHOLDER.to_string();
        *freed_chars += old_len.saturating_sub(text.len());
        ctx_state.rewrite_local_tail_chars(old_len, text.len());
        if let Some(message_id) = candidate.message_id {
            transcript_rewrites.push(MessageTextRewrite {
                message_id,
                new_content: text.clone(),
            });
        }
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

fn build_keepalive_snapshot(
    plan_runtime: Option<&PlanRuntime>,
    latest_plan_event: Option<&PlanEventRef>,
) -> String {
    let Some(plan_runtime) = plan_runtime else {
        return format!(
            "- mode: chat\n- active_plan_path: (none)\n- active_plan_id: (none)\n- current_step: (none)\n- pending_work: (none)\n- latest_plan_event: {}",
            format_plan_event(latest_plan_event)
        );
    };
    let mode = plan_runtime.mode();
    let active_plan_path = plan_runtime
        .active_plan_path()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "(none)".to_string());
    let active_plan_id = mode
        .active_plan_id()
        .map(str::to_string)
        .or_else(|| plan_runtime.active_planning_plan_id())
        .unwrap_or_else(|| "(none)".to_string());
    let todos = match mode {
        crate::core::plan_runtime::PlanState::Planning => plan_runtime.snapshot_session_todos(),
        crate::core::plan_runtime::PlanState::Executing { .. }
        | crate::core::plan_runtime::PlanState::Pending { .. } => plan_runtime
            .active_plan_path()
            .and_then(|path| read_plan(&path).ok())
            .map(|plan| plan.frontmatter.todos)
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    format!(
        "- mode: {}\n- active_plan_path: {}\n- active_plan_id: {}\n- current_step: {}\n- pending_work: {}\n- latest_plan_event: {}",
        mode.as_str(),
        active_plan_path,
        active_plan_id,
        pick_current_step(&todos),
        format_pending_work(&todos),
        format_plan_event(latest_plan_event)
    )
}

fn pick_current_step(todos: &[TodoItem]) -> String {
    todos
        .iter()
        .find(|todo| todo.status == TodoStatus::InProgress)
        .or_else(|| todos.iter().find(|todo| todo.status == TodoStatus::Pending))
        .map(|todo| todo.content.clone())
        .unwrap_or_else(|| "(none)".to_string())
}

fn format_pending_work(todos: &[TodoItem]) -> String {
    let items: Vec<String> = todos
        .iter()
        .filter(|todo| matches!(todo.status, TodoStatus::Pending | TodoStatus::InProgress))
        .take(5)
        .map(|todo| format!("{} [{}]", todo.content, todo.status.as_str()))
        .collect();
    if items.is_empty() {
        "(none)".to_string()
    } else {
        items.join(" | ")
    }
}

fn format_plan_event(event: Option<&crate::core::session::manager::PlanEventRef>) -> String {
    let Some(event) = event else {
        return "(none)".to_string();
    };
    let kind = match event.kind {
        crate::core::session::manager::PlanEventKind::Create => "create",
        crate::core::session::manager::PlanEventKind::Build => "build",
        crate::core::session::manager::PlanEventKind::Update => "update",
    };
    format!("{kind}:{}:{}", event.plan_id, event.path.display())
}

fn text_content_mut(msg: &mut ChatMessage) -> Option<&mut String> {
    match msg.content.as_mut() {
        Some(crate::core::llm::ChatMessageContent::Text(text)) => Some(text),
        _ => None,
    }
}
