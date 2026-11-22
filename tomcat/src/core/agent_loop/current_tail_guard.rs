use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::Utc;
use tracing::{info, warn};

use crate::core::compaction::apply::BoundaryEnv;
use crate::core::compaction::preheat::{
    generate_summary_with_output_limit, PreheatOutcome, SummaryRequestOptions,
};
use crate::core::compaction::{
    compact_tool_results, is_persisted_tool_result_text, persist_tool_result_text,
    TOOL_RESULT_PLACEHOLDER,
};
use crate::core::llm::{
    ChatMessage, ChatMessageRole, LlmProvider, MessageKind, PromptCacheKeyFamily,
};
use crate::core::plan_runtime::PlanRuntime;
use crate::core::session::manager::{
    compound_turn_id, estimate_msg_chars, estimated_tokens_from_chars, generate_entry_id,
    CompactionResult, ContextState,
};
use crate::core::session::transcript::{
    append_entry, entry_id, insert_entry_after_message_id, read_entries_tail,
    rewrite_message_text_entries_by_id, BranchSummaryEntry, MessageTextRewrite, TranscriptEntry,
};
use crate::core::session::user_message_sidecar::ensure_user_message_sidecar_current;

use crate::infra::error::AppError;
use crate::infra::event_bus::ScopedEventEmitter;
use crate::infra::events::AgentEvent;

use super::types::AgentLoop;

const COMPACTABLE_TOOLS: &[&str] = &["read", "search_files", "bash", "task_output"];
/// Build-mode folds the prefix captured at the 50% waterline, retaining the later half as the
/// agent's working set. This is deliberately not a user configuration knob: its value is a
/// policy trade-off, and cache/compaction observations decide whether it should become 0.25.
const MIDTURN_FOLD_KEEP_RATIO: f64 = 0.5;
const PREHEAT_HANDOFF_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AggregatePrecheckDecision {
    pub fits: bool,
    pub working_tokens: usize,
    pub budget_tokens: usize,
    pub overflow_tokens: usize,
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

#[cfg(test)]
pub(super) async fn maybe_reduce_before_next_llm(
    agent: &mut AgentLoop,
    messages: &mut Vec<ChatMessage>,
) -> Result<(), AppError> {
    let _ = prepare_request_context(agent, messages, &mut false).await?;
    Ok(())
}

pub(super) async fn prepare_request_context(
    agent: &mut AgentLoop,
    messages: &mut Vec<ChatMessage>,
    proactive_collapse_failed: &mut bool,
) -> Result<Option<AggregatePrecheckDecision>, AppError> {
    let Some(ctx_state) = agent.context_state.as_mut() else {
        return Ok(None);
    };
    ctx_state.preheat.settle();
    let working_tokens = ctx_state.estimated_token_count();
    let budget_tokens = ctx_state.context_budget_tokens;
    if budget_tokens == 0 {
        return Ok(None);
    }

    let ratio = ctx_state.usage_ratio();
    let mut decision = build_precheck_decision(working_tokens, budget_tokens);
    if decision.yellow_lamp_only {
        info!(
            target: "tomcat_chat_diag",
            phase = "mid_turn_precheck_yellow",
            ratio,
            working_tokens = decision.working_tokens,
            budget_tokens = decision.budget_tokens
        );
    }
    if decision.fits {
        start_midturn_preheat_if_needed(agent, messages)?;
        log_aggregate_precheck_decision(&decision);
        return Ok(Some(decision));
    }

    // Only actual reductions can tell us whether we still need B.
    let reduction = reduce_before_next_llm(agent, messages)?;
    decision.after_each_wave = reduction.after_each_wave;
    let mut mutated = reduction.mutated;
    if context_is_over_budget(agent) && !*proactive_collapse_failed {
        if handoff_preheat_before_collapse(agent, messages).await? {
            mutated = true;
            if context_is_over_budget(agent) {
                let after_preheat = reduce_before_next_llm(agent, messages)?;
                decision
                    .after_each_wave
                    .extend(after_preheat.after_each_wave);
            }
        }
        if context_is_over_budget(agent) {
            match collapse_to_branch_summary(agent, messages).await? {
                CollapseOutcome::Applied => {
                    decision.after_collapse = agent
                        .context_state
                        .as_ref()
                        .map(ContextState::estimated_token_count);
                    mutated = true;
                }
                CollapseOutcome::GenerationFailed => *proactive_collapse_failed = true,
            }
        }
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
    prepare_request_context(agent, messages, &mut false).await
}

fn build_precheck_decision(
    working_tokens: usize,
    budget_tokens: usize,
) -> AggregatePrecheckDecision {
    let fits = working_tokens <= budget_tokens;
    AggregatePrecheckDecision {
        fits,
        working_tokens,
        budget_tokens,
        overflow_tokens: working_tokens.saturating_sub(budget_tokens),
        yellow_lamp_only: fits && ctx_ratio(working_tokens, budget_tokens) >= 0.90,
        after_each_wave: Vec::new(),
        after_collapse: None,
    }
}

fn reduce_before_next_llm(
    agent: &mut AgentLoop,
    messages: &mut Vec<ChatMessage>,
) -> Result<ReductionResult, AppError> {
    let mut result = ReductionResult::default();
    if apply_ready_preheat(agent, messages, None)? {
        result.mutated = true;
        if !context_is_over_budget(agent) {
            return Ok(result);
        }
    }
    let history_reduced = {
        let Some(ctx_state) = agent.context_state.as_mut() else {
            return Ok(result);
        };
        let history_end = agent.start_idx.min(messages.len());
        let placeholder = compact_tool_results(
            ctx_state,
            messages,
            &agent.config.context_config,
            history_end,
        );
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

    if messages.is_empty() {
        return Ok(());
    }
    ensure_working_message_ids(agent, messages)?;
    let transcript_path = ctx_state.transcript_path.clone();
    let compaction_provider = agent.compaction_provider();
    let compaction_input_budget = agent.compaction_input_budget();
    let cache_key = PromptCacheKeyFamily::Compaction.key_for(&agent.config.session_id);
    let control_snapshot = crate::core::plan_runtime::ControlSnapshot::capture(
        agent.config.plan_runtime.as_deref(),
        agent.todos_runtime.as_deref(),
        Some(agent.wire_model()),
    );
    let emitter = std::sync::Arc::new(agent.emitter.clone());
    let covered_count = messages.len();

    let started = agent.context_state.as_mut().is_some_and(|ctx_state| {
        ctx_state.preheat.try_start(
            usage_ratio,
            messages,
            &transcript_path,
            cache_key,
            compaction_provider,
            agent.config.compaction_output_limit,
            Some(compaction_input_budget),
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

/// Apply a ready preheat directly to the sole working list.
pub(super) fn apply_ready_preheat(
    agent: &mut AgentLoop,
    messages: &mut Vec<ChatMessage>,
    min_ratio: Option<f64>,
) -> Result<bool, AppError> {
    let Some(state) = agent.context_state.as_mut() else {
        return Ok(false);
    };
    state.preheat.settle();
    if min_ratio.is_some_and(|minimum| state.usage_ratio() < minimum) {
        return Ok(false);
    }
    let outcome = state.preheat.poll_result();
    Ok(apply_preheat_outcome(agent, messages, outcome, false))
}

fn apply_preheat_outcome(
    agent: &mut AgentLoop,
    messages: &mut Vec<ChatMessage>,
    outcome: PreheatOutcome,
    waited: bool,
) -> bool {
    let PreheatOutcome::Completed(result) = outcome else {
        return false;
    };
    let boundary_env = BoundaryEnv {
        config: &agent.config.context_config,
        work_dir: Path::new(&agent.config.agent_trail_dir),
        session_id: &agent.config.session_id,
        read_file_state: agent.config.read_file_state.as_ref(),
    };
    let Some(ctx_state) = agent.context_state.as_mut() else {
        return false;
    };
    crate::core::compaction::apply::apply_and_emit_boundary(
        ctx_state,
        messages,
        result,
        ctx_state.usage_ratio(),
        waited,
        &agent.emitter,
        &boundary_env,
        &mut agent.start_idx,
    )
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
    if let Err(err) = crate::infra::config::lock::with_config_lock(path, || {
        rewrite_message_text_entries_by_id(path, &merged)
    }) {
        warn!(error = %err, "mid-turn transcript rewrite failed");
    }
}

/// UI phase follows the blocking operation, including errors and cancellation.
struct ForegroundCompaction(ScopedEventEmitter);

impl ForegroundCompaction {
    fn start(emitter: &ScopedEventEmitter) -> Self {
        let _ = emitter.emit(AgentEvent::ForegroundCompactionStart);
        Self(emitter.clone())
    }
}

impl Drop for ForegroundCompaction {
    fn drop(&mut self) {
        let _ = self.0.emit(AgentEvent::ForegroundCompactionEnd);
    }
}

fn compaction_cancelled() -> AppError {
    AppError::Llm("compaction cancelled".to_string())
}

/// Only call at the point where B would otherwise run, never on the normal send path.
pub(super) async fn handoff_preheat_before_collapse(
    agent: &mut AgentLoop,
    messages: &mut Vec<ChatMessage>,
) -> Result<bool, AppError> {
    let cancel = &agent.cancel_token;
    if cancel.is_cancelled() {
        return Err(compaction_cancelled());
    }
    let Some(state) = agent.context_state.as_mut() else {
        return Ok(false);
    };
    state.preheat.settle();
    let waiting = state.preheat.is_running();
    let _phase = waiting.then(|| ForegroundCompaction::start(&agent.emitter));
    let outcome = if waiting {
        info!(target: "tomcat_chat_diag", phase = "compaction_wait_preheat");
        state
            .preheat
            .await_result_or_stop(PREHEAT_HANDOFF_TIMEOUT, cancel)
            .await
    } else {
        state.preheat.poll_result()
    };
    if cancel.is_cancelled() {
        return Err(compaction_cancelled());
    }
    let status = match &outcome {
        PreheatOutcome::Completed(_) => "ready",
        PreheatOutcome::Exhausted => "failed",
        PreheatOutcome::Failed => "reaped",
        PreheatOutcome::NotReady => "absent",
    };
    let applied = apply_preheat_outcome(agent, messages, outcome, waiting);
    info!(target: "tomcat_chat_diag", phase = "compaction_preheat_handoff", status, applied);
    Ok(applied)
}

pub(super) enum CollapseOutcome {
    Applied,
    GenerationFailed,
}

// Keep structure/persistence failures distinct from errors produced by the model call.
enum CollapseBuildError {
    Structure(AppError),
    Generation(AppError),
}

impl CollapseBuildError {
    fn into_inner(self) -> AppError {
        match self {
            Self::Structure(error) | Self::Generation(error) => error,
        }
    }
}

pub(super) async fn collapse_to_branch_summary(
    agent: &mut AgentLoop,
    messages: &mut Vec<ChatMessage>,
) -> Result<CollapseOutcome, AppError> {
    if agent.cancel_token.is_cancelled() {
        return Err(compaction_cancelled());
    }
    let plan_runtime = agent.config.plan_runtime.clone();
    let todos_runtime = agent.todos_runtime.clone();
    let session_model = agent.wire_model().to_string();
    ensure_working_message_ids(agent, messages)?;
    let transcript_path = agent
        .context_state
        .as_ref()
        .map(|state| state.transcript_path.clone())
        .unwrap_or_default();

    let compaction_provider = agent.compaction_provider();
    let cache_key = PromptCacheKeyFamily::Compaction.key_for(&agent.config.session_id);
    let request = CollapseSummaryRequest {
        plan_runtime: plan_runtime.as_deref(),
        todos_runtime: todos_runtime.as_deref(),
        session_model: Some(session_model.as_str()),
        cache_key: cache_key.as_deref(),
        resolved_output_limit: agent.config.compaction_output_limit,
        input_budget_tokens: Some(agent.compaction_input_budget()),
        transcript_path: (!transcript_path.as_os_str().is_empty())
            .then_some(transcript_path.as_path()),
    };
    let cancel = agent.cancel_token.clone();
    let _phase = ForegroundCompaction::start(&agent.emitter);
    let mut generation_attempts = 1;
    let summary_request = async {
        let artifacts = match build_collapse_summary_artifacts(
            messages,
            compaction_provider.as_ref(),
            &agent.config.context_config.compaction_model,
            request,
        )
        .await
        {
            Ok(artifacts) => artifacts,
            Err(CollapseBuildError::Generation(error))
                if agent.config.context_config.compaction_model != session_model =>
            {
                generation_attempts += 1;
                let reason = super::error_classifier::err_snippet(&error.to_string());
                agent.emit_event(crate::infra::events::AgentEvent::LlmNotice {
                    finish_reason: "compaction_model_fallback".to_string(),
                    message: crate::infra::i18n::tr(
                        "agentLoop.compactionFallback",
                        &[
                            ("model", &agent.config.context_config.compaction_model),
                            ("fallback", agent.catalog_id()),
                            ("reason", &reason),
                        ],
                    ),
                });
                build_collapse_summary_artifacts(
                    messages,
                    agent.llm.as_ref(),
                    &session_model,
                    CollapseSummaryRequest {
                        resolved_output_limit: None,
                        input_budget_tokens: Some(agent.binding.limits.input_budget_tokens),
                        ..request
                    },
                )
                .await?
            }
            Err(error) => return Err(error),
        };
        Ok::<_, CollapseBuildError>(artifacts)
    };
    // Both proactive and overflow compaction share this cancellation boundary.
    // Once artifacts are ready, join disk/sidecar publication and the memory update.
    let artifacts = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(compaction_cancelled()),
        result = summary_request => match result {
            Ok(artifacts) => artifacts,
            Err(CollapseBuildError::Structure(error)) => return Err(error),
            Err(CollapseBuildError::Generation(error)) => {
                agent.emit_event(AgentEvent::CompactionError {
                    exhausted_after_retries: false,
                    attempts: generation_attempts,
                    error: error.to_string(),
                    source: "collapse".into(),
                    ratio: agent.context_state.as_ref().map(ContextState::usage_ratio),
                });
                info!(target: "tomcat_chat_diag", phase = "compaction_collapse", outcome = "generation_failed");
                return Ok(CollapseOutcome::GenerationFailed);
            }
        },
    };
    let ctx_state = agent
        .context_state
        .as_mut()
        .ok_or_else(|| AppError::internal("collapse requires context state"))?;
    maybe_write_collapse_entry(
        &ctx_state.transcript_path,
        &artifacts.covered_end_id,
        &artifacts.transcript_entry,
    )?;
    let _ = ensure_user_message_sidecar_current(&ctx_state.transcript_path).await;

    let summary_msg = artifacts.summary_message;
    let new_chars = estimate_msg_chars(&summary_msg);
    let saved_chars = ctx_state.estimate_context_chars.saturating_sub(new_chars);
    ctx_state.estimate_context_chars = new_chars;
    ctx_state.invalidate_api_usage();
    ctx_state.preheat.abort();
    ctx_state.session_obs.compaction_count =
        ctx_state.session_obs.compaction_count.saturating_add(1);
    ctx_state.session_obs.compaction_tokens_freed += estimated_tokens_from_chars(saved_chars);

    *messages = vec![summary_msg];
    agent.start_idx = 1;
    info!(target: "tomcat_chat_diag", phase = "compaction_collapse", outcome = "applied");
    Ok(CollapseOutcome::Applied)
}

/// 生成 collapse 摘要所需的可选运行时资料，集中传递避免 helper 位置参数继续增长。
#[derive(Clone, Copy)]
struct CollapseSummaryRequest<'a> {
    plan_runtime: Option<&'a PlanRuntime>,
    todos_runtime: Option<&'a crate::core::plan_runtime::todo_runtime::TodosRuntime>,
    session_model: Option<&'a str>,
    cache_key: Option<&'a str>,
    resolved_output_limit: Option<u32>,
    input_budget_tokens: Option<usize>,
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
            todos_runtime: None,
            session_model,
            cache_key: None,
            resolved_output_limit: None,
            input_budget_tokens: None,
            transcript_path: None,
        },
    )
    .await
    .map_err(CollapseBuildError::into_inner)
}

async fn build_collapse_summary_artifacts(
    messages: &[ChatMessage],
    llm: &dyn LlmProvider,
    compaction_model: &str,
    request: CollapseSummaryRequest<'_>,
) -> Result<CollapseSummaryArtifacts, CollapseBuildError> {
    let (covered_start_id, covered_end_id) = collapse_bounds(messages).ok_or_else(|| {
        CollapseBuildError::Structure(AppError::Config(
            "collapse is missing a message anchor".to_string(),
        ))
    })?;
    // 控制态与用户原话由 generate_summary 内的 machine_block 统一拼接，
    // recent-files 也在同一个入口生成，避免不同 compaction 路径漏掉其中一块。
    let control = crate::core::plan_runtime::ControlSnapshot::capture(
        request.plan_runtime,
        request.todos_runtime,
        request.session_model,
    );
    let summary_text = generate_summary_with_output_limit(
        messages,
        None,
        llm,
        compaction_model,
        control.as_ref(),
        SummaryRequestOptions {
            cache_key: request.cache_key,
            resolved_output_limit: request.resolved_output_limit,
            input_budget_tokens: request.input_budget_tokens,
            transcript_path: request.transcript_path,
        },
    )
    .await
    .map_err(CollapseBuildError::Generation)?;
    let entry_id = compound_turn_id(&covered_start_id, &covered_end_id);
    let covered_count = messages
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
        superseded: false,
    });
    let summary_message = apply_collapse_summary(
        messages,
        &summary_text,
        &covered_start_id,
        &covered_end_id,
        &entry_id,
    )
    .map_err(CollapseBuildError::Structure)?;
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
    let _guard = crate::infra::config::lock::acquire_config_lock(path)?;
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
    let mut messages = working.to_vec();
    let mut temp = ContextState {
        messages: Vec::new(),
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
        hydrated: true,
    };
    let mut turn_start = messages.len();
    temp.apply_boundary(
        &mut messages,
        CompactionResult {
            summary_text: summary_text.to_string(),
            covered_start_id: covered_start_id.to_string(),
            covered_end_id: covered_end_id.to_string(),
            covered_count: working.len(),
            transcript_compaction_entry_id: Some(entry_id.to_string()),
            estimated_covered_tokens_before: None,
            estimated_summary_tokens: None,
            estimated_tokens_saved: None,
            preheat_elapsed_ms: 0,
        },
        &mut turn_start,
    )?;
    messages
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

pub(super) fn ensure_working_message_ids(
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
        fits = decision.fits,
        working_tokens = decision.working_tokens,
        budget_tokens = decision.budget_tokens,
        overflow = decision.overflow_tokens,
        yellow_lamp_only = decision.yellow_lamp_only,
        after_each_wave = ?decision.after_each_wave,
        after_collapse = ?decision.after_collapse
    );
}

fn text_content_mut(msg: &mut ChatMessage) -> Option<&mut String> {
    msg.text_content_mut()
}
