//! Layer 2 延迟应用：时机 ⑤ 非阻塞检查 + 时机 ② async 检查（含 ratio >= 0.98 同步等待）。

use std::path::Path;
use std::time::Duration;

use tracing::{info, warn};

use crate::core::compaction::preheat::PreheatOutcome;
use crate::core::compaction::run_layer0_cleanup;
use crate::core::session::manager::{
    compound_turn_id, estimated_tokens_from_chars, CompactionResult, ContextState,
};
use crate::core::session::transcript::{
    remove_branch_summary_entry_by_id, set_branch_summary_entry_is_boundary_true,
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

// ---------------------------------------------------------------------------
// check_after_reply — 时机 ⑤（非阻塞）
// ---------------------------------------------------------------------------

/// 在 reasoning loop 最终 assistant 回复后检查：
/// ratio >= 0.85 且预热已完成 → 立即应用 boundary switch。
/// 不阻塞——预热未完成则跳过。
#[must_use = "a successful boundary application may require the caller to rebuild its message snapshot"]
pub fn check_after_reply(
    state: &mut ContextState,
    emitter: &ScopedEventEmitter,
    env: &BoundaryEnv<'_>,
) -> bool {
    if state.usage_ratio() < 0.85 {
        return false;
    }
    let ratio_before = state.usage_ratio();

    match state.preheat.poll_result() {
        PreheatOutcome::Completed(result) => {
            apply_and_emit_boundary(state, result, ratio_before, false, emitter, env)
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
#[must_use = "a successful boundary application requires rebuilding the outgoing message snapshot"]
pub async fn check_before_request(
    state: &mut ContextState,
    emitter: &ScopedEventEmitter,
    env: &BoundaryEnv<'_>,
) -> bool {
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
                apply_and_emit_boundary(state, result, ratio_before, false, emitter, env)
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
                apply_and_emit_boundary(state, result, ratio_before, true, emitter, env)
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

pub(crate) fn apply_and_emit_boundary(
    state: &mut ContextState,
    result: CompactionResult,
    ratio_before: f64,
    was_sync_wait: bool,
    emitter: &ScopedEventEmitter,
    env: &BoundaryEnv<'_>,
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
            let _ = emitter.emit(AgentEvent::BoundarySwitched {
                ratio_before,
                ratio_after,
                covered_count,
                was_sync_wait,
                estimated_tokens_freed: saved,
            });

            run_layer0_after_boundary(state, emitter, env);
            true
        }
        Err(e @ AppError::ApplyBoundaryStale { .. }) => {
            warn!(
                error = %e,
                "apply_boundary stale: covered_end not in user_turns_list; removing branch_summary line, not restoring pending"
            );
            remove_stale_branch_summary_line(state, &result);
            state.preheat.discard_cached_completed();
            let _ = emitter.emit(AgentEvent::CompactionError {
                exhausted_after_retries: false,
                attempts: 0,
                error: e.to_string(),
                source: "apply".to_string(),
                ratio: Some(state.usage_ratio()),
            });
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
            state.preheat.restore_pending_result(result);
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
    emitter: &ScopedEventEmitter,
    env: &BoundaryEnv<'_>,
) {
    let l0 = run_layer0_cleanup(state, env.config, env.work_dir, env.session_id);
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
