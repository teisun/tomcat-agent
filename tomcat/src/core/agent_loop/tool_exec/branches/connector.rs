//! Deferred connector tools (`tool_search` / `tool_describe` / `tool_call`).
//!
//! 这三个 builtin 是模型唯一稳定看见的连接器入口。它们查询 `McpManager` 的动态
//! 目录，但自身 schema 与存在性由 config 决定，因此 MCP Ready/NotReady 不会改 prompt。

use serde::Serialize;

use super::super::{media, ToolExecCtx, ToolExecOutcome};

const DEFAULT_PAGE_SIZE: usize = 20;
const MAX_PAGE_SIZE: usize = 100;

pub(in crate::core::agent_loop) async fn handle_tool_search(
    ctx: &ToolExecCtx<'_>,
    args: &serde_json::Value,
) -> ToolExecOutcome {
    let manager = match connector_manager(ctx) {
        Ok(manager) => manager,
        Err(error) => return ToolExecOutcome::err(error),
    };
    let source = match optional_nonempty_string(args, "source") {
        Ok(source) => source,
        Err(error) => return ToolExecOutcome::err(error),
    };
    let query = match optional_nonempty_string(args, "query") {
        Ok(query) => query,
        Err(error) => return ToolExecOutcome::err(error),
    };
    let limit = match page_size(args) {
        Ok(limit) => limit,
        Err(error) => return ToolExecOutcome::err(error),
    };
    let offset = match page_offset(args) {
        Ok(offset) => offset,
        Err(error) => return ToolExecOutcome::err(error),
    };

    let result = match (query.as_deref(), source.as_deref()) {
        (Some(query), source) => manager
            .search(query, source, limit.saturating_add(1), offset)
            .map(|matches| {
                let (matches, next_offset) = page(matches, limit);
                serde_json::json!({
                    "matches": matches,
                    "next_offset": next_offset.map(|next| offset + next),
                    "next": "Use tool_describe(names) to fetch schemas, then tool_call(name, arguments)."
                })
            }),
        (None, Some(source)) => manager.list_tools(source).map(|tools| {
            let (tools, next_offset) = page(tools.into_iter().skip(offset).collect(), limit);
            serde_json::json!({
                "source": source,
                "tools": tools,
                "next_offset": next_offset.map(|next| offset + next),
                "next": "Use tool_describe(names) to fetch schemas, then tool_call(name, arguments)."
            })
        }),
        (None, None) => {
            let (sources, next_offset) =
                page(manager.list_servers().into_iter().skip(offset).collect(), limit);
            Ok(serde_json::json!({
                "sources": sources,
                "next_offset": next_offset.map(|next| offset + next),
                "next": "Use tool_search(source=\"<name>\") to list one source's tools, or tool_search(query=\"keywords\") to search."
            }))
        }
    };
    result
        .map(json_outcome)
        .unwrap_or_else(|error| ToolExecOutcome::err(error.to_string()))
}

pub(in crate::core::agent_loop) async fn handle_tool_describe(
    ctx: &ToolExecCtx<'_>,
    args: &serde_json::Value,
) -> ToolExecOutcome {
    let manager = match connector_manager(ctx) {
        Ok(manager) => manager,
        Err(error) => return ToolExecOutcome::err(error),
    };
    let names = match required_names(args) {
        Ok(names) => names,
        Err(error) => return ToolExecOutcome::err(error),
    };
    let described = manager.describe_many(&names);
    let tools = described
        .tools
        .into_iter()
        .map(|tool| {
            serde_json::json!({
                "name": tool.model_name,
                "description": tool.description,
                "inputSchema": tool.input_schema,
            })
        })
        .collect::<Vec<_>>();
    json_outcome(serde_json::json!({
        "tools": tools,
        "errors": described.errors,
    }))
}

pub(in crate::core::agent_loop) async fn handle_tool_call(
    ctx: &ToolExecCtx<'_>,
    args: &serde_json::Value,
) -> ToolExecOutcome {
    let manager = match connector_manager(ctx) {
        Ok(manager) => manager,
        Err(error) => return ToolExecOutcome::err(error),
    };
    let name = match args.get("name").and_then(serde_json::Value::as_str) {
        Some(name) if !name.trim().is_empty() => name,
        _ => return ToolExecOutcome::err("tool_call requires a non-empty string 'name'"),
    };
    let arguments = args
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    if !arguments.is_object() {
        return ToolExecOutcome::err("tool_call 'arguments' must be a JSON object");
    }
    let call_context = crate::core::connector::mcp::manager::McpCallContext::new(
        ctx.session_id,
        ctx.tool_call_id,
        ctx.cancel,
    );
    match manager
        .call_model_tool_with_context(name, arguments, call_context)
        .await
    {
        Ok(result) => media::extract_mcp_tool_result_media(&result, ctx.openai_files_runtime).await,
        Err(error) => ToolExecOutcome::err(error.to_string()),
    }
}

fn connector_manager(
    ctx: &ToolExecCtx<'_>,
) -> Result<std::sync::Arc<crate::core::connector::mcp::manager::McpManager>, String> {
    ctx.connector_registry
        .map(|registry| registry.mcp_manager())
        .ok_or_else(|| {
            "connector tools are unavailable: no enabled MCP connector is configured".to_string()
        })
}

fn optional_nonempty_string(
    args: &serde_json::Value,
    field: &str,
) -> Result<Option<String>, String> {
    match args.get(field) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(value)) if !value.trim().is_empty() => {
            Ok(Some(value.trim().to_string()))
        }
        _ => Err(format!(
            "tool_search '{field}' must be a non-empty string when provided"
        )),
    }
}

fn page_size(args: &serde_json::Value) -> Result<usize, String> {
    match args.get("limit") {
        None | Some(serde_json::Value::Null) => Ok(DEFAULT_PAGE_SIZE),
        Some(value) => value
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
            .filter(|value| (1..=MAX_PAGE_SIZE).contains(value))
            .ok_or_else(|| {
                format!("tool_search 'limit' must be an integer between 1 and {MAX_PAGE_SIZE}")
            }),
    }
}

fn page_offset(args: &serde_json::Value) -> Result<usize, String> {
    match args.get("offset") {
        None | Some(serde_json::Value::Null) => Ok(0),
        Some(value) => value
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| "tool_search 'offset' must be a non-negative integer".to_string()),
    }
}

fn required_names(args: &serde_json::Value) -> Result<Vec<String>, String> {
    let values = args
        .get("names")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "tool_describe requires a non-empty string array 'names'".to_string())?;
    if values.is_empty() {
        return Err("tool_describe requires at least one tool name".to_string());
    }
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(ToOwned::to_owned)
                .ok_or_else(|| {
                    "tool_describe 'names' entries must be non-empty strings".to_string()
                })
        })
        .collect()
}

fn page<T>(mut items: Vec<T>, limit: usize) -> (Vec<T>, Option<usize>) {
    let has_next = items.len() > limit;
    items.truncate(limit);
    let next_offset = has_next.then_some(limit);
    (items, next_offset)
}

fn json_outcome(value: impl Serialize) -> ToolExecOutcome {
    match serde_json::to_string(&value) {
        Ok(text) => ToolExecOutcome::ok(text),
        Err(error) => ToolExecOutcome::err(format!("serialize connector tool result: {error}")),
    }
}
