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

pub(crate) fn run(
    ctx: &ChatContext,
    checkpoint_id: String,
    paths: Vec<PathBuf>,
    dry_run: bool,
) -> ChatCommandOutcome {
    let checkpoint_id = CheckpointId::new(checkpoint_id);
    let meta = match ctx.scope_services.checkpoint_store.show(&checkpoint_id) {
        Ok(Some(meta)) => meta,
        Ok(None) => {
            println!("未找到 checkpoint: {checkpoint_id}");
            return ChatCommandOutcome::Handled;
        }
        Err(err) => {
            println!("读取 checkpoint 失败：{err}");
            record_restore_audit(ctx, false, format!("show failed: {err}"));
            return ChatCommandOutcome::Handled;
        }
    };

    if !dry_run && matches!(meta.kind, CheckpointKind::TurnEnd) {
        if let Err(err) = record_pre_rollback(ctx, &checkpoint_id) {
            println!("pre-rollback 失败，已中止 restore：{err}");
            record_restore_audit(ctx, false, format!("pre-rollback failed: {err}"));
            return ChatCommandOutcome::Handled;
        }
    }

    let restore_plan = effective_restore_paths(ctx, &checkpoint_id, &meta, &paths);
    if let Some(message) = restore_plan.warning.as_deref() {
        warn!(checkpoint_id = %checkpoint_id, "{message}");
        println!("警告：{message}");
    }
    let current_session_id = ctx.session_runtime.session.current_session_id().ok().flatten();
    let conflicts = other_session_restore_conflicts(
        ctx,
        current_session_id.as_deref(),
        &restore_plan.paths,
    );
    if !conflicts.is_empty() {
        let detail = render_restore_conflicts(&conflicts);
        warn!(
            checkpoint_id = %checkpoint_id,
            conflicts = %detail,
            "restore may overlap other session changes"
        );
        println!("警告：本次 restore 可能影响其他会话改动：{detail}");
    }
    let report = match ctx.scope_services.checkpoint_store.restore(
        &checkpoint_id,
        RestoreOptions {
            paths: restore_plan.paths.clone(),
            dry_run,
        },
    ) {
        Ok(report) => report,
        Err(err) => {
            println!("restore 失败：{err}");
            record_restore_audit(ctx, false, format!("restore failed: {err}"));
            return ChatCommandOutcome::Handled;
        }
    };

    if dry_run {
        println!("dry-run: {}", checkpoint_id);
        if let Some(summary) = report.summary {
            print!("{}", summary);
        } else {
            println!("当前工作区与目标 checkpoint 无差异。");
        }
        record_restore_audit(ctx, true, format!("dry-run restore {}", checkpoint_id));
        return ChatCommandOutcome::Handled;
    }

    if matches!(
        meta.kind,
        CheckpointKind::TurnEnd | CheckpointKind::Interrupt
    ) {
        if let Err(err) = finalize_restore_transcript(ctx, &meta, &report, &restore_plan.paths) {
            println!("restore 已改盘，但 transcript 回滚失败：{err}");
            record_restore_audit(
                ctx,
                false,
                format!("restore applied but transcript finalize failed: {err}"),
            );
            return ChatCommandOutcome::Handled;
        }
    }

    let restored_paths = restored_paths_for_entry(&report, &restore_plan.paths);
    if restored_paths.is_empty() {
        println!("已恢复 checkpoint {}。", checkpoint_id);
    } else {
        println!(
            "已恢复 checkpoint {}：{}",
            checkpoint_id,
            restored_paths.join(", ")
        );
    }
    // E7：树恢复完成后，把内存里的 plan 模式与磁盘对齐——若磁盘 active executing
    // plan 与本 session 绑定，则把 PlanRuntime 切回 Executing；否则保持当前模式。
    // 失败仅 warning，不影响树恢复的成功结果。
    match ctx.session_runtime.plan_runtime.reload_active_plan_from_disk() {
        Ok(Some(plan_id)) => {
            println!("plan_runtime 已对齐磁盘：EXEC plan_id={plan_id}");
        }
        Ok(None) => {}
        Err(err) => {
            println!("plan_runtime 重新对齐失败（仅警告）：{err}");
        }
    }
    record_restore_audit(
        ctx,
        true,
        format!(
            "restore {} kind={} paths={}",
            checkpoint_id,
            checkpoint_kind_label(&meta.kind),
            restored_paths.join(",")
        ),
    );
    ChatCommandOutcome::Handled
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
    report: &crate::core::CheckpointRestoreReport,
    requested_paths: &[PathBuf],
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
            "restoredPaths": restored_paths_for_entry(report, requested_paths),
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
    ctx.global_services.audit.record_hostcall(HostcallAuditEntry {
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

fn restored_paths_for_entry(
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
    let restore_set: std::collections::BTreeSet<PathBuf> =
        restore_paths.iter().cloned().collect();
    let Ok(entries) = ctx.session_runtime.session.list_sessions() else {
        return Vec::new();
    };
    let mut conflicts = Vec::new();
    for (session_id, _) in entries {
        if current_session_id.is_some_and(|current| current == session_id) {
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
