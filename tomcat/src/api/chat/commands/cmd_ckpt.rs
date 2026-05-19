use crate::api::chat::ChatContext;
use crate::core::{CheckpointKind, CheckpointMeta, ListOptions};

use super::parse::ChatCommandOutcome;

pub(crate) fn run_list(ctx: &ChatContext, limit: Option<usize>) -> ChatCommandOutcome {
    match ctx
        .checkpoint_store
        .list(ctx.session.current_session_key(), ListOptions { limit })
    {
        Ok(entries) => {
            if entries.is_empty() {
                println!("暂无 checkpoint。");
                return ChatCommandOutcome::Handled;
            }
            for meta in entries {
                println!(
                    "{}  {:<10}  {}  {}",
                    meta.id,
                    checkpoint_kind_label(&meta.kind),
                    meta.created_at,
                    meta.turn_id
                );
            }
        }
        Err(err) => {
            println!("checkpoint 列表读取失败：{err}");
        }
    }
    ChatCommandOutcome::Handled
}

pub(crate) fn run_show(ctx: &ChatContext, checkpoint_id: String) -> ChatCommandOutcome {
    match ctx
        .checkpoint_store
        .show(&crate::core::CheckpointId::new(checkpoint_id.clone()))
    {
        Ok(Some(meta)) => print_checkpoint_meta(&meta),
        Ok(None) => println!("未找到 checkpoint: {checkpoint_id}"),
        Err(err) => println!("checkpoint 元数据读取失败：{err}"),
    }
    ChatCommandOutcome::Handled
}

pub(crate) fn run_diff(ctx: &ChatContext, checkpoint_id: String) -> ChatCommandOutcome {
    match ctx
        .checkpoint_store
        .diff(&crate::core::CheckpointId::new(checkpoint_id.clone()))
    {
        Ok(diff) => {
            if diff.text.trim().is_empty() {
                println!("当前工作区与 checkpoint {checkpoint_id} 无差异。");
            } else {
                print!("{}", diff.text);
            }
        }
        Err(err) => println!("checkpoint diff 失败：{err}"),
    }
    ChatCommandOutcome::Handled
}

pub(crate) fn checkpoint_kind_label(kind: &CheckpointKind) -> &'static str {
    match kind {
        CheckpointKind::TurnEnd => "turn_end",
        CheckpointKind::Interrupt => "interrupt",
        CheckpointKind::Manual { .. } => "manual",
    }
}

fn print_checkpoint_meta(meta: &CheckpointMeta) {
    println!("id: {}", meta.id);
    println!("kind: {}", checkpoint_kind_label(&meta.kind));
    println!("created_at: {}", meta.created_at);
    println!("session_id: {}", meta.session_id);
    println!("turn_id: {}", meta.turn_id);
    if let Some(anchor) = &meta.message_anchor {
        println!("message_anchor: {anchor}");
    }
    if let Some(commit) = &meta.git_commit {
        println!("git_commit: {commit}");
    }
    match &meta.kind {
        CheckpointKind::Manual { label } => println!("label: {label}"),
        _ => {}
    }
}
