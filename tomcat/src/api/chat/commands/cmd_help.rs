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

pub(crate) fn help_text() -> String {
    let mut text = "可用命令：\n  /path <绝对路径>           申请该路径的授权（弹出菜单：本次会话 / 写入配置 / 只读 / 禁止 / 取消）\n  /model [current|list|use <id>]  查看当前模型、列出 catalog，或切换当前会话模型\n  /effort <low|medium|high|xhigh>  设置当前模型的思考深度（按 model 持久化）\n  /context <tokens>          选择当前模型支持的 Context 档位（按 model 持久化）\n  /skill list                列出当前会话已发现的 skill（含 user-only）\n  /skill reload              重扫技能目录并原子替换当前会话 SkillSet\n  /skill use <name> \"intent...\"  显式注入指定 skill 正文，并附上本轮意图\n  /connector list            查看 MCP server 的来源、连接状态和工具数\n  /connector add <name> <command> [args...]  写入全局 mcp.json\n  /connector trust-project   信任当前项目，允许项目 MCP 运行\n  /connector test|tools <name>  测试连接或查看工具\n  /connector remove <name>   从全局 mcp.json 移除 server\n  /connector reload          重读 mcp.json 并重新连接\n  /ckpt list [--limit N]     列出最近 checkpoint\n  /ckpt show <id>            查看 checkpoint 元数据\n  /ckpt diff <id>            查看 checkpoint 与当前工作区差异\n  /restore <id> [--path <rel>]... [--dry-run]  从 checkpoint 恢复整树或部分路径\n  /compact                   主动压缩当前上下文，保留摘要后继续会话\n  /thinking [minimal|summary|full|toggle]  切换 thinking 显示档位（缺省=toggle；兼容 on/off）\n  /plan                      进入 PLAN 规划模式（落盘 ~/.tomcat/plans/）\n  /plan exit                 退回 Chat 模式\n  /plan build <plan_id/path> 进入 EXEC 执行模式\n  /plan list                 列出 ~/.tomcat/plans/ 下所有 plan\n  /help                      显示本帮助".to_string();
    for command in super::shared::SHARED_SLASH_COMMANDS {
        text.push_str(&format!("\n  {}  {}", command.usage, command.summary));
    }
    text
}
