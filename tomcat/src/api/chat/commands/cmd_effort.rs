use crate::api::chat::ChatContext;
use crate::core::llm::ThinkingLevel;
use crate::{AppError, ModelThinkingStore};

use super::parse::{ChatCommand, ChatCommandOutcome};

pub(crate) fn parse_args(tokens: Vec<String>) -> ChatCommand {
    match tokens.as_slice() {
        [_cmd, level] => match parse_effort_level(level) {
            Some(level) => ChatCommand::Effort { level },
            None => usage_error(level),
        },
        [_cmd] => ChatCommand::UsageError {
            message: "用法错误：/effort 需要一个参数：off|minimal|low|medium|high|xhigh|max。"
                .to_string(),
        },
        _ => ChatCommand::UsageError {
            message: "用法错误：/effort 仅支持 off|minimal|low|medium|high|xhigh|max。".to_string(),
        },
    }
}

pub fn parse_effort_level(level: &str) -> Option<ThinkingLevel> {
    ThinkingLevel::parse(level)
}

pub fn apply_level(
    store: &ModelThinkingStore,
    model: &str,
    level: ThinkingLevel,
) -> Result<(), AppError> {
    store.set(model, level)
}

pub(crate) fn run(ctx: &ChatContext, level: ThinkingLevel) -> ChatCommandOutcome {
    let entry = match ctx
        .session_runtime
        .session
        .get_session(ctx.session_runtime.session.current_session_key())
    {
        Ok(entry) => entry,
        Err(err) => {
            println!("[effort] 读取当前会话失败: {}", err);
            return ChatCommandOutcome::Handled;
        }
    };
    let model = ctx.effective_model(entry.as_ref());
    match apply_level(&ctx.global_services.model_thinking, &model, level) {
        Ok(()) => {
            println!(
                "[effort] 模型 {} 的思考深度已设为 {}",
                model,
                level.as_str()
            );
        }
        Err(err) => {
            println!("[effort] 设置失败: {}", err);
        }
    }
    ChatCommandOutcome::Handled
}

fn usage_error(level: &str) -> ChatCommand {
    ChatCommand::UsageError {
        message: format!(
            "用法错误：/effort 仅支持 off|minimal|low|medium|high|xhigh|max，收到 `{}`。",
            level
        ),
    }
}
