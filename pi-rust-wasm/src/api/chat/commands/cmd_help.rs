//! `/help` command implementation.

use super::parse::{ChatCommand, ChatCommandOutcome};

pub(crate) fn parse_args(tokens: Vec<String>) -> ChatCommand {
    match tokens.as_slice() {
        [_cmd] => ChatCommand::Help,
        [_cmd, ..] => ChatCommand::UsageError {
            message: "用法错误：/help 不接受参数。".to_string(),
        },
        _ => ChatCommand::Help,
    }
}

pub(crate) fn run() -> ChatCommandOutcome {
    println!("{}", help_text());
    ChatCommandOutcome::Handled
}

pub(crate) fn help_text() -> &'static str {
    "可用命令：\n  /path <绝对路径>   申请该路径的授权（弹出菜单：本次会话 / 写入配置 / 只读 / 禁止 / 取消）\n  /help              显示本帮助"
}
