use crate::api::chat::ChatContext;
use crate::core::llm::{Capabilities, LlmScene, ModelEntry};
use crate::infra::i18n::tr;

use super::parse::{ChatCommand, ChatCommandOutcome};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelCommand {
    Current,
    List,
    Use { model_id: String },
}

pub(crate) fn parse_args(tokens: Vec<String>) -> ChatCommand {
    match tokens.as_slice() {
        [_cmd] => ChatCommand::Model(ModelCommand::Current),
        [_cmd, sub] if sub == "current" => ChatCommand::Model(ModelCommand::Current),
        [_cmd, sub] if sub == "list" => ChatCommand::Model(ModelCommand::List),
        [_cmd, sub, model_id] if sub == "use" => ChatCommand::Model(ModelCommand::Use {
            model_id: model_id.to_string(),
        }),
        _ => ChatCommand::UsageError {
            message: tr("slash.model.usage", &[]),
        },
    }
}

pub(crate) fn run(ctx: &ChatContext, command: ModelCommand) -> ChatCommandOutcome {
    match command {
        ModelCommand::Current => run_current(ctx),
        ModelCommand::List => run_list(ctx),
        ModelCommand::Use { model_id } => run_use(ctx, &model_id),
    }
}

fn run_current(ctx: &ChatContext) -> ChatCommandOutcome {
    let entry = match ctx
        .session_runtime
        .session
        .get_session(ctx.session_runtime.session.current_session_key())
    {
        Ok(entry) => entry,
        Err(err) => {
            println!(
                "{}",
                tr(
                    "slash.sessionReadFailed",
                    &[("command", "model"), ("detail", &err.to_string())]
                )
            );
            return ChatCommandOutcome::Handled;
        }
    };
    let current_model = ctx.effective_model(entry.as_ref());
    println!(
        "{}",
        tr("slash.model.current", &[("model", &current_model)])
    );
    println!(
        "{}",
        tr(
            "slash.model.default",
            &[("model", &ctx.config.llm.default_model)]
        )
    );
    match ctx.resolve_call(LlmScene::Main, entry.as_ref()) {
        Ok(resolved) => {
            println!(
                "{}",
                tr(
                    "slash.model.resolved",
                    &[
                        ("api", &resolved.api),
                        ("provider", &resolved.provider),
                        (
                            "url",
                            resolved
                                .base_url
                                .as_deref()
                                .unwrap_or(&tr("slash.model.providerDefault", &[]))
                        ),
                        ("source", &resolved.key_source),
                    ]
                )
            );
        }
        Err(err) => {
            println!(
                "{}",
                tr("slash.model.resolveFailed", &[("detail", &err.to_string())])
            );
        }
    }
    ChatCommandOutcome::Handled
}

fn run_list(ctx: &ChatContext) -> ChatCommandOutcome {
    let entry = match ctx
        .session_runtime
        .session
        .get_session(ctx.session_runtime.session.current_session_key())
    {
        Ok(entry) => entry,
        Err(err) => {
            println!(
                "{}",
                tr(
                    "slash.sessionReadFailed",
                    &[("command", "model"), ("detail", &err.to_string())]
                )
            );
            return ChatCommandOutcome::Handled;
        }
    };
    let current_model = ctx.effective_model(entry.as_ref());
    let default_model = ctx.config.llm.default_model.as_str();

    println!("{}", tr("cli.model.list", &[]));
    for item in ctx.global_services.model_catalog.entries() {
        println!(
            "{}",
            format_model_list_line(&item, item.id == current_model, item.id == default_model)
        );
    }
    ChatCommandOutcome::Handled
}

fn run_use(ctx: &ChatContext, model_id: &str) -> ChatCommandOutcome {
    let model_id = model_id.trim();
    if model_id.is_empty() {
        println!("{}", tr("slash.model.useUsage", &[]));
        return ChatCommandOutcome::Handled;
    }

    let entry = match ctx.global_services.model_catalog.lookup_explicit(model_id) {
        Ok(entry) => entry,
        Err(err) => {
            println!("[model] {}", err);
            return ChatCommandOutcome::Handled;
        }
    };

    match ctx
        .session_runtime
        .session
        .switch_current_model(Some(&entry.provider), Some(&entry.id))
    {
        Ok(()) => {
            println!(
                "{}",
                tr(
                    "slash.model.switched",
                    &[
                        ("model", &entry.id),
                        ("api", &entry.api),
                        ("provider", &entry.provider)
                    ]
                )
            );
        }
        Err(err) => {
            println!(
                "{}",
                tr("slash.model.switchFailed", &[("detail", &err.to_string())])
            );
        }
    }
    ChatCommandOutcome::Handled
}

fn format_capabilities(capabilities: &Capabilities) -> String {
    let mut labels = Vec::new();
    if capabilities.vision {
        labels.push("vision");
    }
    if capabilities.files {
        labels.push("files");
    }
    if capabilities.tools {
        labels.push("tools");
    }
    if capabilities.reasoning {
        labels.push("reasoning");
    }
    if labels.is_empty() {
        "none".to_string()
    } else {
        labels.join("+")
    }
}

pub(crate) fn format_model_list_line(
    item: &ModelEntry,
    is_current: bool,
    is_default: bool,
) -> String {
    let mut tags = Vec::new();
    if is_current {
        tags.push("current");
    }
    if is_default {
        tags.push("default");
    }
    let tag_text = if tags.is_empty() {
        String::new()
    } else {
        format!(" [{}]", tags.join(", "))
    };
    format!(
        "  - {}{}  api={} provider={} caps={}",
        item.id,
        tag_text,
        item.api,
        item.provider,
        format_capabilities(&item.capabilities)
    )
}
