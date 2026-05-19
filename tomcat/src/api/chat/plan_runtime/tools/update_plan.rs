//! `update_plan` 工具实现（plan-runtime.md §P2 / [update-plan.md]）。
//!
//! 语义：
//! - 任何模式可见；按 `plan_id` 路由（EXEC 缺省取 `active_plan_id`，其它模式必填）。
//! - 只能动 `todos[]` / `milestones[]`；不能改 `mode`/`session_key`/`schema_version` 等
//!   runtime 字段（在 ops 层封死）。
//! - 跨 session 编辑规则：
//!   - 目标 plan `mode ∈ {planning, pending}`：允许（协作改稿）
//!   - 目标 plan `mode == executing` 且 `session_key != current_session_key`：拒（防写入竞争）
//! - 写盘后 EXEC 自动派生：所有 todos completed → 上层 `chat_loop` 接到工具结果后
//!   切 `mode=completed` + frontmatter 同步（PR-PLE / P7 完整实现；本 P2 仅在结果
//!   JSON 中暴露 `plan_mode_before/plan_mode_after` 字段）。

use serde::Deserialize;

use crate::api::chat::plan_runtime::{
    file_store::{
        plan_path_for_id, read_plan, write_plan, Milestone, PlanFileMode, TodoStatus,
    },
    mode::PlanMode,
    ops,
    PlanRuntime,
};

use super::ToolError;

#[derive(Debug, Deserialize)]
pub struct UpdatePlanArgs {
    /// 目标 plan_id；EXEC 模式可省略（默认 active_plan_id）。
    #[serde(default)]
    pub plan_id: Option<String>,
    /// 增量 ops；与 ops::TodoOp 一一对应。
    pub ops: Vec<UpdateOp>,
    /// 可选 milestone 重命名 / 修改（M1：仅支持 title 更新与 todo_ids 重排）。
    #[serde(default)]
    pub milestones_ops: Vec<MilestoneOp>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op")]
#[serde(rename_all = "snake_case")]
pub enum UpdateOp {
    AddTodo {
        id: String,
        content: String,
        #[serde(default = "default_pending")]
        status: TodoStatus,
        #[serde(default)]
        milestone_id: Option<String>,
    },
    SetStatus {
        id: String,
        status: TodoStatus,
    },
    SetContent {
        id: String,
        content: String,
    },
    SetMilestone {
        id: String,
        milestone_id: Option<String>,
    },
    RemoveTodo {
        id: String,
    },
}

fn default_pending() -> TodoStatus {
    TodoStatus::Pending
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op")]
#[serde(rename_all = "snake_case")]
pub enum MilestoneOp {
    Add {
        id: String,
        title: String,
        #[serde(default)]
        todo_ids: Vec<String>,
    },
    SetTitle {
        id: String,
        title: String,
    },
    SetTodoIds {
        id: String,
        todo_ids: Vec<String>,
    },
    Remove {
        id: String,
    },
}

impl UpdatePlanArgs {
    pub fn from_json(raw: &serde_json::Value) -> Result<Self, ToolError> {
        serde_json::from_value(raw.clone())
            .map_err(|e| ToolError::BadArgs(format!("update_plan args: {e}")))
    }
}

pub fn execute(
    runtime: &PlanRuntime,
    args: UpdatePlanArgs,
) -> Result<serde_json::Value, ToolError> {
    let target_plan_id = resolve_target_plan_id(runtime, args.plan_id)?;
    let path = plan_path_for_id(&target_plan_id)?;
    let mut plan = read_plan(&path)?;

    let plan_mode_before = plan.frontmatter.mode;
    enforce_cross_session_policy(runtime, &plan.frontmatter, plan_mode_before)?;

    let todo_ops: Vec<ops::TodoOp> = args
        .ops
        .into_iter()
        .map(|o| match o {
            UpdateOp::AddTodo {
                id,
                content,
                status,
                milestone_id,
            } => ops::TodoOp::AddTodo(crate::api::chat::plan_runtime::file_store::TodoItem {
                id,
                content,
                status,
                milestone_id,
            }),
            UpdateOp::SetStatus { id, status } => ops::TodoOp::SetStatus { id, status },
            UpdateOp::SetContent { id, content } => ops::TodoOp::SetContent { id, content },
            UpdateOp::SetMilestone { id, milestone_id } => {
                ops::TodoOp::SetMilestone { id, milestone_id }
            }
            UpdateOp::RemoveTodo { id } => ops::TodoOp::RemoveTodo { id },
        })
        .collect();
    ops::apply_todos_ops(&mut plan.frontmatter.todos, &todo_ops)?;

    apply_milestone_ops(&mut plan.frontmatter.milestones, &args.milestones_ops)?;

    // EXEC + all completed → 自动派生 completed（写盘）
    let derived_completed = matches!(plan_mode_before, PlanFileMode::Executing)
        && ops::all_completed(&plan.frontmatter.todos);
    if derived_completed {
        plan.frontmatter.mode = PlanFileMode::Completed;
    }

    write_plan(&path, &plan, runtime.lock_timeout_ms())?;
    let plan_mode_after = plan.frontmatter.mode;

    // EXEC 完成时同步内存 mode（PR-PLE 完整 reminder swap 在 P7 完善）。
    if derived_completed {
        runtime.set_mode_completed(target_plan_id.clone());
    }

    Ok(serde_json::json!({
        "plan_id": target_plan_id,
        "applied": todo_ops_len(&plan.frontmatter.todos),
        "plan_mode_before": plan_mode_before.as_str(),
        "plan_mode_after": plan_mode_after.as_str(),
        "items": plan
            .frontmatter
            .todos
            .iter()
            .map(|t| serde_json::json!({
                "id": t.id,
                "content": t.content,
                "status": t.status.as_str(),
                "milestone_id": t.milestone_id,
            }))
            .collect::<Vec<_>>(),
        "milestones": plan
            .frontmatter
            .milestones
            .iter()
            .map(|m| serde_json::json!({
                "id": m.id,
                "title": m.title,
                "todo_ids": m.todo_ids,
            }))
            .collect::<Vec<_>>(),
    }))
}

fn resolve_target_plan_id(
    runtime: &PlanRuntime,
    explicit: Option<String>,
) -> Result<String, ToolError> {
    if let Some(id) = explicit {
        return Ok(id);
    }
    if let PlanMode::Executing { plan_id } | PlanMode::Pending { plan_id } = runtime.mode() {
        return Ok(plan_id);
    }
    if let Some(id) = runtime.active_planning_plan_id() {
        return Ok(id);
    }
    Err(ToolError::BadArgs(
        "update_plan 需要 plan_id；当前模式无 active plan".into(),
    ))
}

fn enforce_cross_session_policy(
    runtime: &PlanRuntime,
    fm: &crate::api::chat::plan_runtime::file_store::PlanFileFrontmatter,
    mode: PlanFileMode,
) -> Result<(), ToolError> {
    if !matches!(mode, PlanFileMode::Executing) {
        return Ok(()); // planning / pending / completed 允许跨 session
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

fn apply_milestone_ops(
    milestones: &mut Vec<Milestone>,
    ops: &[MilestoneOp],
) -> Result<(), ToolError> {
    for op in ops {
        match op {
            MilestoneOp::Add { id, title, todo_ids } => {
                if milestones.iter().any(|m| &m.id == id) {
                    return Err(ToolError::BadArgs(format!("milestone id 重复: {id}")));
                }
                milestones.push(Milestone {
                    id: id.clone(),
                    title: title.clone(),
                    todo_ids: todo_ids.clone(),
                });
            }
            MilestoneOp::SetTitle { id, title } => {
                let m = milestones
                    .iter_mut()
                    .find(|m| &m.id == id)
                    .ok_or_else(|| ToolError::BadArgs(format!("milestone 不存在: {id}")))?;
                m.title = title.clone();
            }
            MilestoneOp::SetTodoIds { id, todo_ids } => {
                let m = milestones
                    .iter_mut()
                    .find(|m| &m.id == id)
                    .ok_or_else(|| ToolError::BadArgs(format!("milestone 不存在: {id}")))?;
                m.todo_ids = todo_ids.clone();
            }
            MilestoneOp::Remove { id } => {
                let before = milestones.len();
                milestones.retain(|m| &m.id != id);
                if milestones.len() == before {
                    return Err(ToolError::BadArgs(format!("milestone 不存在: {id}")));
                }
            }
        }
    }
    Ok(())
}

fn todo_ops_len(todos: &[crate::api::chat::plan_runtime::file_store::TodoItem]) -> usize {
    todos.len()
}
