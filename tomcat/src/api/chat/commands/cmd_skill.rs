use crate::api::chat::ChatContext;

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
    "用法错误：/skill list | /skill reload | /skill use <name> \"intent...\"".to_string()
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
        println!("[skill] 技能系统当前已禁用（[skills].enabled=false）。");
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
        println!("[skill] 已重载技能目录。");
    } else {
        println!("[skill] 技能系统已禁用；当前运行时 SkillSet 已清空。");
    }
    println!("{}", crate::core::skill::render_skill_inventory(&skill_set));
    ChatCommandOutcome::Handled
}

async fn run_use(ctx: &ChatContext, name: &str, intent: &str) -> ChatCommandOutcome {
    if !ctx.config.skills.enabled {
        println!("[skill] 技能系统已禁用，无法执行 /skill use。");
        return ChatCommandOutcome::Handled;
    }

    let snapshot = ctx.skill_set_snapshot();
    let skill = match snapshot.resolve_any(name) {
        Some(skill) => skill.clone(),
        None => {
            let available = crate::core::skill::available_skill_names_csv(&snapshot);
            let available = if available.is_empty() {
                "<none>".to_string()
            } else {
                available
            };
            println!(
                "[skill] 未知 skill `{name}`。当前可用技能: {available}。如已修改磁盘，请先执行 /skill reload。"
            );
            return ChatCommandOutcome::Handled;
        }
    };

    match crate::core::skill::load_skill_payload(ctx.primitive.as_ref(), "__agent__", &skill, None)
        .await
    {
        Ok(payload) => ChatCommandOutcome::Continue {
            line: format!(
                "User explicitly requested skill `{name}` for this turn. Treat the skill body below as required context for the current task.\n\n{payload}\n\nCurrent user intent:\n{intent}"
            ),
            echo_user: false,
            history_line: Some(format!("/skill use {name} {intent}")),
        },
        Err(error) => {
            println!("[skill] 加载 `{name}` 失败: {error}");
            ChatCommandOutcome::Handled
        }
    }
}
