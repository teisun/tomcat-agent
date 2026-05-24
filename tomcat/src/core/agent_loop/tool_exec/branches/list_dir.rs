use super::super::{ToolExecCtx, AGENT_PLUGIN_ID};

pub(in super::super) async fn handle_list_dir(
    ctx: &ToolExecCtx<'_>,
    args: &serde_json::Value,
) -> Result<String, String> {
    let path = args["path"].as_str().unwrap_or("");
    ctx.primitive
        .list_dir(path, AGENT_PLUGIN_ID)
        .await
        .map(|entries| {
            let lines: Vec<String> = entries
                .iter()
                .map(|e| {
                    if e.is_dir {
                        format!("  {}/ (dir)", e.name)
                    } else {
                        format!("  {}", e.name)
                    }
                })
                .collect();
            lines.join("\n")
        })
        .map_err(|e| e.to_string())
}
