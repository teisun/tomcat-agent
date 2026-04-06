//! Layer 2 延迟应用：时机 ⑤ 非阻塞检查 + 时机 ② async 检查（含 ratio >= 0.98 同步等待）。

use std::time::Duration;

use tracing::warn;

use crate::core::session::manager::{generate_entry_id, ContextState};
use crate::core::session::transcript::{append_entry, CompactionEntry, TranscriptEntry};
use crate::infra::error::AppError;

// ---------------------------------------------------------------------------
// check_after_reply — 时机 ⑤（非阻塞）
// ---------------------------------------------------------------------------

/// 在 reasoning loop 最终 assistant 回复后检查：
/// ratio >= 0.85 且预热已完成 → 立即应用 boundary switch。
/// 不阻塞——预热未完成则跳过。
pub fn check_after_reply(state: &mut ContextState) -> bool {
    if state.usage_ratio() < 0.85 {
        return false;
    }
    let finished = state
        .compaction_summary
        .as_ref()
        .map_or(false, |cs| cs.task_handle.is_finished());
    if !finished {
        return false;
    }
    match apply_boundary_switch(state) {
        Ok(true) => true,
        Ok(false) => false,
        Err(e) => {
            warn!("check_after_reply: apply_boundary_switch failed: {}", e);
            false
        }
    }
}

// ---------------------------------------------------------------------------
// check_before_request — 时机 ②（async）
// ---------------------------------------------------------------------------

/// 在发起下一次 LLM 请求前检查：
/// - ratio >= 0.70：已完成则切换
/// - ratio >= 0.98：未完成则 await（30s 超时）
/// 必须是 async fn（chat_loop 运行在 tokio runtime 中）。
pub async fn check_before_request(state: &mut ContextState) -> bool {
    let ratio = state.usage_ratio();

    if ratio < 0.70 {
        return false;
    }

    if let Some(ref cs) = state.compaction_summary {
        if cs.task_handle.is_finished() {
            return match apply_boundary_switch(state) {
                Ok(switched) => switched,
                Err(e) => {
                    warn!("check_before_request: apply failed: {}", e);
                    false
                }
            };
        }
    }

    if ratio >= 0.98 {
        if state.compaction_summary.is_some() {
            let pending = state.compaction_summary.take().unwrap();
            match tokio::time::timeout(Duration::from_secs(30), pending.task_handle).await {
                Ok(Ok(Ok(result))) => {
                    write_boundary_transcript(state, &result);
                    match state.apply_boundary(result) {
                        Ok(()) => return true,
                        Err(e) => {
                            warn!("check_before_request: apply_boundary failed after sync wait: {}", e);
                            return false;
                        }
                    }
                }
                Ok(Ok(Err(e))) => {
                    warn!("check_before_request: preheat task error: {}", e);
                }
                Ok(Err(e)) => {
                    warn!("check_before_request: preheat task panicked: {}", e);
                }
                Err(_) => {
                    warn!("check_before_request: preheat timed out after 30s, clearing");
                }
            }
        }
    }

    false
}

// ---------------------------------------------------------------------------
// apply_boundary_switch
// ---------------------------------------------------------------------------

/// take() CompactionSummary，处理 JoinHandle 结果，应用 boundary 切换，写 transcript。
fn apply_boundary_switch(state: &mut ContextState) -> Result<bool, AppError> {
    let is_finished = state
        .compaction_summary
        .as_ref()
        .map_or(false, |cs| cs.task_handle.is_finished());

    if !is_finished {
        return Ok(false);
    }

    let pending = state.compaction_summary.take().unwrap();
    let join_result = futures_util::FutureExt::now_or_never(pending.task_handle);

    match join_result {
        Some(Ok(Ok(result))) => {
            write_boundary_transcript(state, &result);
            state.apply_boundary(result)?;
            Ok(true)
        }
        Some(Ok(Err(e))) => {
            warn!("apply_boundary_switch: preheat task returned error: {}", e);
            Ok(false)
        }
        Some(Err(e)) => {
            warn!("apply_boundary_switch: preheat task panicked: {}", e);
            Ok(false)
        }
        None => Ok(false),
    }
}

/// 写入 is_boundary=true 的 transcript entry。
fn write_boundary_transcript(
    state: &ContextState,
    result: &crate::core::session::manager::CompactionResult,
) {
    if state.transcript_path.as_os_str().is_empty() {
        return;
    }
    let entry = TranscriptEntry::Compaction(CompactionEntry {
        id: Some(generate_entry_id()),
        parent_id: None,
        timestamp: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        summary: Some(result.summary_text.clone()),
        covered_start_id: Some(result.covered_start_id.clone()),
        covered_end_id: Some(result.covered_end_id.clone()),
        covered_count: Some(result.covered_count),
        is_boundary: Some(true),
    });
    let _ = append_entry(&state.transcript_path, &entry);
}

