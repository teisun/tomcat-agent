use crate::api::chat::ChatContext;
use crate::infra::i18n::tr;

use super::parse::{ChatCommand, ChatCommandOutcome};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillCommand {
    List,
    Reload,
    Use { name: String, intent: String },
}

pub(crate) fn parse_args(tokens: Vec<String>) -> ChatCommand {
    match tokens.as_slice() {
        [_cmd, sub] if sub == "list" => ChatCommand::Skill(SkillCommand::List),
        [_cmd, sub] if sub == "reload" => ChatCommand::Skill(SkillCommand::Reload),
        [_cmd, sub, name, intent @ ..] if sub == "use" && !intent.is_empty() => {
            let intent = intent.join(" ").trim().to_string();
            if intent.is_empty() {
                ChatCommand::UsageError {
                    message: usage_text(),
                }
            } else {
                ChatCommand::Skill(SkillCommand::Use {
                    name: name.to_string(),
                    intent,
                })
            }
        }
        _ => ChatCommand::UsageError {
            message: usage_text(),
        },
    }
}

fn usage_text() -> String {
    tr("slash.skill.usage", &[])
}

pub(crate) async fn run(ctx: &ChatContext, command: SkillCommand) -> ChatCommandOutcome {
    match command {
        SkillCommand::List => run_list(ctx),
        SkillCommand::Reload => run_reload(ctx).await,
        SkillCommand::Use { name, intent } => run_use(ctx, &name, &intent).await,
    }
}

fn run_list(ctx: &ChatContext) -> ChatCommandOutcome {
    if !ctx.config.skills.enabled {
        println!("{}", tr("slash.skill.disabled", &[]));
    }
    println!(
        "{}",
        crate::core::skill::render_skill_inventory(&ctx.skill_set_snapshot())
    );
    ChatCommandOutcome::Handled
}

async fn run_reload(ctx: &ChatContext) -> ChatCommandOutcome {
    let skill_set = ctx.reload_skill_set().await;
    if ctx.config.skills.enabled {
        println!("{}", tr("slash.skill.reloaded", &[]));
    } else {
        println!("{}", tr("slash.skill.cleared", &[]));
    }
    println!("{}", crate::core::skill::render_skill_inventory(&skill_set));
    ChatCommandOutcome::Handled
}

async fn run_use(ctx: &ChatContext, name: &str, intent: &str) -> ChatCommandOutcome {
    if !ctx.config.skills.enabled {
        println!("{}", tr("slash.skill.useDisabled", &[]));
        return ChatCommandOutcome::Handled;
    }

    let snapshot = ctx.skill_set_snapshot();
    if snapshot.resolve_any(name).is_none() {
        let available = crate::core::skill::available_skill_names_csv(&snapshot);
        let available = if available.is_empty() {
            "<none>".to_string()
        } else {
            available
        };
        println!(
            "{}",
            tr(
                "slash.skill.unknown",
                &[("name", name), ("available", &available)]
            )
        );
        return ChatCommandOutcome::Handled;
    }

    match super::cmd_command::message(
        ctx,
        &format!("skill:{name}"),
        crate::core::project_instructions::InstructionKind::Skill,
        intent,
    ) {
        Ok(message) => ChatCommandOutcome::UserMessage {
            message,
            history_line: format!("/skill use {name} {intent}"),
        },
        Err(error) => {
            println!(
                "{}",
                tr(
                    "slash.skill.loadFailed",
                    &[("name", name), ("detail", &error.to_string())]
                )
            );
            ChatCommandOutcome::Handled
        }
    }
}
