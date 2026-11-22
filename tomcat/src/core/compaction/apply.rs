//! Layer 2: nonblocking application at the request and reply waterlines.

use std::path::Path;

use tracing::warn;

use crate::core::compaction::preheat::PreheatOutcome;
use crate::core::compaction::run_layer0_cleanup;
use crate::core::session::manager::{estimated_tokens_from_chars, CompactionResult, ContextState};
use crate::core::session::transcript::{
    append_entry, read_entries_tail, BranchSummaryTextEntry, TranscriptEntry,
};
use crate::core::tools::pipeline::read_state::ReadFileState;
use crate::infra::config::ContextConfig;
use crate::infra::error::AppError;
use crate::infra::event_bus::ScopedEventEmitter;
use crate::infra::events::AgentEvent;

/// 执行 boundary 后续 L0 清理所需的运行时依赖。
///
/// `ContextState` 只保存可持久化的会话上下文；目录和 read stamp 属于本轮执行期
/// 依赖，显式由调用方传入，避免把两种职责混进状态快照。
pub struct BoundaryEnv<'a> {
    pub config: &'a ContextConfig,
    pub work_dir: &'a Path,
    pub session_id: &'a str,
    pub read_file_state: &'a ReadFileState,
}

/// Apply ready preheat at the post-reply waterline, without waiting on generation.
#[must_use = "a successful boundary application rewrites the supplied working message list"]
pub fn check_after_reply(
    state: &mut ContextState,
    messages: &mut Vec<crate::core::llm::ChatMessage>,
    emitter: &ScopedEventEmitter,
    env: &BoundaryEnv<'_>,
    turn_start: &mut usize,
) -> bool {
    apply_at_waterline(state, messages, emitter, env, turn_start, 0.85)
}

/// Apply ready preheat at the pre-request waterline, without waiting on generation.
#[must_use = "a successful boundary application rewrites the supplied working message list"]
pub fn check_before_request(
    state: &mut ContextState,
    messages: &mut Vec<crate::core::llm::ChatMessage>,
    emitter: &ScopedEventEmitter,
    env: &BoundaryEnv<'_>,
    turn_start: &mut usize,
) -> bool {
    apply_at_waterline(state, messages, emitter, env, turn_start, 0.70)
}

fn apply_at_waterline(
    state: &mut ContextState,
    messages: &mut Vec<crate::core::llm::ChatMessage>,
    emitter: &ScopedEventEmitter,
    env: &BoundaryEnv<'_>,
    turn_start: &mut usize,
    minimum_ratio: f64,
) -> bool {
    state.preheat.settle();
    let ratio = state.usage_ratio();
    if ratio < minimum_ratio {
        return false;
    }
    let PreheatOutcome::Completed(result) = state.preheat.poll_result() else {
        return false;
    };
    apply_and_emit_boundary(
        state, messages, result, ratio, false, emitter, env, turn_start,
    )
}

// ---------------------------------------------------------------------------
// apply_and_emit_boundary
// ---------------------------------------------------------------------------

// The primitive deliberately receives both the working list and its tail cursor explicitly:
// hiding either in `ContextState` would recreate the two-list ownership ambiguity this API removes.
#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_and_emit_boundary(
    state: &mut ContextState,
    messages: &mut Vec<crate::core::llm::ChatMessage>,
    result: CompactionResult,
    ratio_before: f64,
    was_sync_wait: bool,
    emitter: &ScopedEventEmitter,
    env: &BoundaryEnv<'_>,
    turn_start: &mut usize,
) -> bool {
    let covered_count = result.covered_count;
    let saved = result.estimated_tokens_saved.unwrap_or(0);

    if !messages
        .iter()
        .any(|message| message.msg_id.as_deref() == Some(result.covered_end_id.as_str()))
    {
        let error = AppError::ApplyBoundaryStale {
            covered_end_id: result.covered_end_id.clone(),
        };
        warn!(
            target: "tomcat_chat_diag",
            %error,
            phase = "apply_boundary_stale",
            "discarding stale preheat result without surfacing a user-actionable compaction error"
        );
        state.preheat.discard_cached_completed();
        return false;
    }

    if let Err(error) = append_completed_summary_text(state, &result) {
        warn!(
            %error,
            "could not persist completed compaction summary; retaining raw context for retry"
        );
        let _ = emitter.emit(AgentEvent::CompactionError {
            exhausted_after_retries: false,
            attempts: 0,
            error: error.to_string(),
            source: "apply".to_string(),
            ratio: Some(state.usage_ratio()),
        });
        state.preheat.restore_pending_result(result);
        return false;
    }

    match state.apply_boundary(messages, result.clone(), turn_start) {
        Ok(()) => {
            state.session_obs.compaction_tokens_freed += saved;
            state.session_obs.compaction_count =
                state.session_obs.compaction_count.saturating_add(1);

            let ratio_after = state.usage_ratio();
            let _ = emitter.emit(AgentEvent::BoundarySwitched {
                ratio_before,
                ratio_after,
                covered_count,
                was_sync_wait,
                estimated_tokens_freed: saved,
            });

            run_layer0_after_boundary(state, messages, emitter, env, *turn_start);
            true
        }
        Err(e @ AppError::ApplyBoundaryStale { .. }) => {
            warn!(
                target: "tomcat_chat_diag",
                error = %e,
                phase = "apply_boundary_stale_after_precondition",
                "discarding stale preheat result without surfacing a user-actionable compaction error"
            );
            state.preheat.discard_cached_completed();
            false
        }
        Err(e) => {
            warn!("apply_boundary failed: {}", e);
            let _ = emitter.emit(AgentEvent::CompactionError {
                exhausted_after_retries: false,
                attempts: 0,
                error: e.to_string(),
                source: "apply".to_string(),
                ratio: Some(state.usage_ratio()),
            });
            false
        }
    }
}

/// Boundary 已成功改写历史，缓存前缀随之失效；现在清理大型工具结果没有额外缓存代价。
///
/// 这段逻辑只能从 `apply_and_emit_boundary` 的成功分支进入，保证时机②、时机⑤和
/// current-tail guard 三条路径的语义完全一致。
pub(crate) fn run_layer0_after_boundary(
    state: &mut ContextState,
    messages: &mut [crate::core::llm::ChatMessage],
    emitter: &ScopedEventEmitter,
    env: &BoundaryEnv<'_>,
    history_end: usize,
) {
    let l0 = run_layer0_cleanup(
        state,
        messages,
        env.config,
        env.work_dir,
        env.session_id,
        history_end,
    );
    for persisted in &l0.persisted {
        state.session_obs.tool_result_chars_persisted += persisted.original_chars;
    }
    for tool_call_id in &l0.evicted_tool_call_ids {
        env.read_file_state.invalidate_tool_call(tool_call_id);
    }

    let persist_tokens_freed = estimated_tokens_from_chars(l0.persist_chars_freed);
    let placeholder_tokens_freed = estimated_tokens_from_chars(l0.placeholder_chars_freed);
    if persist_tokens_freed == 0 && placeholder_tokens_freed == 0 {
        return;
    }

    state.session_obs.compaction_tokens_freed += persist_tokens_freed + placeholder_tokens_freed;
    let _ = emitter.emit(AgentEvent::Layer0ContextRelease {
        persist_tokens_freed,
        placeholder_tokens_freed,
    });
}

fn append_completed_summary_text(
    state: &ContextState,
    result: &CompactionResult,
) -> Result<(), AppError> {
    if state.transcript_path.as_os_str().is_empty() {
        return Ok(());
    }
    let marker_id = result
        .transcript_compaction_entry_id
        .as_deref()
        .ok_or_else(|| {
            AppError::internal(
                "completed preheat is missing its transcript marker id; refusing to leak summary as a message",
            )
        })?;
    let _guard = crate::infra::config::lock::acquire_config_lock(&state.transcript_path)?;
    // This O(1) tail check is sufficient while applying a result writes its body before any
    // later transcript append: polling consumes the result and moves Preheat to Idle, and
    // hydrate restores cache only when no body exists. If a future path can append between body
    // persistence and application, replace this with a for_id scan over the relevant transcript
    // slice before adding that path.
    if read_entries_tail(&state.transcript_path, 1)?
        .last()
        .is_some_and(|entry| {
            matches!(entry, TranscriptEntry::BranchSummaryText(body) if body.for_id == marker_id)
        })
    {
        return Ok(());
    }
    append_entry(
        &state.transcript_path,
        &TranscriptEntry::BranchSummaryText(BranchSummaryTextEntry {
            superseded: false,
            id: Some(format!("{marker_id}:text")),
            parent_id: Some(marker_id.to_string()),
            timestamp: chrono::Utc::now().to_rfc3339(),
            for_id: marker_id.to_string(),
            summary: result.summary_text.clone(),
        }),
    )
}
