//! Shared resource commands: this table is the only remote command allowlist.
use super::parse::ChatCommandOutcome;
use super::{cmd_install, cmd_reload, cmd_uninstall};
use crate::api::chat::{ChatContext, InventoryReport};
use schemars::JsonSchema;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SharedSlashCommand {
    pub name: &'static str,
    pub usage: &'static str,
    pub summary: &'static str,
}
pub const SHARED_SLASH_COMMANDS: &[SharedSlashCommand] = &[
    SharedSlashCommand {
        name: "reload",
        usage: "/reload",
        summary: "重新核对磁盘上的 Skill 与插件",
    },
    SharedSlashCommand {
        name: "install",
        usage: "/install <来源> <current-project|agent|global>",
        summary: "安装 package / 插件 / Skill",
    },
    SharedSlashCommand {
        name: "uninstall",
        usage: "/uninstall <包名> <current-project|agent|global>",
        summary: "卸载 package",
    },
];

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SlashReply {
    pub ok: bool,
    pub text: String,
}
impl SlashReply {
    pub(crate) fn success(text: impl Into<String>) -> Self {
        Self {
            ok: true,
            text: text.into(),
        }
    }
    pub(crate) fn error(text: impl Into<String>) -> Self {
        Self {
            ok: false,
            text: text.into(),
        }
    }
}

pub fn shared_command(name: &str) -> Option<&'static SharedSlashCommand> {
    SHARED_SLASH_COMMANDS
        .iter()
        .find(|command| command.name == name)
}

/// An unlisted first token is normal chat input, even if it contains a slash.
/// Argument validation belongs to the shared executor so terminal-only target
/// selection can fill a missing layer without making Serve interactive.
pub fn parse_shared_slash(text: &str) -> Option<Result<(String, Vec<String>), String>> {
    let token = text.split_whitespace().next()?;
    let name = token.strip_prefix('/')?;
    let command = shared_command(name)?;
    Some(
        shell_words::split(text.trim())
            .map(|tokens| (name.to_string(), tokens.into_iter().skip(1).collect()))
            .map_err(|error| format!("命令参数解析失败：{error}\n用法：{}", command.usage)),
    )
}

enum SharedInvocation<'a> {
    Reload,
    Install {
        source: &'a str,
        target: cmd_install::InstallTarget,
    },
    Uninstall {
        package: &'a str,
        target: cmd_install::InstallTarget,
    },
}

fn unknown_command(name: &str) -> SlashReply {
    SlashReply::error(format!(
        "未知命令 /{name}，输入框可用：{}",
        SHARED_SLASH_COMMANDS
            .iter()
            .map(|command| format!("/{}", command.name))
            .collect::<Vec<_>>()
            .join(" ")
    ))
}

fn parse_invocation<'a>(
    name: &str,
    args: &'a [String],
) -> Result<SharedInvocation<'a>, SlashReply> {
    let command = shared_command(name).ok_or_else(|| unknown_command(name))?;
    let invocation = match name {
        "reload" => args.is_empty().then_some(SharedInvocation::Reload),
        "install" | "uninstall" => match args {
            [subject, layer] => cmd_install::parse_target(layer).map(|target| {
                if name == "install" {
                    SharedInvocation::Install {
                        source: subject,
                        target,
                    }
                } else {
                    SharedInvocation::Uninstall {
                        package: subject,
                        target,
                    }
                }
            }),
            _ => None,
        },
        _ => return Err(unknown_command(name)),
    };
    invocation.ok_or_else(|| {
        SlashReply::error(format!(
            "[{name}] 用法：{}{}",
            command.usage,
            if name == "reload" {
                "（不接受参数）"
            } else {
                ""
            }
        ))
    })
}

/// Validate without changing state; Serve uses this before reserving a session.
pub fn shared_usage_error(name: &str, args: &[String]) -> Option<SlashReply> {
    parse_invocation(name, args).err()
}

/// No printing, transcript mutation, model call, or interactive question here.
pub async fn run_shared_slash_command(
    ctx: &ChatContext,
    name: &str,
    args: &[String],
) -> SlashReply {
    let invocation = match parse_invocation(name, args) {
        Ok(invocation) => invocation,
        Err(reply) => return reply,
    };
    let reply = match invocation {
        SharedInvocation::Reload => cmd_reload::run(ctx).await,
        SharedInvocation::Install { source, target } => cmd_install::run(ctx, source, target).await,
        SharedInvocation::Uninstall { package, target } => {
            cmd_uninstall::run(ctx, package, target).await
        }
    };
    if reply.ok {
        SlashReply::success(format!("{}\n{}", reply.text, ctx.instruction_summary()))
    } else {
        reply
    }
}

pub(crate) async fn run_terminal_shared(
    ctx: &ChatContext,
    name: &str,
    mut args: Vec<String>,
) -> ChatCommandOutcome {
    if matches!(name, "install" | "uninstall") && args.len() == 1 {
        let Some(target) = cmd_install::choose_target(ctx, name).await else {
            println!("[{name}] 已取消，未写入任何文件。");
            return ChatCommandOutcome::Handled;
        };
        args.push(target.label().to_string());
    }
    println!("{}", run_shared_slash_command(ctx, name, &args).await.text);
    ChatCommandOutcome::Handled
}

pub(crate) fn render_sync_summary(name: &str, report: &InventoryReport) -> String {
    let counts = |added: usize, removed: usize, changed: usize| {
        let parts = [("+", added), ("-", removed), ("~", changed)]
            .into_iter()
            .filter(|(_, count)| *count != 0)
            .map(|(sign, count)| format!("{sign}{count}"))
            .collect::<Vec<_>>();
        if parts.is_empty() {
            "不变".to_string()
        } else {
            parts.join(" ")
        }
    };
    let mut lines = vec![if report.has_changes() {
        format!(
            "[{name}] 当前会话已同步：Skill {}，插件 {}（用时 {:.1}ms）",
            counts(
                report.skills_added.len(),
                report.skills_removed.len(),
                report.skills_changed.len()
            ),
            counts(
                report.plugins_added.len(),
                report.plugins_removed.len(),
                report.plugins_changed.len()
            ),
            report.timings.total.as_secs_f64() * 1000.0
        )
    } else {
        format!(
            "[{name}] {}：Skill 与插件没有变化（用时 {:.1}ms）",
            if report.warnings.is_empty() {
                "当前会话已是最新"
            } else {
                "当前清单保持不变，请查看警告"
            },
            report.timings.total.as_secs_f64() * 1000.0
        )
    }];
    for (verb, kind, values) in [
        ("新增", "skill", &report.skills_added),
        ("移除", "skill", &report.skills_removed),
        ("更新", "skill", &report.skills_changed),
        ("新增", "plugin", &report.plugins_added),
        ("移除", "plugin", &report.plugins_removed),
        ("更新", "plugin", &report.plugins_changed),
    ] {
        for value in values {
            lines.push(format!("  - {verb} {kind}: {value}"));
        }
    }
    for warning in &report.warnings {
        lines.push(format!("  - 警告: {warning}"));
    }
    lines.join("\n")
}
