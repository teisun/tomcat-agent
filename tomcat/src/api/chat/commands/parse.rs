//! # Chat command parsing and dispatch
//!
//! `tomcat chat` supports a small set of local commands before a line is sent to the
//! LLM:
//!
//! - `/path <path>` asks for access to one path through the existing permission menu.
//! - `/effort low|medium|high|xhigh` updates the current model's reasoning effort.
//! - `/model ...`, `/plan ...`, `/skill ...`, `/ckpt ...`, `/restore ...` manage
//!   session-local runtime state.
//! - `/help` prints the command list.
//!
//! Command names are intentionally case-sensitive and lowercase-only. Unknown
//! slash-prefixed lines remain ordinary chat input.

use std::path::PathBuf;

use crate::api::chat::ChatContext;

use super::{
    cmd_ckpt, cmd_compact, cmd_connector, cmd_context, cmd_effort, cmd_help, cmd_model, cmd_path,
    cmd_plan, cmd_restore, cmd_skill, cmd_thinking, shared,
};

pub use cmd_connector::ConnectorCommand;
pub use cmd_model::ModelCommand;
pub use cmd_plan::PlanCommand;
pub use cmd_skill::SkillCommand;
pub use cmd_thinking::ThinkingAction;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatCommand {
    /// Not a recognized local command; send the original line to the LLM.
    NotACommand(String),
    /// `/path <path>` with exactly one path argument.
    Path {
        path: PathBuf,
        original_line: String,
    },
    /// `/help`.
    Help,
    /// `/compact`：主动创建一个持久化上下文摘要。
    Compact,
    /// `/thinking minimal|summary|full|toggle`：切换 CliTurnRenderer 的显示档位。
    Thinking {
        action: ThinkingAction,
    },
    /// `/effort low|medium|high|xhigh`：设置当前模型的 reasoning effort。
    Effort {
        level: crate::core::llm::ThinkingLevel,
    },
    /// `/context 400000`：为当前模型选择一个受支持的 Context 档位。
    Context {
        context_window: u32,
    },
    /// `/model current|list|use <id>`：查看/切换当前会话模型。
    Model(ModelCommand),
    /// `/ckpt list|show|diff`.
    CkptList {
        limit: Option<usize>,
    },
    CkptShow {
        checkpoint_id: String,
    },
    CkptDiff {
        checkpoint_id: String,
    },
    /// `/restore <id> [--path <rel>]... [--dry-run]`.
    Restore {
        checkpoint_id: String,
        paths: Vec<PathBuf>,
        dry_run: bool,
    },
    /// `/plan` 子命令族（plan-runtime.md §4.1 R1）。
    Plan(PlanCommand),
    /// A command from the shared resource-command table.
    Shared {
        name: String,
        args: Vec<String>,
    },
    Command(String),
    /// `/skill` 子命令族：列出 / 重载 / 显式注入技能正文。
    Skill(SkillCommand),
    /// `/connector` 子命令族：管理 MCP server。
    Connector(ConnectorCommand),
    /// Recognized command with invalid arguments.
    UsageError {
        message: String,
    },
}

pub(crate) enum ChatCommandOutcome {
    /// Send `line` to the LLM as the current user turn.
    Continue {
        line: String,
        echo_user: bool,
        history_line: Option<String>,
    },
    UserMessage {
        message: crate::core::llm::ChatMessage,
        history_line: String,
    },
    /// Command was fully handled locally; skip the current turn.
    Handled,
    /// Disk restore revived a pending ask_question; resume without new input.
    ResumePendingQuestion,
}

pub fn parse_chat_command(line: &str) -> ChatCommand {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return ChatCommand::NotACommand(line.to_string());
    }

    if let Some(parsed) = shared::parse_shared_slash(trimmed) {
        return match parsed {
            Ok((name, args)) => ChatCommand::Shared { name, args },
            Err(message) => ChatCommand::UsageError { message },
        };
    }

    let first_token = trimmed.split_whitespace().next().unwrap_or_default();
    if !matches!(
        first_token,
        "/path"
            | "/help"
            | "/compact"
            | "/thinking"
            | "/effort"
            | "/context"
            | "/model"
            | "/ckpt"
            | "/restore"
            | "/plan"
            | "/skill"
            | "/connector"
            | "/command"
    ) {
        return ChatCommand::NotACommand(line.to_string());
    }

    let tokens = match shell_words::split(trimmed) {
        Ok(tokens) => tokens,
        Err(e) => {
            return ChatCommand::UsageError {
                message: format!("命令参数解析失败：{}", e),
            };
        }
    };

    match first_token {
        "/path" => cmd_path::parse_args(tokens, trimmed),
        "/help" => cmd_help::parse_args(tokens),
        "/compact" => cmd_compact::parse_args(tokens),
        "/thinking" => cmd_thinking::parse_args(tokens),
        "/effort" => cmd_effort::parse_args(tokens),
        "/context" => cmd_context::parse_args(tokens),
        "/model" => cmd_model::parse_args(tokens),
        "/ckpt" => parse_ckpt_args(tokens),
        "/restore" => parse_restore_args(tokens),
        "/plan" => cmd_plan::parse_args(tokens),
        "/skill" => cmd_skill::parse_args(tokens),
        "/command" => ChatCommand::Command(trimmed.to_string()),
        "/connector" => cmd_connector::parse_args(tokens),
        _ => ChatCommand::NotACommand(line.to_string()),
    }
}

pub(crate) async fn dispatch_chat_command(
    ctx: &ChatContext,
    command: ChatCommand,
    rl: &mut rustyline::DefaultEditor,
    context_state: &mut crate::core::session::manager::ContextState,
    system_text: &str,
) -> ChatCommandOutcome {
    match command {
        ChatCommand::NotACommand(line) => super::cmd_command::shortcut(ctx, line),
        ChatCommand::Command(line) => super::cmd_command::run(ctx, line),
        ChatCommand::Help => cmd_help::run(),
        ChatCommand::Compact => cmd_compact::run(ctx, context_state, system_text).await,
        ChatCommand::UsageError { message } => {
            println!("{}\n\n{}", message, cmd_help::help_text());
            ChatCommandOutcome::Handled
        }
        ChatCommand::Path {
            path,
            original_line,
        } => cmd_path::run(ctx, path, original_line, rl),
        ChatCommand::Thinking { action } => cmd_thinking::run(ctx, action),
        ChatCommand::Effort { level } => cmd_effort::run(ctx, level),
        ChatCommand::Context { context_window } => cmd_context::run(ctx, context_window),
        ChatCommand::Model(model_cmd) => cmd_model::run(ctx, model_cmd),
        ChatCommand::CkptList { limit } => cmd_ckpt::run_list(ctx, limit),
        ChatCommand::CkptShow { checkpoint_id } => cmd_ckpt::run_show(ctx, checkpoint_id),
        ChatCommand::CkptDiff { checkpoint_id } => cmd_ckpt::run_diff(ctx, checkpoint_id),
        ChatCommand::Restore {
            checkpoint_id,
            paths,
            dry_run,
        } => cmd_restore::run(
            ctx,
            checkpoint_id,
            paths,
            dry_run,
            context_state,
            system_text,
        ),
        ChatCommand::Plan(plan_cmd) => cmd_plan::run(ctx, plan_cmd),
        ChatCommand::Shared { name, args } => shared::run_terminal_shared(ctx, &name, args).await,
        ChatCommand::Skill(skill_cmd) => cmd_skill::run(ctx, skill_cmd).await,
        ChatCommand::Connector(connector_cmd) => cmd_connector::run(ctx, connector_cmd).await,
    }
}

fn parse_ckpt_args(tokens: Vec<String>) -> ChatCommand {
    match tokens.as_slice() {
        [cmd] if cmd == "/ckpt" => ChatCommand::UsageError {
            message: "用法错误：/ckpt list [--limit N] | /ckpt show <id> | /ckpt diff <id>"
                .to_string(),
        },
        [cmd, sub] if cmd == "/ckpt" && sub == "list" => ChatCommand::CkptList { limit: None },
        [cmd, sub, flag, value] if cmd == "/ckpt" && sub == "list" && flag == "--limit" => {
            match value.parse::<usize>() {
                Ok(limit) if limit > 0 => ChatCommand::CkptList { limit: Some(limit) },
                _ => ChatCommand::UsageError {
                    message: "用法错误：/ckpt list [--limit N]，其中 N 必须是正整数。".to_string(),
                },
            }
        }
        [cmd, sub, checkpoint_id] if cmd == "/ckpt" && sub == "show" => ChatCommand::CkptShow {
            checkpoint_id: checkpoint_id.to_string(),
        },
        [cmd, sub, checkpoint_id] if cmd == "/ckpt" && sub == "diff" => ChatCommand::CkptDiff {
            checkpoint_id: checkpoint_id.to_string(),
        },
        _ => ChatCommand::UsageError {
            message: "用法错误：/ckpt list [--limit N] | /ckpt show <id> | /ckpt diff <id>"
                .to_string(),
        },
    }
}

fn parse_restore_args(tokens: Vec<String>) -> ChatCommand {
    if tokens.len() < 2 {
        return ChatCommand::UsageError {
            message: "用法错误：/restore <ck_id> [--path <rel>]... [--dry-run]".to_string(),
        };
    }
    if tokens[0] != "/restore" {
        return ChatCommand::NotACommand(tokens.join(" "));
    }

    let checkpoint_id = tokens[1].clone();
    let mut idx = 2usize;
    let mut paths = Vec::new();
    let mut dry_run = false;
    while idx < tokens.len() {
        match tokens[idx].as_str() {
            "--dry-run" => {
                dry_run = true;
                idx += 1;
            }
            "--path" => {
                let Some(path) = tokens.get(idx + 1) else {
                    return ChatCommand::UsageError {
                        message: "用法错误：/restore <ck_id> [--path <rel>]... [--dry-run]"
                            .to_string(),
                    };
                };
                paths.push(PathBuf::from(path));
                idx += 2;
            }
            _ => {
                return ChatCommand::UsageError {
                    message: "用法错误：/restore <ck_id> [--path <rel>]... [--dry-run]".to_string(),
                };
            }
        }
    }

    ChatCommand::Restore {
        checkpoint_id,
        paths,
        dry_run,
    }
}
