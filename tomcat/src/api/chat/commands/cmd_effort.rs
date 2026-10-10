use crate::api::chat::ChatContext;
use crate::core::llm::ThinkingLevel;
use crate::infra::i18n::tr;
use crate::{AppError, ModelPrefsStore};

use super::parse::{ChatCommand, ChatCommandOutcome};

pub(crate) fn parse_args(tokens: Vec<String>) -> ChatCommand {
    match tokens.as_slice() {
        [_cmd, level] => match parse_effort_level(level) {
            Some(level) => ChatCommand::Effort { level },
            None => usage_error(level),
        },
        [_cmd] => ChatCommand::UsageError {
            message: tr("slash.effort.required", &[]),
        },
        _ => ChatCommand::UsageError {
            message: tr("slash.effort.usage", &[]),
        },
    }
}

pub fn parse_effort_level(level: &str) -> Option<ThinkingLevel> {
    ThinkingLevel::parse(level)
}

pub fn apply_level(
    store: &ModelPrefsStore,
    model: &str,
    level: ThinkingLevel,
) -> Result<(), AppError> {
    store.set_reasoning(model, level)
}

pub(crate) fn run(ctx: &ChatContext, level: ThinkingLevel) -> ChatCommandOutcome {
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
                    &[("command", "effort"), ("detail", &err.to_string())]
                )
            );
            return ChatCommandOutcome::Handled;
        }
    };
    let model = ctx.effective_model(entry.as_ref());
    match apply_level(&ctx.global_services.model_prefs, &model, level) {
        Ok(()) => {
            println!(
                "{}",
                tr(
                    "slash.effort.set",
                    &[("model", &model), ("level", level.as_str())]
                )
            );
        }
        Err(err) => {
            println!(
                "{}",
                tr(
                    "slash.setFailed",
                    &[("command", "effort"), ("detail", &err.to_string())]
                )
            );
        }
    }
    ChatCommandOutcome::Handled
}

fn usage_error(level: &str) -> ChatCommand {
    ChatCommand::UsageError {
        message: tr("slash.effort.invalid", &[("value", level)]),
    }
}
