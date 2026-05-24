use crate::core::tools::primitive::SearchFilesArgs;
use crate::infra::error::AppError;

use super::super::{ToolExecCtx, AGENT_PLUGIN_ID};

pub(in super::super) async fn handle_search_files(
    ctx: &ToolExecCtx<'_>,
    args: &serde_json::Value,
) -> Result<String, String> {
    let search_args: SearchFilesArgs = match serde_json::from_value(args.clone()) {
        Ok(args) => args,
        Err(e) => return Err(format!("search_files 参数解析失败: {}", e)),
    };
    ctx.primitive
        .search_files(search_args, AGENT_PLUGIN_ID)
        .await
        .and_then(|output| serde_json::to_string_pretty(&output).map_err(AppError::from))
        .map_err(|e| e.to_string())
}
