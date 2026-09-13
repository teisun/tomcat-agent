use super::super::args::parse_hashline_edit_args;
use super::super::guard::{check_mutation_stamp, refresh_read_stamp};
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
        .hashline_edit_with_cancel(path, segments, ctx.cancel, AGENT_PLUGIN_ID)
        .await
        .map(|r| {
            if r.applied {
                if let Some(state) = ctx.read_file_state {
                    refresh_read_stamp(state, path, ctx.tool_call_id);
                }
                *display_out = Some(ToolDisplay::File {
                    file: r.path.clone(),
                    added: r.added,
                    removed: r.removed,
                    diff: r.diff.clone(),
                    diff_truncated: r.diff_truncated,
                    expired: false,
                });
                format!("已 hashline 编辑: {}", r.path)
            } else {
                // hashline 段哈希不匹配时，旧 read 结果不能再作为下一次编辑
                // 的依据。失效 stamp 让模型可获得一次真实的 refresh read，而不是
                // 被 FILE_UNCHANGED 去重短路。
                invalidate_read_stamp(ctx, path);
                let msg = format!("hashline 编辑被拒绝: {}", r.path);
                *display_out = Some(ToolDisplay::Text { text: msg.clone() });
                msg
            }
        })
        .map_err(|e| {
            invalidate_read_stamp(ctx, path);
            e.to_string()
        })
}

fn invalidate_read_stamp(ctx: &ToolExecCtx<'_>, path: &str) {
    let Some(state) = ctx.read_file_state else {
        return;
    };
    if let Ok(resolved) = crate::infra::platform::normalize_path(path) {
        state.invalidate(&resolved);
    }
}
