//! `update_plan` 工具实现（plan-runtime.md §P2 / [update-plan.md] / G1+G2+N2 2026-05）。
//!
//! 语义：
//! - 任何模式可见；按 `plan_id` 路由（EXEC 缺省取 `active_plan_id`，其它模式必填）。
//! - 入参 **仅认 `kind`**（D3 破坏性）：`upsert | set_status | remove |
//!   milestone_upsert | milestone_remove`。LLM 传 `op` → `BadArgs`。
//! - Mode 矩阵闸门（G2 / `update-plan.md` §6.2）：
//!   - 目标 `plan.mode == completed` → 全拒（N2）。
//!   - `set_status: in_progress` 仅 `executing` 允许；planning / pending 一律拒。
//!   - `milestone_upsert`（新 id）/ `milestone_remove` 仅 `planning` / `pending` 允许。
//! - 跨 session 编辑规则：
//!   - 目标 plan `mode ∈ {planning, pending}`：允许（协作改稿）
//!   - 目标 plan `mode == executing` 且 `session_key != current_session_key`：拒
//! - 写盘后 EXEC 自动派生：所有 todos completed → 切 `mode=completed`。
//! - 返回 JSON（G1）：`plan_id` / `path` / `applied` / `items[]` / `milestones[]` /
//!   `active_in_progress` / `plan_mode_before` / `plan_mode_after` / `warnings[]` /
//!   `panel_snapshot_id`（节流后 panel 刷新版本；目前与 timestamp 等价）。

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
    /// 增量 ops；统一为 `kind` 标记的 enum（D3 破坏性）。
    pub ops: Vec<UpdateOp>,
    /// 老字段保留为反序列化兜底：传则报错（D3 破坏性）。
    #[serde(default)]
    pub milestones_ops: serde_json::Value,
    #[serde(default)]
    pub replace_todos: bool,
    #[serde(default)]
    pub replace_milestones: bool,
}

/// 兼容老调用方（PlanRuntime 内部代码、测试 fixture）保留五种 todo-only 写法的别名；
/// LLM 入口由 `serde(tag="kind")` 反序列化时只认 `upsert / set_status / remove /
/// milestone_upsert / milestone_remove` 五个 kind。
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind")]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum UpdateOp {
    /// 创建或更新 todo。`content` / `status` / `milestone_id` 任一非空即视为更新对应字段；
    /// 不存在则按已提供字段创建。
    Upsert {
        id: String,
        #[serde(default)]
        content: Option<String>,
        #[serde(default)]
        status: Option<TodoStatus>,
        #[serde(default)]
        milestone_id: Option<String>,
    },
    /// 只改 status；最常见的「推进任务」入口。
    SetStatus { id: String, status: TodoStatus },
    /// 删除 todo。
    Remove { id: String },
    /// 创建或更新 milestone。`title` / `todo_ids` 任一非空即视为更新对应字段。
    MilestoneUpsert {
        id: String,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        todo_ids: Option<Vec<String>>,
    },
    /// 删除 milestone。
    MilestoneRemove { id: String },
}

impl UpdateOp {
    /// 旧 API 兼容 helper：测试/内部代码直接构造 SetContent 时映射为 Upsert(content)。
    /// 已在测试与 PlanRuntime 内部用于把"老写法"翻译到新 enum。
    #[allow(non_snake_case)]
    pub fn SetContent(id: impl Into<String>, content: impl Into<String>) -> Self {
        UpdateOp::Upsert {
            id: id.into(),
            content: Some(content.into()),
            status: None,
            milestone_id: None,
        }
    }
    #[allow(non_snake_case)]
    pub fn AddTodo(
        id: impl Into<String>,
        content: impl Into<String>,
        status: TodoStatus,
        milestone_id: Option<String>,
    ) -> Self {
        UpdateOp::Upsert {
            id: id.into(),
            content: Some(content.into()),
            status: Some(status),
            milestone_id,
        }
    }
}

impl UpdatePlanArgs {
    pub fn from_json(raw: &serde_json::Value) -> Result<Self, ToolError> {
        // D3 破坏性：旧字段名 `op` 已下线，遇到立即报错。
        if let Some(ops) = raw.get("ops").and_then(|v| v.as_array()) {
            for op in ops {
                if op.get("op").is_some() && op.get("kind").is_none() {
                    return Err(ToolError::BadArgs(
                        "update_plan ops: 字段 `op` 已下线，请改用 `kind`（kind: upsert | set_status | remove | milestone_upsert | milestone_remove）".into(),
                    ));
                }
            }
        }
        serde_json::from_value(raw.clone())
            .map_err(|e| ToolError::BadArgs(format!("update_plan args: {e}")))
    }
}

pub fn execute(
    runtime: &PlanRuntime,
    args: UpdatePlanArgs,
) -> Result<serde_json::Value, ToolError> {
    if !args.milestones_ops.is_null() {
        return Err(ToolError::BadArgs(
            "update_plan: `milestones_ops` 已合并进 `ops`（用 kind=milestone_upsert / milestone_remove）".into(),
        ));
    }

    let target_plan_id = resolve_target_plan_id(runtime, args.plan_id)?;
    let path = plan_path_for_id(&target_plan_id)?;
    let mut plan = read_plan(&path)?;

    let plan_mode_before = plan.frontmatter.mode;

    // E6：在应用 ops 之前快照各 milestone 状态，便于写盘成功后对比"刚刚完成"集合。
    let milestone_status_before: std::collections::HashMap<String, _> = plan
        .frontmatter
        .milestones
        .iter()
        .map(|m| (m.id.clone(), m.status.clone()))
        .collect();

    // N2：completed 全拒。
    if matches!(plan_mode_before, PlanFileMode::Completed) {
        return Err(ToolError::CrossSessionDenied(format!(
            "plan {target_plan_id} 已 completed，无法再编辑"
        )));
    }

    enforce_cross_session_policy(runtime, &plan.frontmatter, plan_mode_before)?;

    // G2 mode 矩阵闸门：先做语义校验，再下沉到 ops 引擎。
    enforce_mode_matrix(plan_mode_before, &args.ops, &plan)?;

    let mut warnings: Vec<String> = Vec::new();
    let mut applied: usize = 0;

    for op in &args.ops {
        match op {
            UpdateOp::Upsert {
                id,
                content,
                status,
                milestone_id,
            } => {
                let exists = plan.frontmatter.todos.iter().any(|t| &t.id == id);
                if exists {
                    // 单 in_progress 由 apply_todos_ops 引擎保留：所有 status 变更走引擎。
                    if let Some(s) = status {
                        ops::apply_todos_ops(
                            &mut plan.frontmatter.todos,
                            &[ops::TodoOp::SetStatus {
                                id: id.clone(),
                                status: s.clone(),
                            }],
                        )?;
                    }
                    // content / milestone_id 走简单就地写。
                    if let Some(existing) =
                        plan.frontmatter.todos.iter_mut().find(|t| &t.id == id)
                    {
                        if let Some(c) = content {
                            existing.content = c.clone();
                        }
                        if milestone_id.is_some() {
                            existing.milestone_id = milestone_id.clone();
                        }
                    }
                } else {
                    let new_item =
                        crate::api::chat::plan_runtime::file_store::TodoItem {
                            id: id.clone(),
                            content: content.clone().unwrap_or_default(),
                            status: status.clone().unwrap_or(TodoStatus::Pending),
                            milestone_id: milestone_id.clone(),
                        };
                    ops::apply_todos_ops(
                        &mut plan.frontmatter.todos,
                        &[ops::TodoOp::AddTodo(new_item)],
                    )?;
                }
                applied += 1;
            }
            UpdateOp::SetStatus { id, status } => {
                ops::apply_todos_ops(
                    &mut plan.frontmatter.todos,
                    &[ops::TodoOp::SetStatus {
                        id: id.clone(),
                        status: status.clone(),
                    }],
                )?;
                applied += 1;
            }
            UpdateOp::Remove { id } => {
                ops::apply_todos_ops(
                    &mut plan.frontmatter.todos,
                    &[ops::TodoOp::RemoveTodo { id: id.clone() }],
                )?;
                applied += 1;
            }
            UpdateOp::MilestoneUpsert {
                id,
                title,
                todo_ids,
            } => {
                if let Some(m) =
                    plan.frontmatter.milestones.iter_mut().find(|m| &m.id == id)
                {
                    if let Some(t) = title {
                        m.title = t.clone();
                    }
                    if let Some(ids) = todo_ids {
                        m.todo_ids = ids.clone();
                    }
                } else {
                    plan.frontmatter.milestones.push(Milestone {
                        id: id.clone(),
                        title: title.clone().unwrap_or_default(),
                        todo_ids: todo_ids.clone().unwrap_or_default(),
                        status:
                            crate::api::chat::plan_runtime::file_store::MilestoneStatus::Pending,
                        description: None,
                    });
                }
                applied += 1;
            }
            UpdateOp::MilestoneRemove { id } => {
                let before = plan.frontmatter.milestones.len();
                plan.frontmatter.milestones.retain(|m| &m.id != id);
                if plan.frontmatter.milestones.len() == before {
                    return Err(ToolError::BadArgs(format!("milestone 不存在: {id}")));
                }
                applied += 1;
            }
        }
    }

    // E2：每次写盘前从 todos 派生 milestone.status（保单一事实源）。
    {
        use crate::api::chat::plan_runtime::file_store::derive_milestone_status;
        let todos_snapshot = plan.frontmatter.todos.clone();
        for m in plan.frontmatter.milestones.iter_mut() {
            m.status = derive_milestone_status(&m.todo_ids, &todos_snapshot);
        }
    }

    // 一致性校验：每个 todo.milestone_id 必须在 milestones[].id 中；E5 / H13
    if let Err(msg) = validate_milestone_refs(&plan) {
        warnings.push(msg);
    }

    let derived_completed = matches!(plan_mode_before, PlanFileMode::Executing)
        && ops::all_completed(&plan.frontmatter.todos);
    if derived_completed {
        plan.frontmatter.mode = PlanFileMode::Completed;
    }

    // E2：在 body 的 `## Todos Board` 标记区间内自动重写当前 todos 状态视图。
    rewrite_todos_board(&mut plan.body, &plan.frontmatter.todos, &plan.frontmatter.milestones);

    write_plan(&path, &plan, runtime.lock_timeout_ms())?;
    let plan_mode_after = plan.frontmatter.mode;

    if derived_completed {
        runtime.set_mode_completed(target_plan_id.clone());
    }

    // E6：detect milestones that just transitioned to Completed → record checkpoint。
    // `[plan].auto_checkpoint_on_milestone=true`（默认）启用；失败仅 warning，不阻塞工具返回。
    if runtime.auto_checkpoint_on_milestone() {
        if let Some(store) = runtime.checkpoint_store() {
            use crate::api::chat::plan_runtime::file_store::MilestoneStatus;
            for m in &plan.frontmatter.milestones {
                let was_completed = matches!(
                    milestone_status_before.get(&m.id),
                    Some(MilestoneStatus::Completed)
                );
                let now_completed = matches!(m.status, MilestoneStatus::Completed);
                if !was_completed && now_completed {
                    let req = crate::core::CheckpointRecordRequest {
                        session_id: runtime.session_key().to_string(),
                        turn_id: format!("milestone-{}-{}", target_plan_id, m.id),
                        kind: crate::core::CheckpointKind::Milestone {
                            milestone_id: m.id.clone(),
                        },
                        message_anchor: None,
                        notes: Some(serde_json::json!({
                            "plan_id": target_plan_id,
                            "milestone_title": m.title,
                            "label": null,
                        })),
                    };
                    if let Err(e) = store.record(req) {
                        let w = format!("milestone {} checkpoint record 失败: {e}", m.id);
                        tracing::warn!(target: "plan_runtime::update_plan", "{w}");
                        warnings.push(w);
                    }
                }
            }
        }
    }

    let active_in_progress = plan
        .frontmatter
        .todos
        .iter()
        .find(|t| matches!(t.status, TodoStatus::InProgress))
        .map(|t| t.id.clone());

    let panel_snapshot_id = crate::api::chat::plan_runtime::todos_panel::next_panel_snapshot_id();

    // E：fanout UI 刷新——advisory lock 在 write_plan 内已 release，这里仅同步通知
    // 已注册 panel；panel 自行决定如何渲染（CLI/IDE/noop）。
    let snapshot = crate::api::chat::plan_runtime::todos_panel::TodosPanelSnapshot {
        panel_snapshot_id,
        scope: format!("plan:{target_plan_id}"),
        items: plan.frontmatter.todos.clone(),
        milestones: plan.frontmatter.milestones.clone(),
        warnings: warnings.clone(),
    };
    runtime.refresh_notifier().notify(&snapshot);

    Ok(serde_json::json!({
        "plan_id": target_plan_id,
        "path": path.display().to_string(),
        "applied": applied,
        "plan_mode_before": plan_mode_before.as_str(),
        "plan_mode_after": plan_mode_after.as_str(),
        "panel_snapshot_id": panel_snapshot_id,
        "warnings": warnings,
        "active_in_progress": active_in_progress,
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

/// G2 mode 矩阵闸门——参考 [update-plan.md] §6.2。
fn enforce_mode_matrix(
    plan_mode: PlanFileMode,
    ops_list: &[UpdateOp],
    plan: &crate::api::chat::plan_runtime::file_store::PlanFile,
) -> Result<(), ToolError> {
    for op in ops_list {
        match (plan_mode, op) {
            // in_progress 仅在 executing 允许
            (
                PlanFileMode::Planning | PlanFileMode::Pending,
                UpdateOp::SetStatus {
                    status: TodoStatus::InProgress,
                    ..
                },
            )
            | (
                PlanFileMode::Planning | PlanFileMode::Pending,
                UpdateOp::Upsert {
                    status: Some(TodoStatus::InProgress),
                    ..
                },
            ) => {
                return Err(ToolError::BadArgs(format!(
                    "in_progress 仅允许在 executing 模式下使用；当前 plan.mode = {}",
                    plan_mode.as_str()
                )));
            }
            // executing 期不能新增 milestone（id 不存在），也不能删除 milestone
            (PlanFileMode::Executing, UpdateOp::MilestoneUpsert { id, .. }) => {
                if !plan.frontmatter.milestones.iter().any(|m| &m.id == id) {
                    return Err(ToolError::BadArgs(
                        "executing 模式下不能新增 milestone（仅允许 update 已有 id）".into(),
                    ));
                }
            }
            (PlanFileMode::Executing, UpdateOp::MilestoneRemove { .. }) => {
                return Err(ToolError::BadArgs(
                    "executing 模式下不能删除 milestone".into(),
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

/// E5 / H13：每个 todo.milestone_id 必须存在；返回 warning 文本（不抛错，以
/// 允许 LLM 在跨 op 序列里临时引用未来 milestone 然后补 milestone_upsert）。
fn validate_milestone_refs(
    plan: &crate::api::chat::plan_runtime::file_store::PlanFile,
) -> Result<(), String> {
    let known: std::collections::HashSet<&str> = plan
        .frontmatter
        .milestones
        .iter()
        .map(|m| m.id.as_str())
        .collect();
    let dangling: Vec<&str> = plan
        .frontmatter
        .todos
        .iter()
        .filter_map(|t| t.milestone_id.as_deref())
        .filter(|m| !known.contains(m))
        .collect();
    if dangling.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "todo.milestone_id 引用未知 milestone: {}",
            dangling.join(",")
        ))
    }
}

/// E2：在 `## Todos Board` 的标记区间内重写 todos / milestones 状态视图。
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
    todos: &[crate::api::chat::plan_runtime::file_store::TodoItem],
    milestones: &[crate::api::chat::plan_runtime::file_store::Milestone],
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
    if !milestones.is_empty() {
        rendered.push_str("### Milestones\n");
        for m in milestones {
            let checkbox = match m.status {
                crate::api::chat::plan_runtime::file_store::MilestoneStatus::Completed => "x",
                crate::api::chat::plan_runtime::file_store::MilestoneStatus::InProgress => "~",
                crate::api::chat::plan_runtime::file_store::MilestoneStatus::Pending => " ",
            };
            rendered.push_str(&format!("- [{checkbox}] {}: {}\n", m.id, m.title));
        }
        rendered.push('\n');
    }
    rendered.push_str("### Todos\n");
    if todos.is_empty() {
        rendered.push_str("_(empty)_\n");
    } else {
        use crate::api::chat::plan_runtime::file_store::TodoStatus;
        for t in todos {
            let checkbox = match t.status {
                TodoStatus::Completed => "x",
                TodoStatus::InProgress => "~",
                TodoStatus::Cancelled => "-",
                TodoStatus::Pending => " ",
            };
            let m_suffix = t
                .milestone_id
                .as_deref()
                .map(|m| format!(" (milestone={m})"))
                .unwrap_or_default();
            rendered.push_str(&format!(
                "- [{checkbox}] {}: {}{m_suffix}\n",
                t.id, t.content
            ));
        }
    }
    body.replace_range(body_after_begin..end_idx, &rendered);
}

#[cfg(test)]
mod board_tests {
    use super::*;
    use crate::api::chat::plan_runtime::file_store::{
        Milestone, MilestoneStatus, TodoItem, TodoStatus,
    };

    #[test]
    fn rewrite_todos_board_replaces_between_markers() {
        let mut body = "## Todos Board\n\n<!-- todos-board:auto:begin -->\nOLD CONTENT\n<!-- todos-board:auto:end -->\n".to_string();
        let todos = vec![TodoItem {
            id: "t1".into(),
            content: "step".into(),
            status: TodoStatus::InProgress,
            milestone_id: Some("m1".into()),
        }];
        let milestones = vec![Milestone {
            id: "m1".into(),
            title: "M".into(),
            todo_ids: vec!["t1".into()],
            status: MilestoneStatus::InProgress,
            description: None,
        }];
        rewrite_todos_board(&mut body, &todos, &milestones);
        assert!(!body.contains("OLD CONTENT"));
        assert!(body.contains("- [~] t1: step (milestone=m1)"));
        assert!(body.contains("- [~] m1: M"));
        // 标记必须保留
        assert!(body.contains("todos-board:auto:begin"));
        assert!(body.contains("todos-board:auto:end"));
    }

    #[test]
    fn rewrite_todos_board_noop_without_markers() {
        let original = "## Todos Board\n\nno markers here\n".to_string();
        let mut body = original.clone();
        rewrite_todos_board(&mut body, &[], &[]);
        assert_eq!(body, original);
    }
}
