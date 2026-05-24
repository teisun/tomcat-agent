use super::super::args::parse_hashline_edit_args;
use super::super::guard::check_mutation_stamp;
use super::super::{ToolDisplay, ToolExecCtx, AGENT_PLUGIN_ID};

pub(in super::super) async fn handle_hashline_edit(
    ctx: &ToolExecCtx<'_>,
    args: &serde_json::Value,
    display_out: &mut Option<ToolDisplay>,
) -> Result<String, String> {
    let (path, segments) = parse_hashline_edit_args(args)?;
    if let Some(state) = ctx.read_file_state {
        check_mutation_stamp(state, path, "edit")?;
    }
    ctx.primitive
        .hashline_edit(path, segments, AGENT_PLUGIN_ID)
        .await
        .map(|r| {
            if r.applied {
                *display_out = Some(ToolDisplay::File {
                    file: r.path.clone(),
                });
                format!("已 hashline 编辑: {}", r.path)
            } else {
                let msg = format!("hashline 编辑被拒绝: {}", r.path);
                *display_out = Some(ToolDisplay::Text { text: msg.clone() });
                msg
            }
        })
        .map_err(|e| e.to_string())
}
