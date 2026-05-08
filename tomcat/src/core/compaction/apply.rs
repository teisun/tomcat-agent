//! Layer 2 延迟应用：时机 ⑤ 非阻塞检查 + 时机 ② async 检查（含 ratio >= 0.98 同步等待）。

use std::time::Duration;

use tracing::{info, warn};

use crate::core::compaction::preheat::PreheatOutcome;
use crate::core::session::manager::{compound_turn_id, CompactionResult, ContextState};
use crate::core::session::transcript::{
    remove_branch_summary_entry_by_id, set_branch_summary_entry_is_boundary_true,
};
use crate::infra::error::AppError;
use crate::infra::event_bus::{EventBus, EventContext};
use crate::infra::events::AgentEvent;

// ---------------------------------------------------------------------------
// check_after_reply — 时机 ⑤（非阻塞）
// ---------------------------------------------------------------------------

/// 在 reasoning loop 最终 assistant 回复后检查：
/// ratio >= 0.85 且预热已完成 → 立即应用 boundary switch。
/// 不阻塞——预热未完成则跳过。
pub fn check_after_reply(state: &mut ContextState, event_bus: &dyn EventBus) -> bool {
    if state.usage_ratio() < 0.85 {
        return false;
    }
    let ratio_before = state.usage_ratio();

    match state.preheat.poll_result() {
        PreheatOutcome::Completed(result) => {
            apply_and_emit_boundary(state, result, ratio_before, false, event_bus)
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// check_before_request — 时机 ②（async）
// ---------------------------------------------------------------------------

/// 在发起下一次 LLM 请求前检查：
/// - ratio >= 0.70：已完成则切换
/// - ratio >= 0.98：未完成则 await（30s 超时）
pub async fn check_before_request(state: &mut ContextState, event_bus: &dyn EventBus) -> bool {
    let ratio = state.usage_ratio();
    let user_turns_len = state.turn_count();
    let preheat_finished = state.preheat.is_finished();
    let preheat_running = state.preheat.is_running();
    info!(
        target: "tomcat_chat_diag",
        phase = "timing2_check_before_request_entry",
        ratio,
        user_turns_len,
        preheat_finished,
        preheat_running
    );

    if ratio < 0.70 {
        info!(
            target: "tomcat_chat_diag",
            phase = "timing2_check_before_request_exit",
            path = "below_0_70",
            applied = false
        );
        return false;
    }

    let ratio_before = state.usage_ratio();

    if state.preheat.is_finished() {
        let applied = match state.preheat.poll_result() {
            PreheatOutcome::Completed(result) => {
                apply_and_emit_boundary(state, result, ratio_before, false, event_bus)
            }
            _ => false,
        };
        info!(
            target: "tomcat_chat_diag",
            phase = "timing2_check_before_request_exit",
            path = "finished_apply",
            applied
        );
        return applied;
    }

    if ratio >= 0.98 && state.preheat.is_running() {
        let applied = match state.preheat.await_result(Duration::from_secs(30)).await {
            PreheatOutcome::Completed(result) => {
                apply_and_emit_boundary(state, result, ratio_before, true, event_bus)
            }
            _ => false,
        };
        info!(
            target: "tomcat_chat_diag",
            phase = "timing2_check_before_request_exit",
            path = "await_0_98_apply",
            applied
        );
        return applied;
    }

    info!(
        target: "tomcat_chat_diag",
        phase = "timing2_check_before_request_exit",
        path = "no_op",
        applied = false
    );
    false
}

// ---------------------------------------------------------------------------
// apply_and_emit_boundary
// ---------------------------------------------------------------------------

fn apply_and_emit_boundary(
    state: &mut ContextState,
    result: CompactionResult,
    ratio_before: f64,
    was_sync_wait: bool,
    event_bus: &dyn EventBus,
) -> bool {
    let covered_count = result.covered_count;
    let saved = result.estimated_tokens_saved.unwrap_or(0);

    match state.apply_boundary(result.clone()) {
        Ok(()) => {
            // Only record boundary switch after it has successfully applied.
            write_boundary_transcript(state, &result);

            state.session_obs.compaction_tokens_freed += saved;
            state.session_obs.compaction_count =
                state.session_obs.compaction_count.saturating_add(1);

            let ratio_after = state.usage_ratio();
            emit_agent_event(
                event_bus,
                AgentEvent::BoundarySwitched {
                    ratio_before,
                    ratio_after,
                    covered_count,
                    was_sync_wait,
                    estimated_tokens_freed: saved,
                },
            );
            true
        }
        Err(e @ AppError::ApplyBoundaryStale { .. }) => {
            warn!(
                error = %e,
                "apply_boundary stale: covered_end not in user_turns_list; removing branch_summary line, not restoring pending"
            );
            remove_stale_branch_summary_line(state, &result);
            state.preheat.discard_cached_completed();
            emit_agent_event(
                event_bus,
                AgentEvent::CompactionError {
                    exhausted_after_retries: false,
                    attempts: 0,
                    error: e.to_string(),
                    source: "apply".to_string(),
                    ratio: Some(state.usage_ratio()),
                },
            );
            false
        }
        Err(e) => {
            warn!("apply_boundary failed: {}", e);
            emit_agent_event(
                event_bus,
                AgentEvent::CompactionError {
                    exhausted_after_retries: false,
                    attempts: 0,
                    error: e.to_string(),
                    source: "apply".to_string(),
                    ratio: Some(state.usage_ratio()),
                },
            );
            state.preheat.restore_pending_result(result);
            false
        }
    }
}

fn transcript_entry_id_for_stale_remove(result: &CompactionResult) -> String {
    result
        .transcript_compaction_entry_id
        .clone()
        .unwrap_or_else(|| compound_turn_id(&result.covered_start_id, &result.covered_end_id))
}

fn remove_stale_branch_summary_line(state: &ContextState, result: &CompactionResult) {
    if state.transcript_path.as_os_str().is_empty() {
        warn!("remove_stale_branch_summary_line: transcript path empty; skip");
        return;
    }
    let id = transcript_entry_id_for_stale_remove(result);
    if let Err(e) = remove_branch_summary_entry_by_id(&state.transcript_path, &id) {
        warn!(
            entry_id = %id,
            "remove_stale_branch_summary_line: failed (transcript may diverge until reload): {}",
            e
        );
    }
}

// ---------------------------------------------------------------------------
// write_boundary_transcript
// ---------------------------------------------------------------------------

fn write_boundary_transcript(state: &ContextState, result: &CompactionResult) {
    if state.transcript_path.as_os_str().is_empty() {
        return;
    }
    let Some(id) = result.transcript_compaction_entry_id.as_deref() else {
        warn!("write_boundary_transcript: missing transcript_compaction_entry_id; skip transcript update");
        return;
    };
    if let Err(e) = set_branch_summary_entry_is_boundary_true(&state.transcript_path, id) {
        warn!(
            "write_boundary_transcript: failed to set isBoundary for {}: {}",
            id, e
        );
    }
}

fn emit_agent_event(event_bus: &dyn EventBus, event: AgentEvent) {
    let payload = serde_json::to_value(&event).unwrap_or(serde_json::Value::Null);
    let event_name = payload
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let ctx = EventContext::new(event_name.clone(), payload);
    let _ = event_bus.emit_sync(&event_name, ctx);
}
