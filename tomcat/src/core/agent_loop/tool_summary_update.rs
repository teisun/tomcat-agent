//! 单条工具（bash）卡片标题：命令执行后异步 utility 覆盖。
//!
//! 与 [`super::turn_summary`] 同款「先占位、后升级」思路，但粒度是单个 tool call：
//! - 不阻塞命令执行（`tokio::spawn` fire-and-forget）
//! - 仅对 shell 类工具（`bash` / `shell` / `execute_command`）发起
//! - 生成"目的短句"后经 `tool.summary_updated` 事件按 `toolCallId` 热更新前端
//! - 仅 live 生效，不回写 transcript（历史重载回落客户端确定性占位）

use crate::core::summary::generate_command_summary;
use crate::infra::events::wire;

use super::types::AgentLoop;

/// bash 命令输出送给 utility 的上下文上限（够判目的即可，避免长输出拖慢/涨 token）。
const OUTPUT_EXCERPT_MAX_CHARS: usize = 600;

fn is_command_tool(tool_name: &str) -> bool {
    matches!(tool_name, "bash" | "shell" | "execute_command")
}

/// 从 tool args 里拼出用于生成"目的"的命令串（`command` + 可选 `args` 数组）。
fn command_for_summary(args: &serde_json::Value) -> String {
    let command = args
        .get("command")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .trim();
    let argv: Vec<&str> = args
        .get("args")
        .and_then(serde_json::Value::as_array)
        .map(|items| items.iter().filter_map(serde_json::Value::as_str).collect())
        .unwrap_or_default();
    if argv.is_empty() {
        command.to_string()
    } else if command.is_empty() {
        argv.join(" ")
    } else {
        format!("{command} {}", argv.join(" "))
    }
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    text.chars().take(max).collect()
}

/// fire-and-forget：对 shell 类工具异步生成标题并 emit `tool.summary_updated`。
///
/// 失败/超时/命令为空/非 shell 工具都静默跳过（前端保留客户端占位标题）。
pub(super) fn maybe_spawn_tool_summary_update(
    agent: &AgentLoop,
    tool_call_id: &str,
    tool_name: &str,
    args: &serde_json::Value,
    result_text: &str,
) {
    if !is_command_tool(tool_name) {
        return;
    }
    let command = command_for_summary(args);
    if command.trim().is_empty() {
        return;
    }
    let model = agent.config.title_model.clone();
    if model.trim().is_empty() {
        return;
    }

    let llm = agent.title_provider();
    let emitter = agent.emitter.clone();
    let tool_call_id = tool_call_id.to_string();
    let output_excerpt = truncate_chars(result_text.trim(), OUTPUT_EXCERPT_MAX_CHARS);

    tokio::spawn(async move {
        let excerpt = if output_excerpt.is_empty() {
            None
        } else {
            Some(output_excerpt.as_str())
        };
        let title = generate_command_summary(&command, excerpt, llm.as_ref(), &model).await;
        let title = title.trim().to_string();
        if title.is_empty() {
            return;
        }
        let payload = serde_json::json!({
            "type": wire::WIRE_TOOL_SUMMARY_UPDATED,
            "toolCallId": tool_call_id,
            "summaryTitle": title,
        });
        let _ = emitter.emit_payload(wire::WIRE_TOOL_SUMMARY_UPDATED, payload);
    });
}
