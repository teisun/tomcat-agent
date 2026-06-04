use super::super::ToolExecCtx;
use crate::core::tools::web_search::types::WebSearchArgs;

pub(in super::super) async fn handle_web_search(
    ctx: &ToolExecCtx<'_>,
    args: &serde_json::Value,
) -> Result<String, String> {
    let runtime = ctx
        .web_search_runtime
        .ok_or_else(|| "web_search runtime 未注入".to_string())?;
    let parsed: WebSearchArgs = serde_json::from_value(args.clone())
        .map_err(|err| format!("web_search 参数解析失败: {err}"))?;
    let output = runtime
        .search(parsed)
        .await
        .map_err(|err| err.to_string())?;
    serde_json::to_string_pretty(&output).map_err(|err| format!("web_search 结果序列化失败: {err}"))
}
