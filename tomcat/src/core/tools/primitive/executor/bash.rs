//! Bash adapter for callers that use `PrimitiveExecutor` directly.
//!
//! This path preserves the permission gate/audit preflight, then delegates process creation,
//! output draining, foreground yield, and process-group stopping to `BashTaskRegistry`.

use super::helpers::{grant_trigger_str, grant_type_str, permission_scope_str};
use super::DefaultPrimitiveExecutor;
use crate::core::permission::{GrantTrace, PermissionScope};
use crate::core::tools::primitive::{
    BashExecutionState, BashNextAction, BashResult, BashTaskRegistry, PrimitiveOperation,
};
use crate::infra::audit::{AuditPrimitiveOp, PrimitiveAuditEntry};
use crate::infra::error::AppError;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn resolve_foreground_wait_ms(executor: &DefaultPrimitiveExecutor, value: Option<u64>) -> u64 {
    value.unwrap_or(executor.bash_foreground_wait_ms).clamp(
        crate::infra::MIN_TOOLS_BASH_FOREGROUND_WAIT_MS,
        crate::infra::MAX_TOOLS_BASH_FOREGROUND_WAIT_MS,
    )
}

fn validate_bash_cwd(path: &Path, raw_cwd: &str) -> Result<(), AppError> {
    if !path.try_exists().map_err(AppError::Io)? {
        let mut msg = format!(
            "bash.cwd does not exist: {} (input: {:?})",
            path.display(),
            raw_cwd
        );
        if raw_cwd.contains('$') {
            msg.push_str(
                "; environment variables are not expanded here; pass an absolute path or ~/...",
            );
        }
        return Err(AppError::Primitive(msg));
    }
    if !path.is_dir() {
        return Err(AppError::Primitive(format!(
            "bash.cwd is not a directory: {} (input: {:?})",
            path.display(),
            raw_cwd
        )));
    }
    Ok(())
}

fn record_bash_failure(
    executor: &DefaultPrimitiveExecutor,
    audit_cmd: &str,
    plugin_id: &str,
    err: &AppError,
) {
    executor.audit.record_primitive(PrimitiveAuditEntry {
        operation: AuditPrimitiveOp::Bash,
        path_or_cmd: audit_cmd.to_string(),
        plugin_id: plugin_id.to_string(),
        user_approved: false,
        success: false,
        detail: Some(err.to_string()),
        ..Default::default()
    });
}

/// Resolve the working directory. An explicit cwd is gate-checked as a Read and validated for
/// existence/type; an omitted cwd defaults to `.` and is **not** gate-checked (mirrors the
/// shared [`BashTaskRegistry`] guard so the two entry points prompt identically). Any failure
/// records a bash audit entry before propagating.
async fn resolve_cwd(
    executor: &DefaultPrimitiveExecutor,
    cwd: Option<&str>,
    audit_cmd: &str,
    plugin_id: &str,
) -> Result<(PathBuf, Option<String>), AppError> {
    let raw_cwd = cwd.filter(|v| !v.trim().is_empty()).map(str::to_string);
    let Some(raw) = raw_cwd.as_deref() else {
        return Ok((PathBuf::from("."), None));
    };
    let path = match executor
        .gate_check_path(PrimitiveOperation::Read, raw, plugin_id)
        .await
    {
        Ok((path, _, _)) => path,
        Err(err) => {
            record_bash_failure(executor, audit_cmd, plugin_id, &err);
            return Err(err);
        }
    };
    if let Err(err) = validate_bash_cwd(&path, raw) {
        record_bash_failure(executor, audit_cmd, plugin_id, &err);
        return Err(err);
    }
    Ok((path, raw_cwd))
}

/// AST guard + explicit-path preflight + bash policy gate. Returns the resolved `(scope, grant)`
/// so the success audit can record the same `permission_scope` / `grant_type` / `grant_trigger`
/// as the shared registry guard.
async fn preflight(
    executor: &DefaultPrimitiveExecutor,
    audit_cmd: &str,
    cwd: &Path,
    plugin_id: &str,
) -> Result<(PermissionScope, GrantTrace), AppError> {
    executor
        .bash_ast
        .check(audit_cmd)
        .map_err(|e| AppError::Primitive(e.to_string()))?;
    for raw in crate::core::permission::bash_parser::extract_paths(audit_cmd) {
        let candidate = if raw.starts_with("./") || raw.starts_with("../") {
            cwd.join(&raw)
        } else {
            PathBuf::from(&raw)
        };
        executor
            .gate_check_path(
                PrimitiveOperation::Bash,
                &candidate.to_string_lossy(),
                plugin_id,
            )
            .await?;
    }
    executor.gate_check_bash(audit_cmd, plugin_id).await
}

pub(super) async fn execute_bash_impl(
    executor: &DefaultPrimitiveExecutor,
    command: &str,
    cwd: Option<&str>,
    plugin_id: &str,
    argv: Option<&[String]>,
    foreground_wait_ms: Option<u64>,
) -> Result<BashResult, AppError> {
    let started = Instant::now();
    let argv = argv.filter(|a| !a.is_empty()).map(<[String]>::to_vec);
    let audit_cmd = match argv.as_deref() {
        None => command.to_string(),
        Some(args) => format!("{} {}", command, args.join(" ")),
    };
    let (cwd, raw_cwd) = resolve_cwd(executor, cwd, &audit_cmd, plugin_id).await?;
    let (bash_scope, bash_grant) = match preflight(executor, &audit_cmd, &cwd, plugin_id).await {
        Ok(scope_grant) => scope_grant,
        Err(error) => {
            record_bash_failure(executor, &audit_cmd, plugin_id, &error);
            return Err(error);
        }
    };

    let persist_dir = executor
        .bash_persist_dir
        .clone()
        .unwrap_or_else(|| std::env::temp_dir().join("tomcat-bash-tool-results"));
    let shared_registry = executor.bash_task_registry.clone();
    let registry = shared_registry.clone().unwrap_or_else(|| {
        Arc::new(
            BashTaskRegistry::new(persist_dir)
                .with_foreground_wait_ms(executor.bash_foreground_wait_ms),
        )
    });
    let ticket = registry
        .spawn_tracked_unchecked(command.to_string(), argv, Some(cwd.clone()), false)
        .await
        .map_err(|e| {
            AppError::Primitive(format!(
                "bash spawn failed (cwd={}, input={:?}): {}",
                cwd.display(),
                raw_cwd.as_deref().unwrap_or("<inherited>"),
                e
            ))
        })?;
    let wait_ms = resolve_foreground_wait_ms(executor, foreground_wait_ms);
    let finished = tokio::select! {
        result = registry.wait_for_finish(&ticket.task_id) => { result?; true }
        _ = tokio::time::sleep(Duration::from_millis(wait_ms)) => false,
    };

    let result = if finished {
        finalize_foreground_result(
            executor,
            &registry,
            &ticket.task_id,
            &ticket.log_path,
            started,
            false,
        )
        .await?
    } else if shared_registry.is_some() {
        if registry.promote_to_background(&ticket.task_id)? {
            let chunk = registry
                .tail_output_chunk(&ticket.task_id, 8 * 1024)
                .await?;
            BashResult {
                state: BashExecutionState::RunningInBackground,
                foreground_wait_expired: true,
                elapsed_ms: started.elapsed().as_millis() as u64,
                task_id: Some(ticket.task_id.clone()),
                log_path: Some(ticket.log_path.clone()),
                recent_output: chunk.content,
                next_actions: vec![
                    BashNextAction {
                        when: "The result is needed now".into(),
                        tool: Some("task_output".into()),
                        arguments: Some(
                            serde_json::json!({"task_id": ticket.task_id, "block": true, "wait_ms": 30000}),
                        ),
                        action: None,
                    },
                    BashNextAction {
                        when: "The task is stuck or no longer useful".into(),
                        tool: Some("task_stop".into()),
                        arguments: Some(serde_json::json!({"task_id": ticket.task_id})),
                        action: None,
                    },
                ],
                ..Default::default()
            }
        } else {
            finalize_foreground_result(
                executor,
                &registry,
                &ticket.task_id,
                &ticket.log_path,
                started,
                true,
            )
            .await?
        }
    } else {
        finish_expired_foreground(
            executor,
            &registry,
            &ticket.task_id,
            &ticket.log_path,
            started,
        )
        .await?
    };
    executor.audit.record_primitive(PrimitiveAuditEntry {
        operation: AuditPrimitiveOp::Bash,
        path_or_cmd: audit_cmd,
        plugin_id: plugin_id.into(),
        user_approved: true,
        success: result.exit_code == 0 || result.foreground_wait_expired,
        detail: Some(format!(
            "state={:?} foreground_wait_expired={} elapsed_ms={}",
            result.state, result.foreground_wait_expired, result.elapsed_ms
        )),
        permission_scope: Some(permission_scope_str(bash_scope)),
        grant_type: Some(grant_type_str(bash_grant.grant_type)),
        grant_trigger: Some(grant_trigger_str(bash_grant.trigger)),
    });
    Ok(result)
}

async fn finish_expired_foreground(
    executor: &DefaultPrimitiveExecutor,
    registry: &Arc<BashTaskRegistry>,
    task_id: &str,
    log_path: &str,
    started: Instant,
) -> Result<BashResult, AppError> {
    let _ = registry.stop(task_id).await;
    finalize_foreground_result(executor, registry, task_id, log_path, started, true).await
}

async fn finalize_foreground_result(
    executor: &DefaultPrimitiveExecutor,
    registry: &Arc<BashTaskRegistry>,
    task_id: &str,
    log_path: &str,
    started: Instant,
    foreground_wait_expired: bool,
) -> Result<BashResult, AppError> {
    registry.wait_for_finish(task_id).await?;
    let chunk = registry.read_output(task_id, Some(0)).await?;
    let final_state = match registry.get_info(task_id).map(|info| info.status) {
        Some(crate::core::tools::primitive::BashTaskStatus::Stopped) => BashExecutionState::Stopped,
        Some(crate::core::tools::primitive::BashTaskStatus::Finished { .. })
        | Some(crate::core::tools::primitive::BashTaskStatus::Running)
        | Some(crate::core::tools::primitive::BashTaskStatus::DrainingOutput)
        | None => BashExecutionState::Finished,
    };
    registry.remove_foreground(task_id);
    let (raw_stdout, raw_stderr) = split_log_streams(&chunk.content);
    let stdout_outcome = super::output_accum::accumulate_with_persist(
        &raw_stdout,
        executor.bash_max_output_chars,
        executor.bash_persist_dir.as_deref(),
        "bash-stdout",
    );
    let stderr_outcome = super::output_accum::accumulate_with_persist(
        &raw_stderr,
        executor.bash_max_output_chars,
        executor.bash_persist_dir.as_deref(),
        "bash-stderr",
    );
    let truncated = stdout_outcome.truncated || stderr_outcome.truncated;
    let persisted_output_path = stdout_outcome
        .persisted_path
        .or(stderr_outcome.persisted_path)
        .map(|path| path.display().to_string());
    Ok(BashResult {
        stdout: stdout_outcome.text,
        stderr: stderr_outcome.text,
        exit_code: chunk.exit_code.unwrap_or(-1),
        state: final_state,
        foreground_wait_expired,
        elapsed_ms: started.elapsed().as_millis() as u64,
        log_path: Some(log_path.to_string()),
        truncated,
        persisted_output_path,
        ..Default::default()
    })
}

fn split_log_streams(log: &str) -> (String, String) {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    for line in log.split_inclusive('\n') {
        if let Some(value) = line.strip_prefix("STDERR: ") {
            stderr.push(value);
        } else {
            stdout.push(line);
        }
    }
    (stdout.concat(), stderr.concat())
}

#[cfg(test)]
mod tests {
    #[test]
    fn foreground_wait_clamps_to_contract() {
        struct Values;
        assert_eq!(crate::infra::MIN_TOOLS_BASH_FOREGROUND_WAIT_MS, 8_000);
        assert_eq!(crate::infra::MAX_TOOLS_BASH_FOREGROUND_WAIT_MS, 16_000);
        let _ = Values;
    }
}
