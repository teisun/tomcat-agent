use crate::api::chat::ChatContext;
use crate::core::{CheckpointKind, CheckpointMeta, ListOptions};
use crate::infra::i18n::tr;

use super::parse::ChatCommandOutcome;

pub(crate) fn run_list(ctx: &ChatContext, limit: Option<usize>) -> ChatCommandOutcome {
    let session_id = match ctx.session_runtime.session.current_session_id() {
        Ok(Some(v)) => v,
        Ok(None) => {
            println!("{}", tr("terminal.noSession", &[]));
            return ChatCommandOutcome::Handled;
        }
        Err(err) => {
            println!(
                "{}",
                tr("slash.ckpt.sessionFailed", &[("detail", &err.to_string())])
            );
            return ChatCommandOutcome::Handled;
        }
    };
    match ctx
        .scope_services
        .checkpoint_store
        .list(&session_id, ListOptions { limit })
    {
        Ok(entries) => {
            if entries.is_empty() {
                println!("{}", tr("slash.ckpt.empty", &[]));
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
            println!(
                "{}",
                tr("slash.ckpt.listFailed", &[("detail", &err.to_string())])
            );
        }
    }
    ChatCommandOutcome::Handled
}

pub(crate) fn run_show(ctx: &ChatContext, checkpoint_id: String) -> ChatCommandOutcome {
    match ctx
        .scope_services
        .checkpoint_store
        .show(&crate::core::CheckpointId::new(checkpoint_id.clone()))
    {
        Ok(Some(meta)) => print_checkpoint_meta(&meta),
        Ok(None) => println!("{}", tr("slash.ckpt.notFound", &[("id", &checkpoint_id)])),
        Err(err) => println!(
            "{}",
            tr("slash.ckpt.metaFailed", &[("detail", &err.to_string())])
        ),
    }
    ChatCommandOutcome::Handled
}

pub(crate) fn run_diff(ctx: &ChatContext, checkpoint_id: String) -> ChatCommandOutcome {
    match ctx
        .scope_services
        .checkpoint_store
        .diff(&crate::core::CheckpointId::new(checkpoint_id.clone()))
    {
        Ok(diff) => {
            if diff.text.trim().is_empty() {
                println!("{}", tr("slash.ckpt.noDiff", &[("id", &checkpoint_id)]));
            } else {
                print!("{}", diff.text);
            }
        }
        Err(err) => println!(
            "{}",
            tr("slash.ckpt.diffFailed", &[("detail", &err.to_string())])
        ),
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
    if let CheckpointKind::Manual { label } = &meta.kind {
        println!("label: {label}");
    }
}
