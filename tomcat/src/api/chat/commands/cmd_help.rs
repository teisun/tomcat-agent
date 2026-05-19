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
    "可用命令：\n  /path <绝对路径>           申请该路径的授权（弹出菜单：本次会话 / 写入配置 / 只读 / 禁止 / 取消）\n  /ckpt list [--limit N]     列出最近 checkpoint\n  /ckpt show <id>            查看 checkpoint 元数据\n  /ckpt diff <id>            查看 checkpoint 与当前工作区差异\n  /restore <id> [--path <rel>]... [--dry-run]  从 checkpoint 恢复整树或部分路径\n  /thinking [on|off|toggle]  折叠/展开模型 thinking 输出（缺省=toggle）\n  /plan \"<目标>\"             进入 PLAN 规划模式（落盘 ~/.tomcat/plans/）\n  /plan exit                 退回 Chat 模式\n  /plan build <plan_id>      进入 EXEC 执行模式\n  /plan list                 列出 ~/.tomcat/plans/ 下所有 plan\n  /help                      显示本帮助"
}
