use std::path::PathBuf;

use chrono::Utc;
use serde_json::json;

use crate::api::chat::ChatContext;
use crate::core::session::mark_message_entries_after_anchor_superseded;
use crate::core::{
    CheckpointId, CheckpointKind, CheckpointRecordRequest, RestoreOptions, TranscriptEntry,
};
use crate::infra::HostcallAuditEntry;

use super::cmd_ckpt::checkpoint_kind_label;
use super::parse::ChatCommandOutcome;

pub(crate) fn run(
    ctx: &ChatContext,
    checkpoint_id: String,
    paths: Vec<PathBuf>,
    dry_run: bool,
) -> ChatCommandOutcome {
    let checkpoint_id = CheckpointId::new(checkpoint_id);
    let meta = match ctx.checkpoint_store.show(&checkpoint_id) {
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

    let report = match ctx.checkpoint_store.restore(
        &checkpoint_id,
        RestoreOptions {
            paths: paths.clone(),
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
        if let Err(err) = finalize_restore_transcript(ctx, &meta, &report, &paths) {
            println!("restore 已改盘，但 transcript 回滚失败：{err}");
            record_restore_audit(
                ctx,
                false,
                format!("restore applied but transcript finalize failed: {err}"),
            );
            return ChatCommandOutcome::Handled;
        }
    }

    let restored_paths = restored_paths_for_entry(&report, &paths);
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
    match ctx.plan_runtime.reload_active_plan_from_disk() {
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
        .session
        .current_session_id()
        .map_err(|err| err.to_string())?
        .ok_or_else(|| "无当前会话".to_string())?;
    ctx.checkpoint_store
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
    let transcript_path = ctx
        .session
        .current_transcript_path()
        .map_err(|err| err.to_string())?
        .ok_or_else(|| "无当前 transcript".to_string())?;
    mark_message_entries_after_anchor_superseded(&transcript_path, anchor)
        .map_err(|err| err.to_string())?;
    ctx.session
        .append_custom_entry(json!({
            "customType": "checkpoint.restore",
            "checkpointId": meta.id.to_string(),
            "checkpointKind": checkpoint_kind_label(&meta.kind),
            "anchorMessageId": anchor,
            "restoredPaths": restored_paths_for_entry(report, requested_paths),
        }))
        .map_err(|err| err.to_string())?;
    ctx.session
        .update_session(ctx.session.current_session_key(), |entry| {
            entry.last_checkpoint_id = Some(meta.id.to_string());
        })
        .map_err(|err| err.to_string())?;
    Ok(())
}

fn record_restore_audit(ctx: &ChatContext, success: bool, detail: String) {
    ctx.audit.record_hostcall(HostcallAuditEntry {
        plugin_id: "chat".to_string(),
        module: "session".to_string(),
        method: "restore".to_string(),
        success,
        detail: Some(detail),
    });
}

fn current_leaf_message_id(ctx: &ChatContext) -> Option<String> {
    let path = ctx.session.current_transcript_path().ok().flatten()?;
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
