use crate::api::chat::ChatContext;
use crate::infra::i18n::tr;
use crate::{AppError, ModelPrefsStore};

use super::parse::{ChatCommand, ChatCommandOutcome};

pub(crate) fn parse_args(tokens: Vec<String>) -> ChatCommand {
    match tokens.as_slice() {
        [_cmd, value] => match parse_context_window(value) {
            Some(context_window) => ChatCommand::Context { context_window },
            None => usage_error(value),
        },
        [_cmd] => ChatCommand::UsageError {
            message: tr("slash.context.required", &[]),
        },
        _ => ChatCommand::UsageError {
            message: tr("slash.context.usage", &[]),
        },
    }
}

pub(crate) fn parse_context_window(value: &str) -> Option<u32> {
    value.parse::<u32>().ok().filter(|window| *window > 0)
}

pub(crate) fn apply_context_window(
    store: &ModelPrefsStore,
    model: &str,
    context_window: u32,
) -> Result<(), AppError> {
    store.set_context_window(model, Some(context_window))
}

pub(crate) fn run(ctx: &ChatContext, context_window: u32) -> ChatCommandOutcome {
    let session = match ctx
        .session_runtime
        .session
        .get_session(ctx.session_runtime.session.current_session_key())
    {
        Ok(session) => session,
        Err(error) => {
            println!(
                "{}",
                tr(
                    "slash.sessionReadFailed",
                    &[("command", "context"), ("detail", &error.to_string())]
                )
            );
            return ChatCommandOutcome::Handled;
        }
    };
    let model = ctx.effective_model(session.as_ref());
    let entry = match ctx.global_services.model_catalog.lookup_explicit(&model) {
        Ok(entry) => entry,
        Err(error) => {
            println!("[context] {error}");
            return ChatCommandOutcome::Handled;
        }
    };
    if entry.context_window_options.is_empty()
        || !entry.context_window_options.contains(&context_window)
    {
        println!(
            "{}",
            tr(
                "slash.context.unsupported",
                &[
                    ("model", &model),
                    ("tiers", &format!("{:?}", entry.context_window_options))
                ]
            )
        );
        return ChatCommandOutcome::Handled;
    }

    match apply_context_window(&ctx.global_services.model_prefs, &model, context_window) {
        Ok(()) => println!(
            "{}",
            tr(
                "slash.context.set",
                &[("model", &model), ("tokens", &context_window.to_string())]
            )
        ),
        Err(error) => println!(
            "{}",
            tr(
                "slash.setFailed",
                &[("command", "context"), ("detail", &error.to_string())]
            )
        ),
    }
    ChatCommandOutcome::Handled
}

fn usage_error(value: &str) -> ChatCommand {
    ChatCommand::UsageError {
        message: tr("slash.context.invalid", &[("value", value)]),
    }
}
