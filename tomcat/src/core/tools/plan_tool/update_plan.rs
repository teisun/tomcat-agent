//! `update_plan` 工具实现（plan-runtime.md §P2 / [update-plan.md] / G1+G2+N2 2026-05）。
//!
//! 语义：
//! - 任何模式可见；按 `plan_id` / `path` 路由（`plan_id` 优先，缺省取 active plan）。
//! - 入参 **仅认 `kind`**（D3 破坏性）：`upsert | set_status | remove`。
//! - State 矩阵闸门（G2 / `update-plan.md` §6.2）：
//!   - 已完成计划重新出现非终态 todo 时回到 `pending`。
//!   - `set_status: in_progress` 仅 `executing` 允许；planning / pending 一律拒。
//! - 跨 session 编辑规则：
//!   - 目标 plan `state ∈ {planning, pending}`：允许（协作改稿）
//!   - 目标 plan `state == executing` 且 `session_key != current_session_key`：拒
//! - 写盘后 EXEC 由 `NextAction` 统一裁决：至少一个 todo 且全部终态时计划完成。
//!   LLM 可将唯一的最终验收 todo 标为 `Acceptance`；其变为 `in_progress` 时结果
//!   返回 `load_skill(verify)` 提示，验收范围与重跑判断由该 skill 按实际 diff 决定。
//! - 返回 JSON（G1）：`plan_id` / `path` / `applied` / `items[]` /
//!   `active_in_progress` / `plan_state_before` / `plan_state_after` / `warnings[]` /
//!   `panel_snapshot_id` / `next_step`（节流后 panel 刷新版本；目前与 timestamp 等价）。

use std::path::PathBuf;

use serde::Deserialize;

use crate::core::plan_runtime::{
    file_store::{
        update_plan_locked, validate_single_acceptance, write_plan, PlanFileState, TodoItem,
        TodoKind, TodoStatus,
    },
    NextAction, PlanRuntime,
};

use super::shared_todo_ops::{apply_shared_todo_ops, items_json};
use super::ToolError;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdatePlanArgs {
    /// 目标 plan_id；执行中计划可省略（默认当前 active plan）。
    #[serde(default)]
    pub plan_id: Option<String>,
    /// 可选直接路径；仅在未传 plan_id 时生效。
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub replace: bool,
    /// 增量 ops；统一为 `kind` 标记的 enum（D3 破坏性）。
    #[serde(default)]
    pub ops: Vec<UpdateOp>,
}

pub use super::shared_todo_ops::SharedTodoOpArg as UpdateOp;

impl UpdatePlanArgs {
    pub fn from_json(raw: &serde_json::Value) -> Result<Self, ToolError> {
        // D3 破坏性：旧字段名 `op` 已下线，遇到立即报错。
        if let Some(ops) = raw.get("ops").and_then(|v| v.as_array()) {
            for op in ops {
                if op.get("op").is_some() && op.get("kind").is_none() {
                    return Err(ToolError::BadArgs(
                        "update_plan ops: 字段 `op` 已下线，请改用 `kind`（kind: upsert | set_status | remove）".into(),
                    ));
                }
            }
        }
        if raw.get("replace_todos").is_some() || raw.get("replace_milestones").is_some() {
            return Err(ToolError::BadArgs(
                "update_plan 顶层字段 `replace_todos` / `replace_milestones` 已下线，请统一改用 `replace`"
                    .into(),
            ));
        }
        if raw.get("milestones_ops").is_some() {
            return Err(ToolError::BadArgs(
                "update_plan 不再支持 `milestones_ops`；当前仅支持 todo-only ops".into(),
            ));
        }
        serde_json::from_value(raw.clone())
            .map_err(|e| ToolError::BadArgs(format!("update_plan args: {e}")))
    }
}

pub async fn execute(
    runtime: &PlanRuntime,
    args: UpdatePlanArgs,
) -> Result<serde_json::Value, ToolError> {
    execute_for_tool(runtime, args, "update-plan-direct").await
}

/// Apply ordinary todo updates. Runtime completion is based solely on every
/// todo being terminal; code-review and acceptance are prompt-driven.
pub async fn execute_for_tool(
    runtime: &PlanRuntime,
    args: UpdatePlanArgs,
    _tool_call_id: &str,
) -> Result<serde_json::Value, ToolError> {
    let path = resolve_target_plan_path(runtime, args.plan_id.clone(), args.path.clone())?;

    struct UpdateTxOutcome {
        plan: crate::core::plan_runtime::file_store::PlanFile,
        plan_state_before: PlanFileState,
        warnings: Vec<String>,
    }

    let tx = match update_plan_locked(&path, runtime.lock_timeout_ms(), |plan| {
        let plan_state_before = plan.frontmatter.state;
        enforce_cross_session_policy(runtime, &plan.frontmatter, plan_state_before)?;
        enforce_state_matrix(plan_state_before, &args.ops)?;
        enforce_executing_work_content_is_frozen(
            plan_state_before,
            &plan.frontmatter.todos,
            &args.ops,
        )?;
        let mut warnings =
            apply_plan_todo_ops(&mut plan.frontmatter.todos, &args.ops, args.replace)?;

        if matches!(plan_state_before, PlanFileState::Completed)
            && !plan_completion_ready(&plan.frontmatter.todos)
        {
            plan.frontmatter.state = PlanFileState::Pending;
            warnings.push("plan was reopened because its todos are no longer all complete".into());
        }

        rewrite_todos_board(&mut plan.body, &plan.frontmatter.todos);
        Ok(UpdateTxOutcome {
            plan: plan.clone(),
            plan_state_before,
            warnings,
        })
    }) {
        Ok(value) => value,
        Err(crate::core::plan_runtime::file_store::LockedPlanMutationError::Plan(error)) => {
            return Err(error.into());
        }
        Err(crate::core::plan_runtime::file_store::LockedPlanMutationError::Callback(error)) => {
            return Err(error);
        }
    };

    let mut plan = tx.plan;
    let target_plan_id = plan.frontmatter.plan_id.clone();
    let warnings = tx.warnings;
    if plan.frontmatter.state != PlanFileState::Completed
        && plan_completion_ready(&plan.frontmatter.todos)
    {
        finalize_plan_completed(runtime, &target_plan_id, &path, &mut plan)?;
    }

    let plan_state_after = plan.frontmatter.state;
    let next_action = runtime.next_action(&plan.frontmatter).await;
    let active_in_progress = plan
        .frontmatter
        .todos
        .iter()
        .find(|todo| matches!(todo.status, TodoStatus::InProgress))
        .map(|todo| todo.id.clone());
    let panel_snapshot_id = crate::core::plan_runtime::panels::next_panel_snapshot_id();
    runtime.refresh_active_plan_after_write(path.clone(), &plan);
    runtime
        .refresh_notifier()
        .notify(&crate::core::plan_runtime::panels::TodosPanelSnapshot {
            panel_snapshot_id,
            scope: format!("plan:{target_plan_id}"),
            items: plan.frontmatter.todos.clone(),
            warnings: warnings.clone(),
        });

    if matches!(tx.plan_state_before, PlanFileState::Completed)
        && matches!(plan_state_after, PlanFileState::Pending)
    {
        runtime.write_transcript_custom(serde_json::json!({
            "event": crate::infra::wire::WIRE_PLAN_PENDING,
            "plan_id": target_plan_id.clone(),
            "path": crate::infra::platform::format_home_path(&path),
            "state": plan_state_after.as_str(),
        }));
    }
    if !matches!(plan_state_after, PlanFileState::Completed) {
        runtime.write_transcript_custom(serde_json::json!({
            "event": crate::infra::wire::WIRE_PLAN_UPDATE,
            "plan_id": target_plan_id,
            "path": crate::infra::platform::format_home_path(&path),
            "state": plan_state_after.as_str(),
        }));
        runtime.write_transcript_custom(serde_json::json!({
            "event": crate::infra::wire::WIRE_PLAN_TODOS,
            "plan_id": target_plan_id,
            "todos": items_json(&plan.frontmatter.todos),
        }));
    }

    Ok(serde_json::json!({
        "plan_id": target_plan_id,
        "path": crate::infra::platform::format_home_path(&path),
        "applied": args.ops.len(),
        "replace": args.replace,
        "plan_state_before": tx.plan_state_before.as_str(),
        "plan_state_after": plan_state_after.as_str(),
        "panel_snapshot_id": panel_snapshot_id,
        "warnings": warnings,
        "active_in_progress": active_in_progress,
        "items": items_json(&plan.frontmatter.todos),
        "next_step": next_action_json(&next_action),
    }))
}

fn apply_plan_todo_ops(
    todos: &mut Vec<TodoItem>,
    ops_list: &[UpdateOp],
    replace: bool,
) -> Result<Vec<String>, ToolError> {
    if ops_list.iter().any(|op| {
        matches!(
            op,
            UpdateOp::Upsert {
                todo_kind: Some(TodoKind::Unknown),
                ..
            }
        )
    }) {
        return Err(ToolError::BadArgs(
            "update_plan.ops[].todo_kind 只支持 work 或 acceptance".into(),
        ));
    }
    apply_shared_todo_ops(todos, ops_list, replace)?;
    validate_single_acceptance(todos).map_err(|error| ToolError::BadArgs(error.to_string()))?;
    Ok(Vec::new())
}

fn plan_completion_ready(todos: &[TodoItem]) -> bool {
    !todos.is_empty()
        && todos
            .iter()
            .all(|todo| matches!(todo.status, TodoStatus::Completed | TodoStatus::Cancelled))
}

fn next_action_json(action: &NextAction) -> serde_json::Value {
    serde_json::json!({
        "phase": action.phase(),
        "hint": action.instruction(),
    })
}

fn finalize_plan_completed(
    runtime: &PlanRuntime,
    target_plan_id: &str,
    path: &std::path::Path,
    plan: &mut crate::core::plan_runtime::file_store::PlanFile,
) -> Result<(), ToolError> {
    plan.frontmatter.state = PlanFileState::Completed;
    write_plan(path, plan, runtime.lock_timeout_ms())?;
    runtime.refresh_active_plan_after_write(path.to_path_buf(), plan);
    let mut completion_event = serde_json::json!({
        "event": crate::infra::wire::WIRE_PLAN_COMPLETE,
        "plan_id": target_plan_id,
        "path": crate::infra::platform::format_home_path(path),
        "state": PlanFileState::Completed.as_str(),
    });
    if let (Some(event), Some(cache_observation)) = (
        completion_event.as_object_mut(),
        runtime.plan_cache_observation_json(target_plan_id),
    ) {
        event.insert("cacheObservation".into(), cache_observation);
    }
    runtime.write_transcript_custom(completion_event);
    Ok(())
}

fn resolve_target_plan_path(
    runtime: &PlanRuntime,
    explicit_plan_id: Option<String>,
    explicit_path: Option<String>,
) -> Result<PathBuf, ToolError> {
    if let Some(id) = explicit_plan_id {
        return runtime.resolved_plan_path(&id).map_err(ToolError::BadArgs);
    }
    if let Some(path) = explicit_path {
        return crate::infra::platform::normalize_path(&path)
            .map_err(|e| ToolError::BadArgs(format!("update_plan path 非法：{e}")));
    }
    if let Some(plan) = runtime.active_plan() {
        return Ok(plan.path);
    }
    Err(ToolError::BadArgs(
        "update_plan 需要 plan_id 或 path；当前模式无 active plan".into(),
    ))
}

fn enforce_cross_session_policy(
    runtime: &PlanRuntime,
    fm: &crate::core::plan_runtime::file_store::PlanFileFrontmatter,
    state: PlanFileState,
) -> Result<(), ToolError> {
    if !matches!(state, PlanFileState::Executing) {
        return Ok(());
    }
    let target_key = fm.session_key.as_deref().unwrap_or("");
    if target_key != runtime.session_key() {
        return Err(ToolError::CrossSessionDenied(format!(
            "plan {} 当前由 session {target_key} 在 EXEC，本 session {} 不能写入",
            fm.plan_id,
            runtime.session_key()
        )));
    }
    Ok(())
}

/// `content` is the approved work description. During execution it must remain stable, otherwise
/// a todo can appear complete even though the original work was silently rewritten. Progress,
/// verification, and noteworthy trade-offs belong in `evidence`.
fn enforce_executing_work_content_is_frozen(
    plan_state: PlanFileState,
    todos: &[TodoItem],
    ops_list: &[UpdateOp],
) -> Result<(), ToolError> {
    if !matches!(plan_state, PlanFileState::Executing) {
        return Ok(());
    }
    for op in ops_list {
        let UpdateOp::Upsert {
            id,
            content: Some(content),
            ..
        } = op
        else {
            continue;
        };
        let Some(existing) = todos.iter().find(|todo| todo.id == *id) else {
            continue;
        };
        if existing.kind == TodoKind::Work && existing.content != *content {
            return Err(ToolError::BadArgs(format!(
                "执行中的 work todo `{id}` 的 content 已冻结；请保持原工作描述，并用 set_status 的 `evidence` 记录进展或验证结果"
            )));
        }
    }
    Ok(())
}

/// G2 state 矩阵闸门——参考 [update-plan.md] §6.2。
fn enforce_state_matrix(plan_state: PlanFileState, ops_list: &[UpdateOp]) -> Result<(), ToolError> {
    for op in ops_list {
        match (plan_state, op) {
            // in_progress 仅在 executing 允许
            (
                PlanFileState::Planning | PlanFileState::Pending,
                UpdateOp::SetStatus {
                    status: TodoStatus::InProgress,
                    ..
                },
            )
            | (
                PlanFileState::Planning | PlanFileState::Pending,
                UpdateOp::Upsert {
                    status: Some(TodoStatus::InProgress),
                    ..
                },
            ) => {
                return Err(ToolError::BadArgs(format!(
                    "in_progress 仅允许在 executing 状态下使用；当前 plan.state = {}",
                    plan_state.as_str()
                )));
            }
            _ => {}
        }
    }
    Ok(())
}

/// E2：在 `## Todos Board` 的标记区间内重写 todos 状态视图。
///
/// 标记格式：
/// ```text
/// ## Todos Board
///
/// <!-- todos-board:auto:begin -->
/// (auto content)
/// <!-- todos-board:auto:end -->
/// ```
///
/// 若 body 中找不到标记，则**不**改 body（与"用户手工删除 marker → 关闭自动化"语义一致）。
pub fn rewrite_todos_board(
    body: &mut String,
    todos: &[crate::core::plan_runtime::file_store::TodoItem],
) {
    const BEGIN: &str = "<!-- todos-board:auto:begin -->";
    const END: &str = "<!-- todos-board:auto:end -->";
    let Some(begin_idx) = body.find(BEGIN) else {
        return;
    };
    let body_after_begin = begin_idx + BEGIN.len();
    let Some(end_rel) = body[body_after_begin..].find(END) else {
        return;
    };
    let end_idx = body_after_begin + end_rel;
    let mut rendered = String::from("\n");
    rendered.push_str("### Todos\n");
    if todos.is_empty() {
        rendered.push_str("_(empty)_\n");
    } else {
        use crate::core::plan_runtime::file_store::TodoStatus;
        for t in todos {
            let checkbox = match t.status {
                TodoStatus::Completed => "x",
                TodoStatus::InProgress => "~",
                TodoStatus::Cancelled => "-",
                TodoStatus::Pending => " ",
            };
            rendered.push_str(&format!("- [{checkbox}] {}: {}\n", t.id, t.content));
        }
    }
    body.replace_range(body_after_begin..end_idx, &rendered);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_hint_is_a_prompt_not_a_green_build_contract() {
        let hint = NextAction::RunVerify.instruction();

        assert!(hint.contains("load_skill(verify)"));
        assert!(hint.contains("按影响范围复核 diff 并验证"));
        assert!(!hint.contains("green_build"));
        assert!(!hint.contains("acceptance_commands"));
    }
}
