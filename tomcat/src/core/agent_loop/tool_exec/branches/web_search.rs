use super::super::ToolExecCtx;
use crate::core::tools::web_search::types::WebSearchArgs;

pub(in super::super) async fn handle_web_search(
    ctx: &ToolExecCtx<'_>,
    args: &serde_json::Value,
) -> Result<String, String> {
    let runtime = ctx
        .web_search_runtime
        .ok_or_else(|| "web_search runtime was not supplied".to_string())?;
    let parsed: WebSearchArgs = serde_json::from_value(args.clone())
        .map_err(|err| format!("Could not parse web_search arguments: {err}"))?;
    let output = runtime
        .search(parsed, ctx.session_id)
        .await
        .map_err(|err| err.to_string())?;
    serde_json::to_string_pretty(&output)
        .map_err(|err| format!("Could not serialize web_search result: {err}"))
}
