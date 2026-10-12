//! `dispatch_agent`：把只读勘察外包给一批 Explorer 子 Agent。
//!
//! 这里只做参数校验与结果渲染；真正的子 Agent 生命周期在
//! [`crate::core::plan_runtime::explorer`] 与 `prod_reviewer::ProdExplorerDispatcher`。

use crate::core::plan_runtime::explorer::{ExplorerTask, MAX_EXPLORER_TASKS};

use super::super::ToolExecCtx;

pub(in super::super) async fn handle_dispatch_agent(
    ctx: &ToolExecCtx<'_>,
    args: &serde_json::Value,
) -> Result<String, String> {
    let Some(runtime) = ctx.plan_runtime else {
        return Err("dispatch_agent is unavailable: PlanRuntime was not supplied".to_string());
    };
    let tasks = parse_tasks(args)?;
    let reports = runtime
        .dispatch_explorers(&tasks)
        .await
        .map_err(|e| e.to_string())?;
    Ok(crate::core::plan_runtime::explorer::render_reports(
        &reports,
    ))
}

fn parse_tasks(args: &serde_json::Value) -> Result<Vec<ExplorerTask>, String> {
    let raw = args
        .get("tasks")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "dispatch_agent requires a `tasks` array".to_string())?;
    // strict schema 下模型会补空壳条目；空 prompt 的任务派出去也只是浪费一个子 Agent。
    let raw: Vec<&serde_json::Value> = raw
        .iter()
        .filter(|item| {
            item.get("prompt")
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.trim().is_empty())
        })
        .collect();
    if raw.is_empty() {
        return Err("dispatch_agent `tasks` cannot be empty".to_string());
    }
    if raw.len() > MAX_EXPLORER_TASKS {
        return Err(format!(
            "dispatch_agent allows at most {MAX_EXPLORER_TASKS} tasks per call; received {}",
            raw.len()
        ));
    }
    let mut tasks = Vec::with_capacity(raw.len());
    let mut seen = std::collections::BTreeSet::new();
    for (idx, item) in raw.iter().enumerate() {
        let prompt = item
            .get("prompt")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("tasks[{idx}] requires a non-empty `prompt`"))?;
        let id = item
            .get("id")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("task-{}", idx + 1));
        // id 是主 Agent 把结论对回问题的唯一线索，重复就失去意义。
        if !seen.insert(id.clone()) {
            return Err(format!("Duplicate id `{id}` in tasks"));
        }
        tasks.push(ExplorerTask {
            id,
            prompt: prompt.to_string(),
        });
    }
    Ok(tasks)
}

#[cfg(test)]
#[path = "tests/dispatch_agent_test.rs"]
mod tests;
