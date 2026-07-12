use super::super::guard::check_mutation_stamp;
use super::super::{ToolDisplay, ToolExecCtx, AGENT_PLUGIN_ID};

pub(in super::super) async fn handle_write(
    ctx: &ToolExecCtx<'_>,
    args: &serde_json::Value,
    display_out: &mut Option<ToolDisplay>,
) -> Result<String, String> {
    let path = args["path"].as_str().unwrap_or("");
    let content = args["content"].as_str().unwrap_or("");
    let overwrite = args["overwrite"].as_bool().unwrap_or(false);
    let resolved = crate::infra::platform::normalize_path(path)
        .unwrap_or_else(|_| std::path::PathBuf::from(path));
    let exists = resolved.exists();
    if exists && !overwrite {
        return Err(format!(
            "Exists: 路径 `{}` 已存在；如需替换请先 `read` 该文件，然后再用 `overwrite=true` 调用 `write`",
            path
        ));
    }
    if exists && overwrite {
        if let Some(state) = ctx.read_file_state {
            check_mutation_stamp(state, path, "write")?;
        }
    }
    let result = ctx
        .primitive
        .write_file_with_cancel(path, content, overwrite, ctx.cancel, AGENT_PLUGIN_ID)
        .await;
    match result {
        Ok(r) => {
            if let Some(state) = ctx.read_file_state {
                state.invalidate(&resolved);
            }
            if r.written {
                *display_out = Some(ToolDisplay::File {
                    file: r.path.clone(),
                    added: r.added,
                    removed: r.removed,
                    diff: r.diff.clone(),
                });
                let verb = if r.diff_hint.is_some() {
                    "已覆盖"
                } else {
                    "已写入"
                };
                let mut msg = format!("{}: {} ({} bytes)", verb, r.path, r.bytes_written);
                if let Some(diff) = r.diff_hint.as_ref() {
                    if !diff.is_empty() {
                        msg.push_str("\n--- diff (truncated)\n");
                        msg.push_str(diff);
                    }
                }
                Ok(msg)
            } else {
                let msg = format!("写入被拒绝: {}", r.path);
                *display_out = Some(ToolDisplay::Text { text: msg.clone() });
                Ok(msg)
            }
        }
        Err(e) => Err(e.to_string()),
    }
}
