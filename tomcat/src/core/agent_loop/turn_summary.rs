//! TurnEnd 摘要标题：即时规则占位 + 异步 utility 覆盖。

use tracing::warn;

use crate::core::summary::{fallback_turn_summary, generate_turn_summary, ToolSnapshot};
use crate::infra::events::wire;

use super::types::{AgentLoop, ToolCallInfo};

fn tool_snapshots(tool_calls: &[ToolCallInfo]) -> Vec<ToolSnapshot> {
    tool_calls
        .iter()
        .map(|tc| {
            let args = serde_json::from_str(&tc.arguments).unwrap_or(serde_json::Value::Null);
            ToolSnapshot::from_tool_call(&tc.name, &args)
        })
        .collect()
}

/// TurnEnd 立即可用的规则占位摘要。
///
/// 只对有 tool 的回合生成；text-only 回合不为纯 thinking 额外制造折叠标题。
pub(super) fn resolve_turn_summary_title(tool_calls: &[ToolCallInfo]) -> Option<String> {
    if tool_calls.is_empty() {
        return None;
    }
    let title = fallback_turn_summary(&tool_snapshots(tool_calls));
    if title.trim().is_empty() {
        None
    } else {
        Some(title)
    }
}

/// fire-and-forget utility 标题覆盖。
///
/// - 不阻塞 `TurnEnd`
/// - 仅对有 tool 的回合发起
/// - 失败/超时/与规则占位相同都静默跳过
/// - 成功时同时回写 transcript message `summary_title`，并发事件通知前端热更新
pub(super) fn maybe_spawn_turn_summary_update(
    agent: &AgentLoop,
    assistant_message_id: Option<&str>,
    turn_index: usize,
    thinking_text: Option<String>,
    tool_calls: &[ToolCallInfo],
    current_summary_title: Option<&str>,
) {
    if tool_calls.is_empty() {
        return;
    }

    let model = agent.config.title_model.clone();
    if model.trim().is_empty() {
        return;
    }

    let llm = agent.title_provider();
    let emitter = agent.emitter.clone();
    let session_manager = agent.session_manager.clone();
    let session_id = agent.config.session_id.clone();
    let assistant_message_id = assistant_message_id.map(ToOwned::to_owned);
    let tool_call_ids: Vec<String> = tool_calls.iter().map(|tc| tc.id.clone()).collect();
    let tools = tool_snapshots(tool_calls);
    let current_summary_title = current_summary_title
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(ToOwned::to_owned);

    tokio::spawn(async move {
        let title =
            generate_turn_summary(thinking_text.as_deref(), &tools, llm.as_ref(), &model).await;
        let title = title.trim().to_string();
        if title.is_empty() {
            return;
        }
        if current_summary_title.as_deref() == Some(title.as_str()) {
            return;
        }

        if let (Some(session_manager), Some(message_id)) =
            (session_manager.as_ref(), assistant_message_id.as_deref())
        {
            if let Err(error) = session_manager.rewrite_message_summary_title_in_session(
                &session_id,
                message_id,
                &title,
            ) {
                warn!(
                    error = %error,
                    session_id = %session_id,
                    message_id = %message_id,
                    "rewrite turn summary title failed"
                );
            }
        }

        let payload = serde_json::json!({
            "type": wire::WIRE_TURN_SUMMARY_UPDATED,
            "turnIndex": turn_index,
            "assistantMessageId": assistant_message_id,
            "toolCallIds": tool_call_ids,
            "summaryTitle": title,
        });
        let _ = emitter.emit_payload(wire::WIRE_TURN_SUMMARY_UPDATED, payload);
    });
}
