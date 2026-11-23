//! Parse the complete request before admitting any child. Only Explorer is dispatchable.
use super::super::ToolExecCtx;
use crate::core::plan_runtime::explorer::{render_reports, ExplorerDelivery, ExplorerTask};

pub(in super::super) async fn handle_dispatch_agent(
    ctx: &ToolExecCtx<'_>,
    args: &serde_json::Value,
) -> Result<String, String> {
    let (tasks, background) = parse_tasks(args)?;
    let runtime = ctx
        .plan_runtime
        .ok_or("dispatch_agent unavailable without a parent runtime")?;
    match runtime
        .dispatch_tool_batch(&tasks, background, Some(ctx.tool_call_id))
        .await?
    {
        ExplorerDelivery::Reports(reports) => Ok(render_reports(&reports)),
        ExplorerDelivery::Tickets(tickets) => {
            serde_json::to_string(&tickets).map_err(|e| e.to_string())
        }
    }
}

fn parse_tasks(args: &serde_json::Value) -> Result<(Vec<ExplorerTask>, bool), String> {
    let raw = args["tasks"]
        .as_array()
        .filter(|v| !v.is_empty())
        .ok_or("tasks must be a non-empty array")?;
    let background = args
        .get("run_in_background")
        .map(|v| v.as_bool().ok_or("run_in_background must be boolean"))
        .transpose()?
        .unwrap_or(false);
    let mut tasks = Vec::with_capacity(raw.len());
    for (i, item) in raw.iter().enumerate() {
        if item["type"] != "explorer" {
            return Err(format!("tasks[{i}].type must be explorer"));
        }
        if item.get("id").is_some() {
            return Err(format!(
                "tasks[{i}]: explorer cannot resume a child; omit id"
            ));
        }
        let field = |key: &str| {
            item[key]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| format!("tasks[{i}] needs a non-empty {key}"))
        };
        let description = field("description")?;
        if description.chars().any(char::is_control) || description.chars().count() > 120 {
            return Err(format!(
                "tasks[{i}].description must be a single line of at most 120 characters"
            ));
        }
        tasks.push(ExplorerTask {
            description: description.into(),
            prompt: field("prompt")?.into(),
        });
    }
    Ok((tasks, background))
}

#[cfg(test)]
#[path = "tests/dispatch_agent_test.rs"]
mod tests;
