use std::sync::Arc;

use crate::core::agent_loop::types::{BackgroundCompletionRoutes, CompletionRoute, ToolCallInfo};
use crate::core::session::manager::INTERRUPTED_TOOL_RESULT_TEXT;
use crate::core::tools::primitive::BashTaskRegistry;
use crate::infra::event_bus::ScopedEventEmitter;
use crate::infra::events::{AgentEvent, ToolOutput};

use super::super::ToolExecCtx;

const TASK_OUTPUT_BLOCK_DEFAULT_WAIT_MS: u64 = 5_000;
const TASK_OUTPUT_BLOCK_MIN_WAIT_MS: u64 = 5_000;
const TASK_OUTPUT_BLOCK_MAX_WAIT_MS: u64 = 600_000;
const TASK_OUTPUT_TICK_MS: u64 = 500;
const TASK_OUTPUT_WAIT_TAIL_MAX_BYTES: u64 = 4_096;

fn clamp_block_wait_ms(wait_ms_raw: Option<u64>) -> u64 {
    match wait_ms_raw {
        Some(0) => TASK_OUTPUT_BLOCK_MIN_WAIT_MS,
        Some(v) => v.clamp(TASK_OUTPUT_BLOCK_MIN_WAIT_MS, TASK_OUTPUT_BLOCK_MAX_WAIT_MS),
        None => TASK_OUTPUT_BLOCK_DEFAULT_WAIT_MS,
    }
}

pub(in super::super) async fn handle_task_output(
    ctx: &ToolExecCtx<'_>,
    tc: &ToolCallInfo,
    args: &serde_json::Value,
) -> Result<String, String> {
    if args.get("timeout_ms").is_some() {
        return Err("task_output: legacy timeout_ms is unsupported; use wait_ms".to_string());
    }
    let Some(registry) = ctx.bash_task_registry.as_ref() else {
        return Err(super::background_unavailable::bash_background_unavailable(
            "task_output",
            ctx.subagent_type,
        ));
    };
    let task_id = args
        .get("task_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "task_output: missing required field `task_id`".to_string())?;
    let since = args.get("since").and_then(|v| v.as_u64());
    let block_param = args.get("block").and_then(|v| v.as_bool()).unwrap_or(false);
    let wait_ms_raw = args.get("wait_ms").and_then(|v| v.as_u64());

    let wait_ms = clamp_block_wait_ms(wait_ms_raw);
    let block = block_param;

    if !block {
        return registry
            .read_output(task_id, since)
            .await
            .map(|c| serde_json::to_string(&c).unwrap_or_else(|_| "{}".to_string()))
            .map_err(|e| e.to_string());
    }

    handle_task_output_blocking(
        registry,
        task_id,
        since,
        wait_ms,
        ctx.cancel,
        tc,
        ctx.event_emitter,
        ctx.completion_routes,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn handle_task_output_blocking(
    registry: &Arc<BashTaskRegistry>,
    task_id: &str,
    since: Option<u64>,
    wait_ms: u64,
    cancel: &tokio_util::sync::CancellationToken,
    tc: &ToolCallInfo,
    event_emitter: Option<&ScopedEventEmitter>,
    completion_routes: Option<&BackgroundCompletionRoutes>,
) -> Result<String, String> {
    use std::time::Instant;
    use tokio::time::{sleep_until, Duration, Instant as TokioInstant};

    let since_value = since.unwrap_or(0);

    let mut already_delivered_by_lifecycle = false;
    if let Some(routes) = completion_routes {
        let mut g = routes.lock();
        match g.get(task_id).copied() {
            Some(CompletionRoute::Delivered) => {
                already_delivered_by_lifecycle = true;
            }
            _ => {
                g.insert(task_id.to_string(), CompletionRoute::ToolWillDeliver);
            }
        }
    }
    if already_delivered_by_lifecycle {
        return finish_blocking_with(registry, task_id, since_value, BlockingWakeKind::Finished)
            .await
            .map(|(text, _)| text);
    }

    let started = Instant::now();
    let deadline = TokioInstant::now() + Duration::from_millis(wait_ms);

    let mut ticker = tokio::time::interval(Duration::from_millis(TASK_OUTPUT_TICK_MS));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    ticker.tick().await;
    emit_task_output_update(
        event_emitter,
        tc,
        task_id,
        since_value,
        wait_ms,
        wait_ms.saturating_sub(elapsed_ms(started)),
        "waiting_for_output",
        None,
    );

    let wake_kind = loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                if let Some(routes) = completion_routes {
                    routes.lock().remove(task_id);
                }
                // This is a terminal tool result, not a tool failure. Returning the shared
                // interruption sentinel lets the dispatcher persist a complete assistant/tool
                // pair before the enclosing turn observes cancellation and becomes aborted.
                return Err(INTERRUPTED_TOOL_RESULT_TEXT.to_string());
            }
            wait = registry.wait_for_finish(task_id) => {
                match wait {
                    Ok(()) => break BlockingWakeKind::Finished,
                    Err(e) => {
                        if let Some(routes) = completion_routes {
                            routes.lock().remove(task_id);
                        }
                        return Err(e.to_string());
                    }
                }
            }
            _ = sleep_until(deadline) => {
                break BlockingWakeKind::WaitWindowElapsed;
            }
            _ = ticker.tick() => {
                let remaining = wait_ms.saturating_sub(elapsed_ms(started));
                emit_task_output_update(
                        event_emitter,
                    tc,
                    task_id,
                    since_value,
                    wait_ms,
                    remaining,
                    "waiting_for_output",
                    None,
                );
            }
        }
    };

    let (text, delivered) = finish_blocking_with(registry, task_id, since_value, wake_kind).await?;

    if let Some(routes) = completion_routes {
        let mut g = routes.lock();
        match wake_kind {
            BlockingWakeKind::Finished => {
                g.insert(task_id.to_string(), CompletionRoute::Delivered);
            }
            BlockingWakeKind::WaitWindowElapsed => {
                if delivered {
                    g.insert(task_id.to_string(), CompletionRoute::Delivered);
                } else {
                    g.remove(task_id);
                }
            }
        }
    }

    Ok(text)
}

#[derive(Debug, Clone, Copy)]
enum BlockingWakeKind {
    Finished,
    WaitWindowElapsed,
}

async fn finish_blocking_with(
    registry: &Arc<BashTaskRegistry>,
    task_id: &str,
    since_value: u64,
    wake: BlockingWakeKind,
) -> Result<(String, bool), String> {
    let chunk = match wake {
        BlockingWakeKind::Finished => registry
            .read_output(task_id, Some(since_value))
            .await
            .map_err(|e| e.to_string())?,
        BlockingWakeKind::WaitWindowElapsed => {
            let snapshot = registry
                .read_output_tail(task_id, Some(since_value), TASK_OUTPUT_WAIT_TAIL_MAX_BYTES)
                .await
                .map_err(|e| e.to_string())?;
            if snapshot.finished {
                registry
                    .read_output(task_id, Some(since_value))
                    .await
                    .map_err(|e| e.to_string())?
            } else {
                snapshot
            }
        }
    };
    let wake_reason = match wake {
        BlockingWakeKind::Finished => "finished",
        BlockingWakeKind::WaitWindowElapsed => {
            if chunk.finished {
                "finished"
            } else {
                "wait_window_elapsed"
            }
        }
    };
    let mut value = serde_json::to_value(&chunk).unwrap_or(serde_json::Value::Null);
    if let serde_json::Value::Object(ref mut map) = value {
        map.insert(
            "wakeReason".to_string(),
            serde_json::Value::String(wake_reason.to_string()),
        );
    }
    let delivered = matches!(wake, BlockingWakeKind::Finished)
        || matches!(wake, BlockingWakeKind::WaitWindowElapsed) && chunk.finished;
    Ok((value.to_string(), delivered))
}

fn elapsed_ms(started: std::time::Instant) -> u64 {
    let dur = started.elapsed();
    dur.as_secs() * 1000 + (dur.subsec_millis() as u64)
}

#[allow(clippy::too_many_arguments)]
fn emit_task_output_update(
    event_emitter: Option<&ScopedEventEmitter>,
    tc: &ToolCallInfo,
    task_id: &str,
    since: u64,
    wait_ms: u64,
    remaining_ms: u64,
    phase: &str,
    wake_reason: Option<&str>,
) {
    let Some(emitter) = event_emitter else {
        return;
    };
    let mut partial = serde_json::Map::new();
    partial.insert("phase".to_string(), serde_json::Value::String(phase.into()));
    if let Some(wr) = wake_reason {
        partial.insert(
            "wakeReason".to_string(),
            serde_json::Value::String(wr.into()),
        );
    }
    partial.insert(
        "taskId".to_string(),
        serde_json::Value::String(task_id.into()),
    );
    partial.insert(
        "since".to_string(),
        serde_json::Value::Number(serde_json::Number::from(since)),
    );
    partial.insert(
        "waitMs".to_string(),
        serde_json::Value::Number(serde_json::Number::from(wait_ms)),
    );
    partial.insert(
        "remainingMs".to_string(),
        serde_json::Value::Number(serde_json::Number::from(remaining_ms)),
    );
    let args: serde_json::Value =
        serde_json::from_str(&tc.arguments).unwrap_or(serde_json::Value::Null);
    let event = AgentEvent::ToolExecutionUpdate {
        tool_call_id: tc.id.clone(),
        tool_name: tc.name.clone(),
        args,
        partial_result: ToolOutput(serde_json::Value::Object(partial)),
    };
    let _ = emitter.emit(event);
}

#[cfg(test)]
#[path = "tests/task_output_test.rs"]
mod tests;
