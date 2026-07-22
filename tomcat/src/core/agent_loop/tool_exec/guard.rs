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
        return Err(format!(
            "NoPriorRead: 当前会话未对 `{}` 执行过 `read`，禁止盲写/盲改；请先 `read` 再 `{}`",
            path, op_label
        ));
    };
    let Ok(meta) = std::fs::metadata(&resolved) else {
        return Ok(());
    };
    if meta.is_dir() {
        return Err(format!(
            "{}: 目标 `{}` 是目录，不能作为入参",
            op_label, path
        ));
    }
    let cur_mtime = crate::core::tools::pipeline::read_state::metadata_mtime_ms(&meta);
    if stamp.mtime_ms != cur_mtime || stamp.size != meta.len() {
        return Err(format!(
            "Stale: 文件 `{}` 自上次 read 后已被修改（mtime/size 不一致），请先重新 `read` 再 `{}`",
            path, op_label
        ));
    }
    Ok(())
}

pub(super) fn validate_read_bounds(offset: Option<u64>, limit: Option<u64>) -> Result<(), String> {
    if let Some(o) = offset {
        if o < 1 {
            return Err(
                "read.offset must be >= 1 (1-based line number; pass `1` to start from the first line)"
                    .to_string(),
            );
        }
    }
    if let Some(l) = limit {
        if !(1..=10_000).contains(&l) {
            return Err(format!(
                "read.limit must be in [1, 10000] (got {}); split large reads with multiple offset+limit calls",
                l
            ));
        }
    }
    Ok(())
}
