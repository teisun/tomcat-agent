//! `create_plan` 工具实现（plan-runtime.md §P2 / [create-plan.md]）。
//!
//! 语义：
//! - 仅 `Planning` 模式可见；EXEC/CHAT/Pending/Completed 调用 → `InvisibleInMode`。
//! - 整盘写入 `~/.tomcat/plans/<plan_id>.plan.md`；runtime 拼 frontmatter（mode/session/created_at/schema_version）。
//! - P4 前 `review` 字段返回 `aborted: true` 占位（reviewer 子 Agent 在 P4 接入）。
//! - 写盘后 PlanRuntime 内存切换为 `Planning`（已是 Planning 时不变），保持 active_plan_id。
//!
//! 注意：本 P2 PR-PLB 实现尚**不**直接在 `tool_exec.rs` 注册分发；将在 P6 PR-PLC
//! 把 4 个 plan 工具同时接入 LLM tool_call 路径。当前仅作为 PlanRuntime 内部 API
//! 给 §9.3B / §9.4 单测与集成测调用。

use serde::Deserialize;

use crate::api::chat::plan_runtime::{
    file_store::{
        plan_path_for_id, write_plan, Milestone, PlanFile, PlanFileFrontmatter, PlanFileMode,
        TodoItem, TodoStatus, PLAN_FILE_SCHEMA_VERSION,
    },
    mode::PlanMode,
    ops,
    safety::assert_plan_id_safe,
    PlanRuntime,
};

use super::ToolError;

/// `create_plan` 入参 schema（与 catalog parameters 对齐）。
#[derive(Debug, Deserialize)]
pub struct CreatePlanArgs {
    /// 计划 id；必须通过 `assert_plan_id_safe`（仅 `[a-z0-9_-]+`）。
    pub plan_id: String,
    /// 高层目标。
    pub goal: String,
    /// 自由 markdown body（`## Goal` / `## Draft` 等段落）。
    #[serde(default)]
    pub body: Option<String>,
    pub milestones: Vec<MilestoneArg>,
    pub todos: Vec<TodoArg>,
}

#[derive(Debug, Deserialize)]
pub struct MilestoneArg {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub todo_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct TodoArg {
    pub id: String,
    pub content: String,
    #[serde(default = "default_pending")]
    pub status: TodoStatus,
    #[serde(default)]
    pub milestone_id: Option<String>,
}

fn default_pending() -> TodoStatus {
    TodoStatus::Pending
}

impl CreatePlanArgs {
    /// 从 OpenAI tool_call `arguments` JSON 反序列化。
    pub fn from_json(raw: &serde_json::Value) -> Result<Self, ToolError> {
        serde_json::from_value(raw.clone())
            .map_err(|e| ToolError::BadArgs(format!("create_plan args: {e}")))
    }
}

/// `create_plan` 执行体；返回 ToolResult 内容 JSON：
///
/// ```json
/// { "plan_id": "...", "path": "...", "mode": "planning",
///   "review": { "aborted": true, "summary": "P4 接入" } }
/// ```
/// `create_plan` 同步执行（不派发 reviewer）；返回写盘成功后的核心信息。
/// 当 PlanRuntime 注入了 `ReviewerDispatcher` 时，调用方应使用
/// [`execute_with_reviewer`] 以获得真实的 review summary。
pub fn execute(
    runtime: &PlanRuntime,
    args: CreatePlanArgs,
) -> Result<serde_json::Value, ToolError> {
    let mode = runtime.mode();
    if !matches!(mode, PlanMode::Planning) {
        return Err(ToolError::InvisibleInMode {
            tool: "create_plan",
            mode: mode.as_str().to_string(),
        });
    }
    assert_plan_id_safe(&args.plan_id)
        .map_err(|e| ToolError::BadArgs(format!("plan_id 非法: {e}")))?;
    if args.goal.trim().is_empty() {
        return Err(ToolError::BadArgs("goal 不可为空".into()));
    }

    let todos: Vec<TodoItem> = args
        .todos
        .iter()
        .map(|t| TodoItem {
            id: t.id.clone(),
            content: t.content.clone(),
            status: t.status,
            milestone_id: t.milestone_id.clone(),
        })
        .collect();
    // 复用 ops 引擎的不变量校验：duplicate id / single in_progress
    let mut v = Vec::with_capacity(todos.len());
    let add_ops: Vec<_> = todos
        .iter()
        .map(|t| ops::TodoOp::AddTodo(t.clone()))
        .collect();
    ops::apply_todos_ops(&mut v, &add_ops)?;

    let milestones: Vec<Milestone> = args
        .milestones
        .into_iter()
        .map(|m| Milestone {
            id: m.id,
            title: m.title,
            todo_ids: m.todo_ids,
        })
        .collect();

    let now = chrono::Local::now().to_rfc3339();
    let frontmatter = PlanFileFrontmatter {
        plan_id: args.plan_id.clone(),
        goal: args.goal.clone(),
        mode: PlanFileMode::Planning,
        session_key: None,
        session_id: None,
        created_at: now,
        schema_version: PLAN_FILE_SCHEMA_VERSION,
        milestones,
        todos,
        unknown: serde_yaml::Mapping::new(),
    };
    let body = args
        .body
        .unwrap_or_else(|| default_body(&args.goal));
    let plan = PlanFile { frontmatter, body };
    let path = plan_path_for_id(&args.plan_id)?;
    write_plan(&path, &plan, runtime.lock_timeout_ms())?;

    // 内存切换：保持 Planning，记录 active_plan_id 给后续 update_plan / build 用。
    runtime.set_active_planning_plan_id(args.plan_id.clone());

    Ok(serde_json::json!({
        "plan_id": args.plan_id,
        "path": path.display().to_string(),
        "mode": "planning",
        "review": {
            "aborted": true,
            "summary": "reviewer 子 Agent 将在 P4 接入；当前阶段返回 aborted 占位",
            "changes_summary": "none",
            "applied_changes": false,
        }
    }))
}

/// 同 `execute`，但在写盘成功后**同步**派发 reviewer 子 Agent。
///
/// 顺序严格遵守 RV14：write_plan 完成（lock 已释放）→ dispatch_reviewer。
/// reviewer 解析失败 / max_turns / 父 abort 都不影响 create_plan 成功；
/// 摘要写入 ToolResult.review。
pub async fn execute_with_reviewer(
    runtime: &PlanRuntime,
    args: CreatePlanArgs,
    allow_review_edit: bool,
) -> Result<serde_json::Value, ToolError> {
    let mut out = execute(runtime, args)?;
    let plan_id = out["plan_id"].as_str().unwrap_or("").to_string();
    // 由 PlanRuntime 自洽派发；advisory lock 已在 write_plan 内 drop。
    let summary = runtime.dispatch_reviewer(&plan_id, allow_review_edit).await;
    // 若 reviewer 通过 update_plan / edit 改了 plan 文件 → reload 内存视图
    // （目前 P2 内存仅持 active_planning_plan_id；具体 reload 字段在 P7 PR-PLE 接 panel 时扩展）
    if summary.applied_changes {
        let _ = reload_after_review(runtime, &plan_id);
    }
    out["review"] = summary.to_json();
    Ok(out)
}

fn reload_after_review(_runtime: &PlanRuntime, plan_id: &str) -> Result<(), ToolError> {
    use crate::api::chat::plan_runtime::file_store::{plan_path_for_id, read_plan};
    let path = plan_path_for_id(plan_id)?;
    // 当前 PlanRuntime 不缓存 plan 内容（todos 直接在 disk 上读改）；
    // 这里仅做 read 一次以验证仍可解析（防御 D7：reviewer 写坏文件）。
    let _ = read_plan(&path)?;
    Ok(())
}

fn default_body(goal: &str) -> String {
    format!(
        "## Goal\n\n{goal}\n\n## Draft\n\n（待 reviewer 子 Agent 填入草案要点）\n\n## Review\n\n（等待 reviewer 子 Agent 写入或保持空白）\n\n## Todos Board\n\n（由 update_plan / todos 自动维护）\n"
    )
}
