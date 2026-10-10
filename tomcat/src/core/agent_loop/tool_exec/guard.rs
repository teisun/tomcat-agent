use std::sync::Arc;

pub(super) fn is_plan_reviewer_whitelisted_tool(name: &str, expose_skills: bool) -> bool {
    crate::core::plan_runtime::plan_reviewer::PLAN_REVIEWER_ALLOWED_TOOLS.contains(&name)
        || (expose_skills && name == "load_skill")
}

pub(super) fn is_code_reviewer_whitelisted_tool(name: &str, expose_skills: bool) -> bool {
    crate::core::plan_runtime::code_reviewer::CODE_REVIEWER_ALLOWED_TOOLS.contains(&name)
        || (expose_skills && name == "load_skill")
}

pub(super) fn reviewer_allowed_tools_description(
    subagent_type: crate::core::agent_loop::types::SubagentType,
    expose_skills: bool,
) -> String {
    let mut desc = match subagent_type {
        crate::core::agent_loop::types::SubagentType::PlanReviewer => {
            "read/search_files/list_dir/todos/update_plan/edit".to_string()
        }
        crate::core::agent_loop::types::SubagentType::CodeReviewer => {
            "read/search_files/list_dir/bash".to_string()
        }
        _ => String::new(),
    };
    if expose_skills {
        desc.push_str("/load_skill");
    }
    desc
}

/// Explorer 只读：写工具、plan 工具一律不可用；也不能再套娃派发 explorer。
pub(super) fn is_explorer_whitelisted_tool(name: &str) -> bool {
    crate::core::plan_runtime::explorer::EXPLORER_ALLOWED_TOOLS.contains(&name)
}

pub(super) fn explorer_allowed_tools_description() -> String {
    crate::core::plan_runtime::explorer::EXPLORER_ALLOWED_TOOLS.join("/")
}

pub(super) fn is_verifier_whitelisted_tool(name: &str, expose_skills: bool) -> bool {
    matches!(
        name,
        "read" | "search_files" | "list_dir" | "bash" | "web_fetch"
    ) || (expose_skills && name == "load_skill")
}

pub(super) fn verifier_allowed_tools_description(expose_skills: bool) -> String {
    let mut desc = "read/search_files/list_dir/bash/web_fetch".to_string();
    if expose_skills {
        desc.push_str("/load_skill");
    }
    desc
}

pub(super) fn check_mutation_stamp(
    state: &Arc<crate::core::tools::pipeline::read_state::ReadFileState>,
    path: &str,
    op_label: &str,
) -> Result<(), String> {
    let resolved = match crate::infra::platform::normalize_path(path) {
        Ok(p) => p,
        Err(_) => return Ok(()),
    };
    let Some(stamp) = state.get(&resolved) else {
        return Err(format!("NoPriorRead: This session has not read `{path}`; blind writes/edits are forbidden. Use `read` before `{op_label}`"));
    };
    let Ok(meta) = std::fs::metadata(&resolved) else {
        return Ok(());
    };
    if meta.is_dir() {
        return Err(format!(
            "{op_label}: target `{path}` is a directory and cannot be used as input"
        ));
    }
    let cur_mtime = crate::core::tools::pipeline::read_state::metadata_mtime_ms(&meta);
    if stamp.mtime_ms != cur_mtime || stamp.size != meta.len() {
        // 只有完整、无窗口的文本 read 才会保存整份文件的哈希；分窗结果的 hash
        // 只是那一段文本，不能拿它冒充全文件指纹。快速指纹不一致时，给这类完整
        // read 一次内容相等的机会，避免 touch / 时间戳精度导致的假 Stale。
        let full_file_was_read = stamp.offset.is_none()
            && stamp.limit.is_none()
            && stamp.covered_lines.is_some_and(|(start, _)| start == 1)
            && stamp.reached_eof;
        if full_file_was_read
            && std::fs::read(&resolved).ok().is_some_and(|content| {
                crate::core::tools::pipeline::read_state::hash_content(&content)
                    == stamp.content_hash
            })
        {
            return Ok(());
        }
        return Err(format!("Stale: File `{path}` changed since the last read (mtime/size mismatch); read it again before `{op_label}`"));
    }
    Ok(())
}

/// 记录本进程刚刚成功落盘后的新版本。
///
/// 这不是把文件全文重新塞进模型上下文：`is_partial_view=false` 与
/// `reached_eof=false` 明确禁止 read 去走“未变化”短路。它只更新 mutation guard
/// 需要的元数据，避免 agent 自己刚完成的 edit 被下一次 edit 误判成外部 Stale。
pub(super) fn refresh_read_stamp(
    state: &Arc<crate::core::tools::pipeline::read_state::ReadFileState>,
    path: &str,
    tool_call_id: &str,
) {
    if !state.mutation_stamp_refresh_enabled() {
        return;
    }
    let Ok(resolved) = crate::infra::platform::normalize_path(path) else {
        return;
    };
    let Ok(meta) = std::fs::metadata(&resolved) else {
        return;
    };
    if meta.is_dir() {
        return;
    }
    let Ok(content) = std::fs::read(&resolved) else {
        return;
    };
    state.put(
        resolved,
        crate::core::tools::pipeline::read_state::ReadStamp {
            mtime_ms: crate::core::tools::pipeline::read_state::metadata_mtime_ms(&meta),
            size: meta.len(),
            content_hash: crate::core::tools::pipeline::read_state::hash_content(&content),
            offset: None,
            limit: None,
            is_partial_view: false,
            render_mode: crate::core::tools::pipeline::read_state::ReadRenderMode::Plain,
            covered_lines: None,
            reached_eof: false,
            tool_call_id: Some(tool_call_id.to_string()),
        },
    );
}

pub(super) fn validate_read_bounds(offset: Option<u64>, limit: Option<u64>) -> Result<(), String> {
    if let Some(o) = offset {
        if o < 1 {
            return Err(
                "read.offset must be >= 1 (1-based line number; pass `1` to start from the first line)".to_string(),
            );
        }
    }
    if let Some(l) = limit {
        if !(1..=10_000).contains(&l) {
            return Err(format!("read.limit must be in [1, 10000] (got {l}); split large reads with multiple offset+limit calls"));
        }
    }
    Ok(())
}
