use crate::core::tools::primitive::{SearchFilesArgs, SearchFilesOutputMode};
use crate::infra::error::AppError;

use super::super::{ToolExecCtx, AGENT_PLUGIN_ID};

/// `output_mode=content` 时默认带的上下文行数。
const DEFAULT_CONTENT_CONTEXT_LINES: usize = 3;

pub(in super::super) async fn handle_search_files(
    ctx: &ToolExecCtx<'_>,
    args: &serde_json::Value,
) -> Result<String, String> {
    let mut search_args: SearchFilesArgs = match serde_json::from_value(args.clone()) {
        Ok(args) => args,
        Err(e) => return Err(format!("Could not parse search_files arguments: {e}")),
    };
    // 命中行单独看往往不够判断，模型接着就要 read 那一段——默认带 ±3 行省掉这次往返。
    // 显式传 context=0 可以关掉。
    if search_args.output_mode == SearchFilesOutputMode::Content && search_args.context.is_none() {
        search_args.context = Some(DEFAULT_CONTENT_CONTEXT_LINES);
    }
    ctx.primitive
        .search_files(search_args, AGENT_PLUGIN_ID)
        .await
        .and_then(|output| serde_json::to_string_pretty(&output).map_err(AppError::from))
        .map_err(|e| e.to_string())
}
