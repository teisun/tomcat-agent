use super::super::ToolExecCtx;
use crate::core::tools::web_fetch::types::WebFetchArgs;

pub(in super::super) async fn handle_web_fetch(
    ctx: &ToolExecCtx<'_>,
    args: &serde_json::Value,
) -> Result<String, String> {
    let runtime = ctx
        .web_fetch_runtime
        .ok_or_else(|| "web_fetch runtime was not supplied".to_string())?;
    let parsed: WebFetchArgs = serde_json::from_value(args.clone())
        .map_err(|err| format!("Could not parse web_fetch arguments: {err}"))?;
    let output = runtime.fetch(parsed).await.map_err(|err| err.to_string())?;
    serde_json::to_string_pretty(&output)
        .map_err(|err| format!("Could not serialize web_fetch result: {err}"))
}
