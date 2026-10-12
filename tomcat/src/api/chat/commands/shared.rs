//! Shared resource commands: this table is the only remote command allowlist.
use super::parse::ChatCommandOutcome;
use super::{cmd_install, cmd_reload, cmd_uninstall};
use crate::api::chat::{ChatContext, InventoryReport};
use crate::infra::i18n::tr;
use schemars::JsonSchema;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SharedSlashCommand {
    pub name: &'static str,
    pub usage: String,
    pub summary: String,
}
pub fn shared_slash_commands() -> Vec<SharedSlashCommand> {
    ["reload", "install", "uninstall"]
        .into_iter()
        .map(|name| SharedSlashCommand {
            name,
            usage: tr(&format!("slash.{name}.usage"), &[]),
            summary: tr(&format!("slash.{name}.summary"), &[]),
        })
        .collect()
}

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

pub fn shared_command(name: &str) -> Option<SharedSlashCommand> {
    shared_slash_commands()
        .into_iter()
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
            .map_err(|error| {
                tr(
                    "slash.parseFailed",
                    &[("detail", &error.to_string()), ("usage", &command.usage)],
                )
            }),
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
    let commands = shared_slash_commands()
        .iter()
        .map(|command| format!("/{}", command.name))
        .collect::<Vec<_>>()
        .join(" ");
    SlashReply::error(tr(
        "slash.unknown",
        &[("name", name), ("commands", &commands)],
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
        SlashReply::error(tr(
            "slash.usage",
            &[
                ("name", name),
                ("usage", &command.usage),
                (
                    "suffix",
                    &if name == "reload" {
                        tr("slash.noArguments", &[])
                    } else {
                        String::new()
                    },
                ),
            ],
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
            println!("{}", tr("slash.cancelled", &[("name", name)]));
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
            tr("slash.sync.unchanged", &[])
        } else {
            parts.join(" ")
        }
    };
    let elapsed = format!("{:.1}", report.timings.total.as_secs_f64() * 1000.0);
    let mut lines = vec![if report.has_changes() {
        tr(
            "slash.sync.changed",
            &[
                ("name", name),
                (
                    "skills",
                    &counts(
                        report.skills_added.len(),
                        report.skills_removed.len(),
                        report.skills_changed.len(),
                    ),
                ),
                (
                    "plugins",
                    &counts(
                        report.plugins_added.len(),
                        report.plugins_removed.len(),
                        report.plugins_changed.len(),
                    ),
                ),
                ("elapsed", &elapsed),
            ],
        )
    } else {
        tr(
            "slash.sync.noChanges",
            &[
                ("name", name),
                ("elapsed", &elapsed),
                (
                    "status",
                    &tr(
                        if report.warnings.is_empty() {
                            "slash.sync.latest"
                        } else {
                            "slash.sync.warnings"
                        },
                        &[],
                    ),
                ),
            ],
        )
    }];
    for (verb, kind, values) in [
        ("slash.sync.added", "skill", &report.skills_added),
        ("slash.sync.removed", "skill", &report.skills_removed),
        ("slash.sync.updated", "skill", &report.skills_changed),
        ("slash.sync.added", "plugin", &report.plugins_added),
        ("slash.sync.removed", "plugin", &report.plugins_removed),
        ("slash.sync.updated", "plugin", &report.plugins_changed),
    ] {
        for value in values {
            lines.push(format!("  - {} {kind}: {value}", tr(verb, &[])));
        }
    }
    for warning in &report.warnings {
        lines.push(tr("slash.warning", &[("detail", warning)]));
    }
    lines.join("\n")
}
