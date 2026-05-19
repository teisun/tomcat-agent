//! `visible_tools_for_mode` — 按 PlanMode 过滤 LLM 可见工具集。
//!
//! 与 `core/tools/contract/catalog.rs` 的 `build_function_definitions` 全集（**含**
//! plan_only 工具）配对使用：chat_loop 装配 `tool_definitions` 时调用本函数，避免在
//! CHAT 期把 `create_plan` / `ask_question` 暴露给 LLM。
//!
//! 规则（plan-runtime.md §4.1 R6 / 2026-05 调整）：
//! - **Chat / Pending / Completed**：保留 `todos` / `update_plan` / `ask_question`；
//!   **排除** `create_plan`（仅 PLAN 可创建新计划）
//! - **Planning**：包含 `create_plan` / `ask_question` / `todos` / `update_plan`；
//!   写工具（`write`/`edit`/`hashline_edit`/`delete`/`bash`）**全部保留**——写盘路径由
//!   [`safety::enforce_write_path_policy`] 在 `tool_exec` 路径层拦截到 `~/.tomcat/plans/*.plan.md`。
//! - **Executing { plan_id }**：包含 `todos` / `update_plan`；**排除** `create_plan` / `ask_question`；
//!   plan 文件全禁写由 `safety` 在路径层守护，推进任务仅走 `update_plan`。

use serde_json::Value;

use super::mode::PlanMode;
use crate::core::tools::contract::catalog::BUILTIN_TOOL_CATALOG;

/// EXEC 模式排除的 plan 工具（plan-runtime.md §4.1 R6：EXEC 不允许 create_plan / ask_question）。
const HIDDEN_IN_EXECUTING: &[&str] = &["create_plan", "ask_question"];

/// CHAT / Pending / Completed 视图排除的 plan 工具（仅 `create_plan`；`todos` / `update_plan` /
/// `ask_question` 在这些模式保留）。
const HIDDEN_IN_CHAT_VIEW: &[&str] = &["create_plan"];

/// 按 PlanMode 过滤生成 LLM 可见工具的 OpenAI function definition 列表。
///
/// 与 `build_function_definitions` 同 serde shape：
/// ```json
/// [{ "type": "function", "function": { "name": ..., "description": ..., "parameters": {...} } }]
/// ```
pub fn visible_tools_for_mode(mode: &PlanMode) -> Vec<Value> {
    BUILTIN_TOOL_CATALOG
        .iter()
        .filter(|entry| filter_for_mode(entry.name, entry.plan_only, mode))
        .map(|entry| {
            serde_json::json!({
                "type": "function",
                "function": {
                    "name": entry.name,
                    "description": entry.description,
                    "parameters": (entry.parameters)(),
                }
            })
        })
        .collect()
}

fn filter_for_mode(name: &str, _plan_only: bool, mode: &PlanMode) -> bool {
    match mode {
        PlanMode::Chat | PlanMode::Pending { .. } | PlanMode::Completed { .. } => {
            // CHAT 视图：仅排除 create_plan；保留 todos / update_plan / ask_question
            !HIDDEN_IN_CHAT_VIEW.contains(&name)
        }
        PlanMode::Planning => {
            // Planning：全集（含 create_plan / ask_question / todos / update_plan）；
            // 写工具不在 catalog 层屏蔽，由 safety::enforce_write_path_policy 在路径层拦截。
            true
        }
        PlanMode::Executing { .. } => !HIDDEN_IN_EXECUTING.contains(&name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(values: &[Value]) -> std::collections::BTreeSet<String> {
        values
            .iter()
            .map(|v| v["function"]["name"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn chat_mode_excludes_create_plan_only() {
        let tools = visible_tools_for_mode(&PlanMode::Chat);
        let n = names(&tools);
        assert!(
            !n.contains("create_plan"),
            "CHAT mode must hide create_plan"
        );
        for kept in ["update_plan", "todos", "ask_question", "write", "bash"] {
            assert!(n.contains(kept), "CHAT must expose {kept}, got: {n:?}");
        }
    }

    #[test]
    fn planning_mode_exposes_full_set_including_writers_and_bash() {
        let tools = visible_tools_for_mode(&PlanMode::Planning);
        let n = names(&tools);
        for plan_tool in ["create_plan", "update_plan", "todos", "ask_question"] {
            assert!(
                n.contains(plan_tool),
                "PLANNING must expose {plan_tool}, got: {n:?}"
            );
        }
        for kept in ["write", "edit", "bash"] {
            assert!(
                n.contains(kept),
                "PLANNING must NOT hide writer/bash at catalog layer (path policy guards it): {kept}, got: {n:?}"
            );
        }
    }

    #[test]
    fn executing_mode_excludes_create_plan_and_ask_question() {
        let tools = visible_tools_for_mode(&PlanMode::Executing {
            plan_id: "demo".into(),
        });
        let n = names(&tools);
        assert!(n.contains("update_plan"), "EXEC must keep update_plan");
        assert!(n.contains("todos"), "EXEC must keep todos");
        for hidden in HIDDEN_IN_EXECUTING {
            assert!(
                !n.contains(*hidden),
                "EXEC must hide {hidden}, got: {n:?}"
            );
        }
        assert!(n.contains("write"), "EXEC must keep write at catalog layer");
        assert!(n.contains("bash"), "EXEC must keep bash");
    }

    #[test]
    fn pending_mode_view_equals_chat_view() {
        let pending = visible_tools_for_mode(&PlanMode::Pending {
            plan_id: "demo".into(),
        });
        let chat = visible_tools_for_mode(&PlanMode::Chat);
        assert_eq!(names(&pending), names(&chat));
    }

    #[test]
    fn completed_mode_view_equals_chat_view() {
        let done = visible_tools_for_mode(&PlanMode::Completed {
            plan_id: "demo".into(),
        });
        let chat = visible_tools_for_mode(&PlanMode::Chat);
        assert_eq!(names(&done), names(&chat));
    }
}
