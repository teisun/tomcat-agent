use super::super::args::parse_edit_args;
use super::super::edit_sim::simulate_apply_edits;
use super::super::guard::check_mutation_stamp;
use super::super::{ToolDisplay, ToolExecCtx, AGENT_PLUGIN_ID};

pub(in super::super) async fn handle_edit(
    ctx: &ToolExecCtx<'_>,
    args: &serde_json::Value,
    display_out: &mut Option<ToolDisplay>,
) -> Result<String, String> {
    let (path, edits) = parse_edit_args(args)?;

    if crate::core::tools::pipeline::edit_normalize::is_unsupported_structured_file(path) {
        return Err(format!(
            "Notebook: `{}` 是 Jupyter 笔记本（.ipynb），edit 不支持；请使用专用 nbformat 工具或先把目标 cell 导出为 .py / .md 再 edit",
            path
        ));
    }
    if let Some(state) = ctx.read_file_state {
        check_mutation_stamp(state, path, "edit")?;
    }

    if ctx.subagent_type == crate::core::agent_loop::types::SubagentType::Reviewer
        && ctx.review_kind != Some(crate::core::plan_runtime::review::ReviewKind::Code)
    {
        let normalized_path = crate::infra::platform::normalize_path(path)
            .map_err(|e| format!("reviewer edit 预检路径解析失败：{e}"))?;
        let old = std::fs::read_to_string(&normalized_path)
            .map_err(|e| format!("reviewer edit 预检读原文失败：{e}"))?;
        let new = simulate_apply_edits(&old, &edits);
        if let Err(denied) = crate::core::plan_runtime::safety::reviewer_body_diff_guard(&old, &new)
        {
            return Err(format!("reviewer edit 被拒：{denied}"));
        }
    }

    ctx.primitive
        .edit_file_with_cancel(path, edits, ctx.cancel, AGENT_PLUGIN_ID)
        .await
        .map(|r| {
            if r.applied {
                *display_out = Some(ToolDisplay::File {
                    file: r.path.clone(),
                });
                format!("已编辑: {}", r.path)
            } else {
                let msg = format!("编辑被拒绝: {}", r.path);
                *display_out = Some(ToolDisplay::Text { text: msg.clone() });
                msg
            }
        })
        .map_err(|e| e.to_string())
}
