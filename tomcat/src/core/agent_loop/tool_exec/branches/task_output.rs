use std::sync::Arc;

use crate::core::agent_loop::types::{BackgroundCompletionRoutes, CompletionRoute, ToolCallInfo};
use crate::core::tools::primitive::{BashTaskOutputChunk, BashTaskRegistry, WakeReason};
use crate::infra::event_bus::EventBus;
use crate::infra::events::{AgentEvent, ToolOutput};

use super::super::ToolExecCtx;

const TASK_OUTPUT_BLOCK_DEFAULT_TIMEOUT_MS: u64 = 5_000;
const TASK_OUTPUT_BLOCK_MAX_TIMEOUT_MS: u64 = 30_000;
const TASK_OUTPUT_TICK_MS: u64 = 500;

pub(in super::super) async fn handle_task_output(
    ctx: &ToolExecCtx<'_>,
    tc: &ToolCallInfo,
    args: &serde_json::Value,
) -> Result<String, String> {
    let Some(registry) = ctx.bash_task_registry.as_ref() else {
        return Err("task_output 未启用：未注入 BashTaskRegistry".to_string());
    };
    let task_id = args
        .get("task_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "task_output 缺少 task_id".to_string())?;
    let since = args.get("since").and_then(|v| v.as_u64());
    let block_param = args.get("block").and_then(|v| v.as_bool()).unwrap_or(false);
    let timeout_ms_raw = args.get("timeout_ms").and_then(|v| v.as_u64());

    let timeout_ms = match timeout_ms_raw {
        Some(0) => 0,
        Some(v) => v.min(TASK_OUTPUT_BLOCK_MAX_TIMEOUT_MS),
        None => TASK_OUTPUT_BLOCK_DEFAULT_TIMEOUT_MS,
    };
    let block = block_param && timeout_ms > 0;

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
        timeout_ms,
        ctx.cancel,
        tc,
        ctx.event_bus,
        ctx.completion_routes,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn handle_task_output_blocking(
    registry: &Arc<BashTaskRegistry>,
    task_id: &str,
    since: Option<u64>,
    timeout_ms: u64,
    cancel: &tokio_util::sync::CancellationToken,
    tc: &ToolCallInfo,
    event_bus: Option<&Arc<dyn EventBus>>,
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
    let deadline = TokioInstant::now() + Duration::from_millis(timeout_ms);

    let mut ticker = tokio::time::interval(Duration::from_millis(TASK_OUTPUT_TICK_MS));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    ticker.tick().await;
    emit_task_output_update(
        event_bus,
        tc,
        task_id,
        since_value,
        timeout_ms,
        timeout_ms.saturating_sub(elapsed_ms(started)),
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
                return Err("task_output(block=true) 已被取消".to_string());
            }
            wait = registry.wait_for_change(task_id, since) => {
                match wait {
                    Ok(WakeReason::NewOutput) => break BlockingWakeKind::NewOutput,
                    Ok(WakeReason::Finished) => break BlockingWakeKind::Finished,
                    Err(e) => {
                        if let Some(routes) = completion_routes {
                            routes.lock().remove(task_id);
                        }
                        return Err(e.to_string());
                    }
                }
            }
            _ = sleep_until(deadline) => {
                break BlockingWakeKind::Timeout;
            }
            _ = ticker.tick() => {
                let remaining = timeout_ms.saturating_sub(elapsed_ms(started));
                emit_task_output_update(
                    event_bus,
                    tc,
                    task_id,
                    since_value,
                    timeout_ms,
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
            BlockingWakeKind::Timeout => {
                g.remove(task_id);
            }
            BlockingWakeKind::NewOutput => {
                if delivered {
                    g.insert(task_id.to_string(), CompletionRoute::Delivered);
                }
            }
        }
    }

    Ok(text)
}

#[derive(Debug, Clone, Copy)]
enum BlockingWakeKind {
    NewOutput,
    Finished,
    Timeout,
}

async fn finish_blocking_with(
    registry: &Arc<BashTaskRegistry>,
    task_id: &str,
    since_value: u64,
    wake: BlockingWakeKind,
) -> Result<(String, bool), String> {
    let chunk = match wake {
        BlockingWakeKind::Finished | BlockingWakeKind::NewOutput => registry
            .read_output(task_id, Some(since_value))
            .await
            .map_err(|e| e.to_string())?,
        BlockingWakeKind::Timeout => BashTaskOutputChunk {
            task_id: task_id.to_string(),
            content: String::new(),
            start_offset: since_value,
            next_offset: since_value,
            finished: false,
            exit_code: None,
        },
    };
    let wake_reason = match wake {
        BlockingWakeKind::Finished => "finished",
        BlockingWakeKind::NewOutput => {
            if chunk.finished {
                "finished"
            } else {
                "new_output"
            }
        }
        BlockingWakeKind::Timeout => "timeout",
    };
    let mut value = serde_json::to_value(&chunk).unwrap_or(serde_json::Value::Null);
    if let serde_json::Value::Object(ref mut map) = value {
        map.insert(
            "wakeReason".to_string(),
            serde_json::Value::String(wake_reason.to_string()),
        );
    }
    let delivered = matches!(wake, BlockingWakeKind::Finished)
        || matches!(wake, BlockingWakeKind::NewOutput) && chunk.finished;
    Ok((value.to_string(), delivered))
}

fn elapsed_ms(started: std::time::Instant) -> u64 {
    let dur = started.elapsed();
    dur.as_secs() * 1000 + (dur.subsec_millis() as u64)
}

#[allow(clippy::too_many_arguments)]
fn emit_task_output_update(
    event_bus: Option<&Arc<dyn EventBus>>,
    tc: &ToolCallInfo,
    task_id: &str,
    since: u64,
    timeout_ms: u64,
    remaining_ms: u64,
    phase: &str,
    wake_reason: Option<&str>,
) {
    let Some(bus) = event_bus else {
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
        "timeoutMs".to_string(),
        serde_json::Value::Number(serde_json::Number::from(timeout_ms)),
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
    let payload = serde_json::to_value(&event).unwrap_or(serde_json::Value::Null);
    let event_name = payload
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("tool_execution_update")
        .to_string();
    let ctx = crate::infra::event_bus::EventContext::new(event_name.clone(), payload);
    let _ = bus.emit_sync(&event_name, ctx);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn finish_blocking_with_new_output_and_finished_marks_delivered() {
        use crate::core::tools::primitive::BashTaskRegistry;

        let dir = tempfile::tempdir().expect("tempdir");
        let registry = Arc::new(BashTaskRegistry::new(dir.path().join("tool-results")));
        let ticket = registry
            .spawn("echo done; exit 0".to_string(), None, None)
            .await
            .expect("spawn");

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let (text, delivered) = finish_blocking_with(
            &registry,
            &ticket.task_id,
            0,
            BlockingWakeKind::NewOutput,
        )
        .await
        .expect("finish_blocking_with");
        let chunk: serde_json::Value = serde_json::from_str(&text).expect("valid json");

        assert_eq!(chunk["wakeReason"], serde_json::Value::String("finished".into()));
        assert_eq!(chunk["finished"], serde_json::Value::Bool(true));
        assert!(delivered, "NewOutput + finished must claim Delivered");
    }
}
