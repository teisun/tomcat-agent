//! TurnEnd 摘要标题解析（utility 模型 + 规则回退）。

use crate::core::summary::{fallback_turn_summary, generate_turn_summary, ToolSnapshot};

use super::types::{AgentLoop, ToolCallInfo};

pub(super) async fn resolve_turn_summary_title(
    agent: &AgentLoop,
    thinking_text: Option<&str>,
    tool_calls: &[ToolCallInfo],
) -> Option<String> {
    let has_thinking = thinking_text.is_some_and(|t| !t.trim().is_empty());
    let has_tools = !tool_calls.is_empty();
    if !has_thinking && !has_tools {
        return None;
    }

    let tools: Vec<ToolSnapshot> = tool_calls
        .iter()
        .map(|tc| {
            let args = serde_json::from_str(&tc.arguments).unwrap_or(serde_json::Value::Null);
            ToolSnapshot::from_tool_call(&tc.name, &args)
        })
        .collect();

    let model = agent.config.title_model.clone();
    if model.trim().is_empty() {
        // utility title 模型不可用（未配置/未解析）→ 静默规则回退，不发起 LLM 调用、不阻塞主 chat 流。
        let fallback = fallback_turn_summary(&tools);
        return if fallback.trim().is_empty() {
            None
        } else {
            Some(fallback)
        };
    }

    let llm = agent.title_provider();
    let title = generate_turn_summary(thinking_text, &tools, llm.as_ref(), &model).await;
    if title.trim().is_empty() {
        None
    } else {
        Some(title)
    }
}
