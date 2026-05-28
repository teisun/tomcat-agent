//! `/thinking` command implementation.
//!
//! 在 `tomcat chat` 内运行时切换 `CliTurnRenderer` 的 thinking 显示档位。
//! 与 `PI_CHAT_SHOW_THINKING` 环境变量共用同一个 `Arc<AtomicU8>`：环境变量
//! 在 `ChatContext::from_config` 时设置进程初值，本命令负责对话期内的运行时切换。
//!
//! 详见 `docs/architecture/llm-stream-events-cli-pipeline.md` §4.2.4。

use std::sync::atomic::{AtomicU8, Ordering};

use crate::api::chat::ChatContext;
use crate::infra::config::ThinkingDisplay;

use super::parse::{ChatCommand, ChatCommandOutcome};

/// 子动作枚举：`/thinking minimal|summary|full|toggle`。
///
/// 为兼容历史脚本，`on` 视为 `full`，`off` 视为 `summary`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingAction {
    Minimal,
    Summary,
    Full,
    Toggle,
}

pub(crate) fn parse_args(tokens: Vec<String>) -> ChatCommand {
    match tokens.as_slice() {
        [_cmd] => ChatCommand::Thinking {
            action: ThinkingAction::Toggle,
        },
        [_cmd, sub] => match sub.as_str() {
            "minimal" => ChatCommand::Thinking {
                action: ThinkingAction::Minimal,
            },
            "summary" => ChatCommand::Thinking {
                action: ThinkingAction::Summary,
            },
            "full" | "on" => ChatCommand::Thinking {
                action: ThinkingAction::Full,
            },
            "off" => ChatCommand::Thinking {
                action: ThinkingAction::Summary,
            },
            "toggle" => ChatCommand::Thinking {
                action: ThinkingAction::Toggle,
            },
            other => ChatCommand::UsageError {
                message: format!(
                    "用法错误：/thinking 仅支持 minimal/summary/full/toggle（兼容 on/off），收到 `{}`。",
                    other
                ),
            },
        },
        _ => ChatCommand::UsageError {
            message: "用法错误：/thinking 仅接受 0 或 1 个参数（minimal/summary/full/toggle）。"
                .to_string(),
        },
    }
}

pub(crate) fn run(ctx: &ChatContext, action: ThinkingAction) -> ChatCommandOutcome {
    let new_value = apply_action(&ctx.thinking_display, action);
    println!("[thinking] 已切换到 {} 模式", display_name(new_value));
    ChatCommandOutcome::Handled
}

/// 把 `ThinkingAction` 应用到 `thinking_display`；返回应用后的新档位，便于测试。
pub fn apply_action(flag: &AtomicU8, action: ThinkingAction) -> ThinkingDisplay {
    match action {
        ThinkingAction::Minimal => {
            flag.store(ThinkingDisplay::Minimal.as_u8(), Ordering::Release);
            ThinkingDisplay::Minimal
        }
        ThinkingAction::Summary => {
            flag.store(ThinkingDisplay::Summary.as_u8(), Ordering::Release);
            ThinkingDisplay::Summary
        }
        ThinkingAction::Full => {
            flag.store(ThinkingDisplay::Full.as_u8(), Ordering::Release);
            ThinkingDisplay::Full
        }
        ThinkingAction::Toggle => loop {
            let prev_raw = flag.load(Ordering::Acquire);
            let prev = ThinkingDisplay::from_u8(prev_raw);
            let next = prev.next_cycle();
            if flag
                .compare_exchange(prev_raw, next.as_u8(), Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return next;
            }
        },
    }
}

fn display_name(mode: ThinkingDisplay) -> &'static str {
    match mode {
        ThinkingDisplay::Minimal => "minimal",
        ThinkingDisplay::Summary => "summary",
        ThinkingDisplay::Full => "full",
    }
}
