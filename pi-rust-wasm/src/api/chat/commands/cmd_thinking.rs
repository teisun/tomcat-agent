//! `/thinking` command implementation.
//!
//! 在 `pi chat` 内运行时切换 `CliTurnRenderer` 的 thinking 折叠/展开开关。
//! 与 `PI_CHAT_SHOW_THINKING` 环境变量共用同一个 `Arc<AtomicBool>`：环境变量
//! 在 `ChatContext::from_config` 时设置进程初值，本命令负责对话期内的运行时切换。
//!
//! 详见 `docs/architecture/llm-stream-events-cli-pipeline.md` §4.2.4。

use std::sync::atomic::Ordering;

use crate::api::chat::ChatContext;

use super::parse::{ChatCommand, ChatCommandOutcome};

/// 子动作枚举：`/thinking on|off|toggle`，缺省（不带子命令）等价 `toggle`，
/// 与 openclaw `Ctrl+T` 的「按一下翻一次」语义对齐。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingAction {
    On,
    Off,
    Toggle,
}

pub(crate) fn parse_args(tokens: Vec<String>) -> ChatCommand {
    match tokens.as_slice() {
        [_cmd] => ChatCommand::Thinking {
            action: ThinkingAction::Toggle,
        },
        [_cmd, sub] => match sub.as_str() {
            "on" => ChatCommand::Thinking {
                action: ThinkingAction::On,
            },
            "off" => ChatCommand::Thinking {
                action: ThinkingAction::Off,
            },
            "toggle" => ChatCommand::Thinking {
                action: ThinkingAction::Toggle,
            },
            other => ChatCommand::UsageError {
                message: format!(
                    "用法错误：/thinking 仅支持 on/off/toggle，收到 `{}`。",
                    other
                ),
            },
        },
        _ => ChatCommand::UsageError {
            message: "用法错误：/thinking 仅接受 0 或 1 个参数（on/off/toggle）。".to_string(),
        },
    }
}

pub(crate) fn run(ctx: &ChatContext, action: ThinkingAction) -> ChatCommandOutcome {
    let new_value = apply_action(&ctx.show_thinking, action);
    let zh = if new_value { "已展开" } else { "已折叠" };
    let en = if new_value { "expanded" } else { "folded" };
    println!("[thinking] {} | thinking {}", zh, en);
    ChatCommandOutcome::Handled
}

/// 把 `ThinkingAction` 应用到 `show_thinking` AtomicBool；返回应用后的新值，便于测试。
pub fn apply_action(flag: &std::sync::atomic::AtomicBool, action: ThinkingAction) -> bool {
    match action {
        ThinkingAction::On => {
            flag.store(true, Ordering::Release);
            true
        }
        ThinkingAction::Off => {
            flag.store(false, Ordering::Release);
            false
        }
        ThinkingAction::Toggle => {
            // 用 fetch_xor 保证多线程并发时仍是严格翻一次（避免 load+store 间的竞争）。
            let prev = flag.fetch_xor(true, Ordering::AcqRel);
            !prev
        }
    }
}
