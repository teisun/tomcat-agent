use super::super::{ToolExecCtx, AGENT_PLUGIN_ID};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::core::tools::primitive::{BashExecutionState, BashResult};
use crate::infra::events::{AgentEvent, ToolOutput};

const RECENT_OUTPUT_BYTES: u64 = 8 * 1024;

pub(in super::super) async fn handle_bash(
    ctx: &ToolExecCtx<'_>,
    args: &serde_json::Value,
) -> Result<String, String> {
    if args.get("timeout_ms").is_some() {
        return Err("bash: legacy timeout_ms is unsupported; use foreground_wait_ms".to_string());
    }
    let command = args["command"].as_str().unwrap_or("");
    let cwd = args["cwd"].as_str();
    let argv: Option<Vec<String>> = args.get("args").and_then(|v| v.as_array()).map(|arr| {
        arr.iter()
            .filter_map(|x| x.as_str().map(String::from))
            .collect()
    });
    let run_in_background = args
        .get("run_in_background")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let Some(registry) = ctx.bash_task_registry.as_ref() else {
        // No shared registry (isolated tests, or subagents without background support).
        // Background bash is genuinely unavailable, but foreground bash must still run: fall
        // back to the primitive executor's own tracked execution (no live output streaming).
        if run_in_background {
            return Err(super::background_unavailable::bash_background_unavailable(
                "bash",
                ctx.subagent_type,
            ));
        }
        return foreground_via_primitive(ctx, command, cwd, argv, args).await;
    };

    if run_in_background {
        return run_background_bash(ctx, registry, command, cwd, argv).await;
    }

    let wait_ms = args
        .get("foreground_wait_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or_else(|| registry.foreground_wait_ms())
        .clamp(
            crate::infra::MIN_TOOLS_BASH_FOREGROUND_WAIT_MS,
            crate::infra::MAX_TOOLS_BASH_FOREGROUND_WAIT_MS,
        );
    let started = Instant::now();
    let output_rx = registry.subscribe_output();
    let ticket = registry
        .spawn_tracked_with_preview_barrier(
            command.to_string(),
            argv,
            cwd.map(PathBuf::from),
            false,
        )
        .await
        .map_err(|e| e.to_string())?;
    // Caller cancellation must tear down the tracked process tree. The outer tool
    // dispatcher runs a `biased` `tokio::select!` on this same token and always wins the
    // race, dropping this future before an inline `cancel.cancelled()` arm could react. So
    // the stop is driven from a detached watcher: dropping this future detaches (does not
    // abort) the JoinHandle, letting the watcher survive to kill the process group. Every
    // path where the task must keep living (normal finish, promotion to background) aborts
    // the watcher first.
    let cancel_stop =
        spawn_cancel_stop(ctx.cancel.clone(), registry.clone(), ticket.task_id.clone());
    let bridge = spawn_live_output_bridge(ctx, args, registry.clone(), &ticket, output_rx);

    let wait = registry.wait_for_finish(&ticket.task_id);
    tokio::pin!(wait);
    let deadline = tokio::time::sleep(std::time::Duration::from_millis(wait_ms));
    tokio::pin!(deadline);
    tokio::select! {
        biased;
        result = &mut wait => {
            cancel_stop.abort();
            result.map_err(|e| e.to_string())?;
            let _ = bridge.await;
            foreground_result(registry, &ticket.task_id).await
        }
        _ = &mut deadline => {
            if !registry.promote_to_background(&ticket.task_id).map_err(|e| e.to_string())? {
                cancel_stop.abort();
                let _ = bridge.await;
                foreground_result(registry, &ticket.task_id).await
            } else {
                // Promoted to background: the task now outlives this tool call (and even a
                // later turn abort), so stop watching for cancellation and let the bridge
                // detach to keep streaming.
                cancel_stop.abort();
                let chunk = registry.tail_output_chunk(&ticket.task_id, RECENT_OUTPUT_BYTES)
                    .await.map_err(|e| e.to_string())?;
                Ok(serde_json::json!({
                    "state": "running_in_background",
                    "foregroundWaitExpired": true,
                    "elapsedMs": started.elapsed().as_millis() as u64,
                    "taskId": ticket.task_id,
                    "logPath": ticket.log_path,
                    "recentOutput": chunk.content,
                    "nextActions": [
                        {"when":"The result is needed now","tool":"task_output","arguments":{"task_id":ticket.task_id,"block":true,"wait_ms":30000}},
                        {"when":"Independent work remains","action":"Continue that work and wait for the background completion notification"},
                        {"when":"The task is stuck, wrong, or no longer useful","tool":"task_stop","arguments":{"task_id":ticket.task_id}}
                    ],
                    "message": "Foreground wait ended; the command is still running. Do not rerun it."
                }).to_string())
            }
        }
    }
}

/// Foreground bash without a shared registry: run it through the primitive executor (which does
/// its own gate/audit/tracked spawn) and render the result. Live streaming is unavailable here,
/// so the returned string is the terminal snapshot only.
async fn foreground_via_primitive(
    ctx: &ToolExecCtx<'_>,
    command: &str,
    cwd: Option<&str>,
    argv: Option<Vec<String>>,
    args: &serde_json::Value,
) -> Result<String, String> {
    let foreground_wait_ms = args.get("foreground_wait_ms").and_then(|v| v.as_u64());
    let result = ctx
        .primitive
        .execute_bash(
            command,
            cwd,
            AGENT_PLUGIN_ID,
            argv.as_deref(),
            foreground_wait_ms,
        )
        .await
        .map_err(|e| e.to_string())?;
    Ok(format_primitive_bash_result(result))
}

/// Render a primitive [`BashResult`] into the dispatcher's user-visible string.
///
/// This path is only used when the caller has no shared registry (isolated tests and similar
/// bare primitive call sites). If the foreground wait expires there, the command is stopped
/// locally and the combined log path is surfaced for follow-up debugging.
fn format_primitive_bash_result(result: BashResult) -> String {
    if matches!(result.state, BashExecutionState::RunningInBackground) {
        let mut out = result.recent_output;
        out.push_str(
            "\n(still running after the foreground wait; background tracking is unavailable in this context)",
        );
        return out;
    }
    let mut out = result.stdout;
    if !result.stderr.is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str("STDERR: ");
        out.push_str(&result.stderr);
    }
    if result.foreground_wait_expired {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str("Foreground wait expired; the command was stopped in this context.");
        if let Some(log_path) = result.log_path.as_deref() {
            out.push_str(" Full log: ");
            out.push_str(log_path);
        }
    }
    out.push_str(&format!("\n(exit code: {})", result.exit_code));
    out
}

/// Watch `cancel` and stop the tracked task if the caller cancels the turn.
///
/// This is deliberately a standalone detached task rather than a `tokio::select!` arm
/// inside [`handle_bash`]: the outer dispatcher selects on the *same* token with `biased;`
/// and therefore always drops the `handle_bash` future first on cancellation. Dropping the
/// future detaches (does not abort) this `JoinHandle`, so the watcher keeps running and can
/// still tear down the process group. Callers must `abort()` it on any path where the task
/// should keep living (normal completion or promotion to background).
fn spawn_cancel_stop(
    cancel: tokio_util::sync::CancellationToken,
    registry: std::sync::Arc<crate::core::tools::primitive::BashTaskRegistry>,
    task_id: String,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        cancel.cancelled().await;
        let _ = registry.stop(&task_id).await;
    })
}

async fn run_background_bash(
    ctx: &ToolExecCtx<'_>,
    registry: &std::sync::Arc<crate::core::tools::primitive::BashTaskRegistry>,
    command: &str,
    cwd: Option<&str>,
    argv: Option<Vec<String>>,
) -> Result<String, String> {
    let output_rx = registry.subscribe_output();
    let ticket = registry
        .spawn_tracked_with_preview_barrier(command.to_string(), argv, cwd.map(PathBuf::from), true)
        .await
        .map_err(|error| error.to_string())?;
    let bridge = spawn_live_output_bridge(
        ctx,
        &serde_json::json!({
            "command": command,
            "cwd": cwd,
            "run_in_background": true,
        }),
        registry.clone(),
        &ticket,
        output_rx,
    );
    // Explicit background returns immediately, but the detached bridge remains associated
    // with the original tool call until the task's drained completion marker arrives.
    drop(bridge);
    let mut value = serde_json::to_value(&ticket).unwrap_or(serde_json::Value::Null);
    if let serde_json::Value::Object(ref mut map) = value {
        map.insert(
            "state".into(),
            serde_json::Value::String("running_in_background".into()),
        );
    }
    Ok(value.to_string())
}

fn spawn_live_output_bridge(
    ctx: &ToolExecCtx<'_>,
    args: &serde_json::Value,
    registry: std::sync::Arc<crate::core::tools::primitive::BashTaskRegistry>,
    ticket: &crate::core::tools::primitive::BashTaskTicket,
    mut output_rx: tokio::sync::broadcast::Receiver<
        crate::core::tools::primitive::BashTaskOutputEvent,
    >,
) -> tokio::task::JoinHandle<()> {
    let emitter = ctx.event_emitter.cloned();
    let tool_call_id = ctx.tool_call_id.to_string();
    let args = args.clone();
    let task_id = ticket.task_id.clone();
    let log_path = ticket.log_path.clone();
    tokio::spawn(async move {
        let Some(emitter) = emitter else {
            let _ = registry.acknowledge_preview_flushed(&task_id);
            return;
        };
        let mut interval = tokio::time::interval(Duration::from_millis(250));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        interval.tick().await;
        let mut pending = Vec::<crate::core::tools::primitive::BashTaskOutputEvent>::new();
        let mut completed = false;
        let mut recover_snapshot = false;
        loop {
            tokio::select! {
                received = output_rx.recv() => match received {
                    Ok(event) if event.task_id == task_id => {
                        completed |= event.completed;
                        pending.push(event);
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        pending.clear();
                        recover_snapshot = true;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => completed = true,
                },
                _ = interval.tick() => {
                    if recover_snapshot {
                        if let Ok(snapshot) = registry.runtime_preview(&task_id) {
                            emit_snapshot(&emitter, &tool_call_id, &args, &task_id, &log_path, snapshot);
                        }
                        completed |= registry.get_info(&task_id).is_some_and(|info| {
                            !matches!(info.status, crate::core::tools::primitive::BashTaskStatus::Running)
                        });
                        pending.clear();
                        recover_snapshot = false;
                    } else {
                        emit_pending(&emitter, &tool_call_id, &args, &task_id, &log_path, &mut pending);
                    }
                    if completed && pending.is_empty() {
                        let _ = registry.acknowledge_preview_flushed(&task_id);
                        break;
                    }
                }
            }
        }
    })
}

fn emit_pending(
    emitter: &crate::infra::event_bus::ScopedEventEmitter,
    tool_call_id: &str,
    args: &serde_json::Value,
    task_id: &str,
    log_path: &str,
    pending: &mut Vec<crate::core::tools::primitive::BashTaskOutputEvent>,
) {
    if pending.is_empty() {
        return;
    }
    let mut output = String::new();
    let mut first = None;
    let mut last = None;
    let mut streams = Vec::new();
    for event in pending.drain(..) {
        if event.completed {
            continue;
        }
        first.get_or_insert(event.start_offset);
        last = Some((event.next_offset, event.sequence, event.truncated));
        if !event.output.is_empty() {
            output.push_str(&event.output);
            streams.push(match event.stream {
                crate::core::tools::primitive::BashOutputStream::Stdout => "stdout",
                crate::core::tools::primitive::BashOutputStream::Stderr => "stderr",
            });
        }
    }
    let Some((next_offset, sequence, mut truncated)) = last else {
        return;
    };
    if output.len() > 8 * 1024 {
        let mut start = output.len() - 8 * 1024;
        while !output.is_char_boundary(start) {
            start += 1;
        }
        output = output[start..].to_string();
        first = Some(next_offset.saturating_sub(output.len() as u64));
        truncated = true;
    }
    let stream = if streams.windows(2).any(|w| w[0] != w[1]) {
        "mixed"
    } else {
        streams.first().copied().unwrap_or("stdout")
    };
    let _ = emitter.emit(AgentEvent::ToolExecutionUpdate {
        tool_call_id: tool_call_id.to_string(),
        tool_name: "bash".to_string(),
        args: args.clone(),
        partial_result: ToolOutput(serde_json::json!({
            "stream": stream, "output": output,
            "startOffset": first.unwrap_or(next_offset), "nextOffset": next_offset,
            "sequence": sequence, "truncated": truncated,
            "logPath": log_path, "taskId": task_id,
        })),
    });
}

fn emit_snapshot(
    emitter: &crate::infra::event_bus::ScopedEventEmitter,
    tool_call_id: &str,
    args: &serde_json::Value,
    task_id: &str,
    log_path: &str,
    snapshot: crate::core::tools::primitive::BashRuntimePreview,
) {
    let (output, start_offset) = wire_preview_tail(snapshot.output, snapshot.start_offset);
    let _ = emitter.emit(AgentEvent::ToolExecutionUpdate {
        tool_call_id: tool_call_id.to_string(),
        tool_name: "bash".to_string(),
        args: args.clone(),
        partial_result: ToolOutput(serde_json::json!({
            "stream": "mixed", "output": output,
            "startOffset": start_offset, "nextOffset": snapshot.next_offset,
            "sequence": snapshot.sequence, "truncated": true,
            "logPath": log_path, "taskId": task_id,
        })),
    });
}

fn wire_preview_tail(output: String, start_offset: u64) -> (String, u64) {
    if output.len() <= 8 * 1024 {
        return (output, start_offset);
    }
    let mut start = output.len() - 8 * 1024;
    while !output.is_char_boundary(start) {
        start += 1;
    }
    let start_offset = start_offset.saturating_add(start as u64);
    (output[start..].to_string(), start_offset)
}

async fn foreground_result(
    registry: &crate::core::tools::primitive::BashTaskRegistry,
    task_id: &str,
) -> Result<String, String> {
    let chunk = registry
        .read_output(task_id, Some(0))
        .await
        .map_err(|e| e.to_string())?;
    let mut out = chunk.content;
    out.push_str(&format!("\n(exit code: {})", chunk.exit_code.unwrap_or(-1)));
    registry.remove_foreground(task_id);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::wire_preview_tail;

    #[test]
    fn lag_snapshot_tail_is_utf8_bounded_and_keeps_durable_offset() {
        let output = format!("{}€tail", "x".repeat(9 * 1024));
        let original_len = output.len();
        let (tail, start_offset) = wire_preview_tail(output, 100);
        assert!(tail.len() <= 8 * 1024);
        assert!(tail.is_char_boundary(0));
        assert_eq!(start_offset + tail.len() as u64, 100 + original_len as u64);
        assert!(tail.ends_with("€tail"));
    }

    /// Regression: the outer tool dispatcher's `biased` `tokio::select!` wins the
    /// cancellation race and drops the `handle_bash` future before any inline
    /// `cancel.cancelled()` arm can run. The detached cancel watcher must survive that drop
    /// and still tear down the tracked process group, otherwise a cancelled foreground
    /// command leaks a live `sleep 30` (here) for its full duration.
    #[cfg(unix)]
    #[tokio::test]
    async fn cancel_watcher_stops_tracked_task_after_future_drop() {
        use crate::core::tools::primitive::{BashTaskRegistry, BashTaskStatus};
        use std::sync::Arc;
        use tokio_util::sync::CancellationToken;

        let dir = tempfile::tempdir().expect("tempdir");
        let registry = Arc::new(BashTaskRegistry::new(dir.path().join("tool-results")));
        let ticket = registry
            .spawn_tracked(
                "sleep 30".to_string(),
                None::<Vec<String>>,
                None::<std::path::PathBuf>,
                false,
            )
            .await
            .expect("spawn tracked");

        let cancel = CancellationToken::new();
        let watcher =
            super::spawn_cancel_stop(cancel.clone(), registry.clone(), ticket.task_id.clone());
        // Mirror the dispatcher dropping `handle_bash`: dropping the JoinHandle detaches the
        // watcher rather than aborting it.
        drop(watcher);
        cancel.cancel();

        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            registry.wait_for_finish(&ticket.task_id),
        )
        .await
        .expect("cancellation must stop the process well before `sleep 30` finishes")
        .expect("wait_for_finish");

        let info = registry.get_info(&ticket.task_id).expect("task info");
        assert!(
            !matches!(info.status, BashTaskStatus::Running),
            "cancelled task must leave Running, got {:?}",
            info.status
        );
    }
}
