use std::path::PathBuf;

use chrono::Utc;
use serde_json::json;
use tracing::warn;

use crate::api::chat::ChatContext;
use crate::core::{
    CheckpointId, CheckpointKind, CheckpointRecordRequest, ListOptions, RestoreOptions,
    TranscriptEntry,
};
use crate::infra::HostcallAuditEntry;

use super::cmd_ckpt::checkpoint_kind_label;
use super::parse::ChatCommandOutcome;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RestorePathPlan {
    pub paths: Vec<PathBuf>,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RestoreConflict {
    pub session_id: String,
    pub paths: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
pub(crate) struct RestoreCoreReport {
    pub changed_paths: Vec<String>,
    pub dry_run: bool,
    pub meta: crate::core::CheckpointMeta,
    pub restored_paths: Vec<String>,
    pub revert_files: bool,
    pub reloaded_plan_id: Option<String>,
    pub summary: Option<String>,
    pub transcript_truncated: bool,
    pub warnings: Vec<String>,
}

pub(crate) fn run(
    ctx: &ChatContext,
    checkpoint_id: String,
    paths: Vec<PathBuf>,
    dry_run: bool,
) -> ChatCommandOutcome {
    let checkpoint_id = CheckpointId::new(checkpoint_id);
    let report = match restore_core_with_paths(ctx, checkpoint_id.clone(), &paths, true, dry_run) {
        Ok(report) => report,
        Err(message) => {
            println!("{message}");
            return ChatCommandOutcome::Handled;
        }
    };

    for warning in &report.warnings {
        warn!(checkpoint_id = %checkpoint_id, "{warning}");
        println!("警告：{warning}");
    }

    if report.dry_run {
        println!("dry-run: {}", checkpoint_id);
        if let Some(summary) = report.summary {
            print!("{}", summary);
        } else {
            println!("当前工作区与目标 checkpoint 无差异。");
        }
        return ChatCommandOutcome::Handled;
    }

    if report.restored_paths.is_empty() {
        println!("已恢复 checkpoint {}。", checkpoint_id);
    } else {
        println!(
            "已恢复 checkpoint {}：{}",
            checkpoint_id,
            report.restored_paths.join(", ")
        );
    }
    if let Some(plan_id) = report.reloaded_plan_id {
        println!("plan_runtime 已对齐磁盘：EXEC plan_id={plan_id}");
    }
    ChatCommandOutcome::Handled
}

pub(crate) fn restore_core(
    ctx: &ChatContext,
    checkpoint_id: CheckpointId,
    revert_files: bool,
    dry_run: bool,
) -> Result<RestoreCoreReport, String> {
    restore_core_with_paths(ctx, checkpoint_id, &[], revert_files, dry_run)
}

fn restore_core_with_paths(
    ctx: &ChatContext,
    checkpoint_id: CheckpointId,
    requested_paths: &[PathBuf],
    revert_files: bool,
    dry_run: bool,
) -> Result<RestoreCoreReport, String> {
    let meta = match ctx.scope_services.checkpoint_store.show(&checkpoint_id) {
        Ok(Some(meta)) => meta,
        Ok(None) => {
            record_restore_audit(ctx, false, format!("checkpoint missing: {checkpoint_id}"));
            return Err(format!("未找到 checkpoint: {checkpoint_id}"));
        }
        Err(err) => {
            record_restore_audit(ctx, false, format!("show failed: {err}"));
            return Err(format!("读取 checkpoint 失败：{err}"));
        }
    };

    let current_session_id = match ctx.session_runtime.session.current_session_id() {
        Ok(Some(session_id)) => session_id,
        Ok(None) => {
            record_restore_audit(ctx, false, "restore missing current session".to_string());
            return Err("当前无活动会话，无法执行 restore".to_string());
        }
        Err(err) => {
            record_restore_audit(ctx, false, format!("current_session_id failed: {err}"));
            return Err(format!("读取当前会话失败：{err}"));
        }
    };
    if meta.session_id != current_session_id {
        record_restore_audit(
            ctx,
            false,
            format!(
                "checkpoint session mismatch: checkpoint={checkpoint_id} checkpoint_session={} current_session={current_session_id}",
                meta.session_id
            ),
        );
        return Err("checkpoint 不属于当前会话，不能跨会话 restore".to_string());
    }

    let note_paths = checkpoint_note_paths(meta.notes.as_ref());
    let restore_plan = if revert_files {
        Some(effective_restore_paths(
            ctx,
            &checkpoint_id,
            &meta,
            requested_paths,
        ))
    } else {
        None
    };
    let mut warnings = Vec::new();
    if let Some(restore_plan) = restore_plan.as_ref() {
        warnings = collect_restore_warnings(ctx, &checkpoint_id, &restore_plan.paths);
        if let Some(message) = restore_plan.warning.as_deref() {
            warnings.insert(0, message.to_string());
        }
    }

    if revert_files && !dry_run && matches!(meta.kind, CheckpointKind::TurnEnd) {
        if let Err(err) = record_pre_rollback(ctx, &checkpoint_id) {
            record_restore_audit(ctx, false, format!("pre-rollback failed: {err}"));
            return Err(format!("pre-rollback 失败，已中止 restore：{err}"));
        }
    }

    let mut summary = None;
    let mut restored_paths = Vec::new();
    let changed_paths = if revert_files {
        let restore_plan = restore_plan
            .as_ref()
            .expect("revert restore should compute restore path plan");
        let report = match ctx.scope_services.checkpoint_store.restore(
            &checkpoint_id,
            RestoreOptions {
                paths: restore_plan.paths.clone(),
                dry_run,
            },
        ) {
            Ok(report) => report,
            Err(err) => {
                record_restore_audit(ctx, false, format!("restore failed: {err}"));
                return Err(format!("restore 失败：{err}"));
            }
        };
        summary = report.summary.clone();
        restored_paths = resolved_restore_paths(&report, &restore_plan.paths);
        if !dry_run
            && matches!(
                meta.kind,
                CheckpointKind::TurnEnd | CheckpointKind::Interrupt
            )
        {
            if let Err(err) = finalize_restore_transcript(ctx, &meta, &restored_paths) {
                record_restore_audit(
                    ctx,
                    false,
                    format!("restore applied but transcript finalize failed: {err}"),
                );
                return Err(format!("restore 已改盘，但 transcript 回滚失败：{err}"));
            }
        }
        changed_paths_for_report(&report, &restore_plan.paths)
    } else {
        if dry_run {
            summary = checkpoint_diff_summary(ctx, &checkpoint_id);
        }
        if !dry_run
            && matches!(
                meta.kind,
                CheckpointKind::TurnEnd | CheckpointKind::Interrupt
            )
        {
            if let Err(err) = finalize_restore_transcript(ctx, &meta, &[]) {
                record_restore_audit(ctx, false, format!("transcript-only restore failed: {err}"));
                return Err(format!("restore 对话截断失败：{err}"));
            }
        }
        note_paths
            .iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect()
    };

    let mut reloaded_plan_id = None;
    if revert_files && !dry_run {
        match ctx
            .session_runtime
            .plan_runtime
            .reload_active_plan_from_disk()
        {
            Ok(Some(plan_id)) => {
                reloaded_plan_id = Some(plan_id);
            }
            Ok(None) => {}
            Err(err) => {
                warnings.push(format!("plan_runtime 重新对齐失败（仅警告）：{err}"));
            }
        }
    }

    record_restore_audit(
        ctx,
        true,
        format!(
            "restore {} kind={} revert_files={} dry_run={} paths={}",
            checkpoint_id,
            checkpoint_kind_label(&meta.kind),
            revert_files,
            dry_run,
            if restored_paths.is_empty() {
                changed_paths.join(",")
            } else {
                restored_paths.join(",")
            }
        ),
    );

    Ok(RestoreCoreReport {
        changed_paths,
        dry_run,
        meta: meta.clone(),
        restored_paths,
        revert_files,
        reloaded_plan_id,
        summary,
        transcript_truncated: !dry_run
            && matches!(
                meta.kind,
                CheckpointKind::TurnEnd | CheckpointKind::Interrupt
            ),
        warnings,
    })
}

fn record_pre_rollback(ctx: &ChatContext, checkpoint_id: &CheckpointId) -> Result<(), String> {
    let label = format!("pre-rollback to {}", checkpoint_id.short());
    let message_anchor = current_leaf_message_id(ctx);
    let session_id = ctx
        .session_runtime
        .session
        .current_session_id()
        .map_err(|err| err.to_string())?
        .ok_or_else(|| "无当前会话".to_string())?;
    ctx.scope_services
        .checkpoint_store
        .record(CheckpointRecordRequest {
            session_id,
            turn_id: format!(
                "restore::pre-rollback::{}::{}",
                checkpoint_id,
                Utc::now().timestamp_millis()
            ),
            kind: CheckpointKind::Manual { label },
            message_anchor,
            notes: None,
        })
        .map(|_| ())
        .map_err(|err| err.to_string())
}

fn finalize_restore_transcript(
    ctx: &ChatContext,
    meta: &crate::core::CheckpointMeta,
    restored_paths: &[String],
) -> Result<(), String> {
    let anchor = meta
        .message_anchor
        .as_deref()
        .ok_or_else(|| "checkpoint 缺少 message_anchor，无法安全标记 superseded".to_string())?;
    ctx.session_runtime
        .session
        .mark_messages_after_anchor_superseded(anchor)
        .map_err(|err| err.to_string())?;
    ctx.session_runtime
        .session
        .append_custom_entry(json!({
            "customType": "checkpoint.restore",
            "checkpointId": meta.id.to_string(),
            "checkpointKind": checkpoint_kind_label(&meta.kind),
            "anchorMessageId": anchor,
            "restoredPaths": restored_paths,
        }))
        .map_err(|err| err.to_string())?;
    ctx.session_runtime
        .session
        .update_session(ctx.session_runtime.session.current_session_key(), |entry| {
            entry.last_checkpoint_id = Some(meta.id.to_string());
        })
        .map_err(|err| err.to_string())?;
    Ok(())
}

fn record_restore_audit(ctx: &ChatContext, success: bool, detail: String) {
    ctx.global_services
        .audit
        .record_hostcall(HostcallAuditEntry {
            plugin_id: "chat".to_string(),
            module: "session".to_string(),
            method: "restore".to_string(),
            success,
            detail: Some(detail),
        });
}

fn current_leaf_message_id(ctx: &ChatContext) -> Option<String> {
    let path = ctx
        .session_runtime
        .session
        .current_transcript_path()
        .ok()
        .flatten()?;
    let leaf = crate::core::session::transcript::get_leaf_entry(&path)
        .ok()
        .flatten()?;
    match leaf {
        TranscriptEntry::Message(me) => me.id,
        _ => None,
    }
}

fn resolved_restore_paths(
    report: &crate::core::CheckpointRestoreReport,
    requested_paths: &[PathBuf],
) -> Vec<String> {
    if !report.changed_paths.is_empty() {
        return report
            .changed_paths
            .iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect();
    }
    if !requested_paths.is_empty() {
        return requested_paths
            .iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect();
    }
    vec![".".to_string()]
}

fn changed_paths_for_report(
    report: &crate::core::CheckpointRestoreReport,
    requested_paths: &[PathBuf],
) -> Vec<String> {
    if !report.changed_paths.is_empty() {
        return report
            .changed_paths
            .iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect();
    }
    requested_paths
        .iter()
        .map(|path| path.to_string_lossy().to_string())
        .collect()
}

fn checkpoint_diff_summary(ctx: &ChatContext, checkpoint_id: &CheckpointId) -> Option<String> {
    ctx.scope_services
        .checkpoint_store
        .diff(checkpoint_id)
        .ok()
        .and_then(|diff| (!diff.text.trim().is_empty()).then_some(diff.text))
}

fn collect_restore_warnings(
    ctx: &ChatContext,
    checkpoint_id: &CheckpointId,
    restore_paths: &[PathBuf],
) -> Vec<String> {
    let current_session_id = ctx
        .session_runtime
        .session
        .current_session_id()
        .ok()
        .flatten();
    let conflicts =
        other_session_restore_conflicts(ctx, current_session_id.as_deref(), restore_paths);
    if conflicts.is_empty() {
        return Vec::new();
    }
    let detail = render_restore_conflicts(&conflicts);
    warn!(
        checkpoint_id = %checkpoint_id,
        conflicts = %detail,
        "restore may overlap other session changes"
    );
    vec![format!("本次 restore 可能影响其他会话改动：{detail}")]
}

pub(crate) fn effective_restore_paths(
    ctx: &ChatContext,
    checkpoint_id: &CheckpointId,
    meta: &crate::core::CheckpointMeta,
    requested_paths: &[PathBuf],
) -> RestorePathPlan {
    if !requested_paths.is_empty() {
        return RestorePathPlan {
            paths: requested_paths.to_vec(),
            warning: None,
        };
    }
    let meta_paths = checkpoint_note_paths(meta.notes.as_ref());
    if !meta_paths.is_empty() {
        return RestorePathPlan {
            paths: meta_paths,
            warning: None,
        };
    }
    match ctx.scope_services.checkpoint_store.diff(checkpoint_id) {
        Ok(diff) if !diff.changed_paths.is_empty() => RestorePathPlan {
            paths: diff.changed_paths,
            warning: None,
        },
        Ok(_) => RestorePathPlan {
            paths: Vec::new(),
            warning: Some(
                "无法自动收窄 restore 路径，将继续执行整树 restore。".to_string(),
            ),
        },
        Err(err) => RestorePathPlan {
            paths: Vec::new(),
            warning: Some(format!(
                "无法自动收窄 restore 路径（读取 checkpoint diff 失败：{err}），将继续执行整树 restore。"
            )),
        },
    }
}

pub(crate) fn other_session_restore_conflicts(
    ctx: &ChatContext,
    current_session_id: Option<&str>,
    restore_paths: &[PathBuf],
) -> Vec<RestoreConflict> {
    if restore_paths.is_empty() {
        return Vec::new();
    }
    let restore_set: std::collections::BTreeSet<PathBuf> = restore_paths.iter().cloned().collect();
    let Ok(store) = ctx.session_runtime.session.load_store() else {
        return Vec::new();
    };
    let mut session_ids: Vec<String> = store.sessions.keys().cloned().collect();
    session_ids.sort();
    let mut conflicts = Vec::new();
    for session_id in session_ids {
        if current_session_id.is_some_and(|current| current == session_id.as_str()) {
            continue;
        }
        let Ok(metas) = ctx
            .scope_services
            .checkpoint_store
            .list(&session_id, ListOptions::default())
        else {
            continue;
        };
        let overlapping: Vec<PathBuf> = metas
            .into_iter()
            .flat_map(|meta| checkpoint_note_paths(meta.notes.as_ref()))
            .filter(|path| restore_set.contains(path))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        if !overlapping.is_empty() {
            conflicts.push(RestoreConflict {
                session_id,
                paths: overlapping,
            });
        }
    }
    conflicts
}

fn render_restore_conflicts(conflicts: &[RestoreConflict]) -> String {
    conflicts
        .iter()
        .map(|conflict| {
            format!(
                "{}({})",
                conflict.session_id,
                conflict
                    .paths
                    .iter()
                    .map(|path| path.to_string_lossy().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn checkpoint_note_paths(notes: Option<&serde_json::Value>) -> Vec<PathBuf> {
    notes
        .and_then(|notes| notes.get("changedPaths"))
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .filter(|path| !path.trim().is_empty())
        .map(PathBuf::from)
        .collect()
}
