//! `visible_tools_for_mode` — 按 PlanMode 过滤 LLM 可见工具集。
//!
//! 与 `core/tools/contract/catalog.rs` 的 `build_function_definitions` 全集（**含**
//! plan_only 工具）配对使用：chat_loop 装配 `tool_definitions` 时调用本函数，避免在
//! CHAT 期把 `create_plan` / `ask_question` 暴露给 LLM。
//!
//! 规则（plan-runtime.md §4.1 R6）：
//! - **Chat**：排除 `create_plan` / `ask_question` / `todos` / `update_plan`
//!   （CHAT 期不需要 plan 工具；用户用 `/plan` slash 切模式）
//! - **Planning**：包含 `create_plan` / `ask_question` / `todos` / `update_plan`；
//!   隐藏写盘/exec 工具中的 `write` / `edit` / `bash`（白名单仍可触达 `~/.tomcat/plans/*` 通过 plan 工具）
//! - **Executing { plan_id }**：包含 `todos` / `update_plan`；排除 `create_plan` / `ask_question`
//! - **Pending { .. }**：与 Chat 等价（pending 期不暴露 plan 工具，等用户 /plan build）
//! - **Completed { .. }**：与 Chat 等价（自动收口后回 CHAT 视图）

use serde_json::Value;

use super::mode::PlanMode;
use crate::core::tools::contract::catalog::BUILTIN_TOOL_CATALOG;

/// PLAN 模式期间隐藏的「执行向」工具名称（仅 Planning 模式生效）。
/// PR-PLF 还会在 executor 路径上额外拦截 `~/.tomcat/plans/*` 直写——这里只是 LLM-facing 过滤。
const HIDDEN_IN_PLANNING: &[&str] = &["write", "edit", "hashline_edit", "bash"];

/// EXEC 模式排除的 plan 工具（plan-runtime.md §4.1 R6：EXEC 不允许 create_plan / ask_question）。
const HIDDEN_IN_EXECUTING: &[&str] = &["create_plan", "ask_question"];

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

fn filter_for_mode(name: &str, plan_only: bool, mode: &PlanMode) -> bool {
    match mode {
        PlanMode::Chat | PlanMode::Pending { .. } | PlanMode::Completed { .. } => {
            // CHAT 等价视图：排除所有 plan_only 工具
            !plan_only
        }
        PlanMode::Planning => {
            if HIDDEN_IN_PLANNING.contains(&name) {
                return false;
            }
            // 含 plan_only 工具，但排除 EXEC 专属（todos 中的 EXEC-only 行为由 PlanRuntime 路由）
            // Planning 包含 create_plan / ask_question / todos / update_plan
            true
        }
        PlanMode::Executing { .. } => {
            if plan_only && HIDDEN_IN_EXECUTING.contains(&name) {
                return false;
            }
            // EXEC：保留 todos / update_plan；排除 create_plan / ask_question
            true
        }
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
    fn chat_mode_excludes_all_plan_only_tools() {
        let tools = visible_tools_for_mode(&PlanMode::Chat);
        let n = names(&tools);
        for plan_tool in ["create_plan", "update_plan", "todos", "ask_question"] {
            assert!(
                !n.contains(plan_tool),
                "CHAT mode must not expose {plan_tool}"
            );
        }
    }

    #[test]
    fn planning_mode_includes_plan_tools_and_excludes_writers() {
        let tools = visible_tools_for_mode(&PlanMode::Planning);
        let n = names(&tools);
        for plan_tool in ["create_plan", "update_plan", "todos", "ask_question"] {
            assert!(
                n.contains(plan_tool),
                "PLANNING must expose {plan_tool}, got: {n:?}"
            );
        }
        for hidden in HIDDEN_IN_PLANNING {
            assert!(
                !n.contains(*hidden),
                "PLANNING must hide writer tool {hidden}, got: {n:?}"
            );
        }
    }

    #[test]
    fn executing_mode_keeps_writers_and_excludes_create_plan_ask_question() {
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
        assert!(n.contains("write"), "EXEC must keep write");
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
