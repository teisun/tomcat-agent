//! `update_plan` 工具实现（plan-runtime.md §P2 / [update-plan.md] / G1+G2+N2 2026-05）。
//!
//! 语义：
//! - 任何模式可见；按 `plan_id` / `path` 路由（`plan_id` 优先，缺省取 active plan）。
//! - 入参 **仅认 `kind`**（D3 破坏性）：`upsert | set_status | remove`。
//! - State 矩阵闸门（G2 / `update-plan.md` §6.2）：
//!   - 目标 `plan.state == completed` → 全拒（N2）。
//!   - `set_status: in_progress` 仅 `executing` 允许；planning / pending 一律拒。
//! - 跨 session 编辑规则：
//!   - 目标 plan `state ∈ {planning, pending}`：允许（协作改稿）
//!   - 目标 plan `state == executing` 且 `session_key != current_session_key`：拒
//! - 写盘后 EXEC 自动派生：所有 todos completed → 先写 `Executing`，若 code review
//!   轮次未耗尽则先派发 code reviewer；`verdict=pass` 时同回合 verifier，否则把
//!   `code_review` 返回给主 Agent。code review 轮次耗尽后直接走 verifier。
//! - 返回 JSON（G1）：`plan_id` / `path` / `applied` / `items[]` /
//!   `active_in_progress` / `plan_state_before` / `plan_state_after` / `warnings[]` /
//!   `panel_snapshot_id` / `code_review` / `verify`（节流后 panel 刷新版本；目前与 timestamp 等价）。

use std::path::PathBuf;

use serde::Deserialize;

use crate::core::plan_runtime::{
    file_store::{update_plan_locked, write_plan, PlanFileState, TodoStatus},
    ops,
    state::PlanState,
    PlanRuntime,
};

use super::shared_todo_ops::{apply_shared_todo_ops, items_json};
use super::ToolError;

#[derive(Debug, Deserialize)]
pub struct UpdatePlanArgs {
    /// 目标 plan_id；EXEC 模式可省略（默认 active_plan_id）。
    #[serde(default)]
    pub plan_id: Option<String>,
    /// 可选直接路径；仅在未传 plan_id 时生效。
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub replace: bool,
    /// 增量 ops；统一为 `kind` 标记的 enum（D3 破坏性）。
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
    execute_for_tool(runtime, args, "legacy-update-plan").await
}

pub async fn execute_for_tool(
    runtime: &PlanRuntime,
    args: UpdatePlanArgs,
    tool_call_id: &str,
) -> Result<serde_json::Value, ToolError> {
    let path = resolve_target_plan_path(runtime, args.plan_id, args.path)?;
    struct UpdateTxOutcome {
        plan: crate::core::plan_runtime::file_store::PlanFile,
        target_plan_id: String,
        plan_state_before: PlanFileState,
        warnings: Vec<String>,
        active_in_progress: Option<String>,
        derived_completed: bool,
    }

    let tx = match update_plan_locked(&path, runtime.lock_timeout_ms(), |plan| {
        let target_plan_id = plan.frontmatter.plan_id.clone();
        let plan_state_before = plan.frontmatter.state;

        enforce_cross_session_policy(runtime, &plan.frontmatter, plan_state_before)?;

        // G2 state 矩阵闸门：先做语义校验，再下沉到 ops 引擎。
        enforce_state_matrix(plan_state_before, &args.ops)?;

        apply_shared_todo_ops(&mut plan.frontmatter.todos, &args.ops, args.replace)?;

        let warnings: Vec<String> = Vec::new();
        let all_completed = ops::all_completed(&plan.frontmatter.todos);
        let derived_completed =
            matches!(plan_state_before, PlanFileState::Executing) && all_completed;

        if matches!(plan_state_before, PlanFileState::Completed) && !all_completed {
            plan.frontmatter.state = PlanFileState::Pending;
        }

        // E2：在 body 的 `## Todos Board` 标记区间内自动重写当前 todos 状态视图。
        rewrite_todos_board(&mut plan.body, &plan.frontmatter.todos);

        if derived_completed {
            // 第一写：todos 完成，但 state 保持 Executing，确保 verifier/code reviewer 看到的是
            // 「已做完 todos、尚未正式收工」的磁盘态。
            plan.frontmatter.state = PlanFileState::Executing;
        }

        let active_in_progress = plan
            .frontmatter
            .todos
            .iter()
            .find(|t| matches!(t.status, TodoStatus::InProgress))
            .map(|t| t.id.clone());

        Ok(UpdateTxOutcome {
            plan: plan.clone(),
            target_plan_id,
            plan_state_before,
            warnings,
            active_in_progress,
            derived_completed,
        })
    }) {
        Ok(v) => v,
        Err(crate::core::plan_runtime::file_store::LockedPlanMutationError::Plan(e)) => {
            return Err(e.into());
        }
        Err(crate::core::plan_runtime::file_store::LockedPlanMutationError::Callback(e)) => {
            return Err(e);
        }
    };

    let applied = args.ops.len();
    let mut plan = tx.plan;
    let target_plan_id = tx.target_plan_id;
    let plan_state_before = tx.plan_state_before;
    let active_in_progress = tx.active_in_progress;
    let mut warnings = tx.warnings;

    let mut code_review_json = serde_json::Value::Null;
    if tx.derived_completed {
        write_plan_progress_transcript(runtime, &target_plan_id, &path, &plan);
    }
    let plan_state_after = if tx.derived_completed {
        if let Some(round) = runtime.try_begin_code_review_round(&target_plan_id) {
            let review_attempt_id = format!("{target_plan_id}:{round}");
            runtime.write_code_review_started_transcript(
                &target_plan_id,
                round,
                &review_attempt_id,
                tool_call_id,
                None,
            );
            let mut code_review_summary = runtime.dispatch_code_reviewer(&target_plan_id).await;
            warnings.extend(code_review_summary.normalize_for_result());
            runtime.write_code_review_transcript(
                &target_plan_id,
                &code_review_summary,
                round,
                &review_attempt_id,
                tool_call_id,
            );
            code_review_json = code_review_summary.to_json();

            match code_review_summary.verdict.as_deref() {
                Some("pass") => {
                    finalize_plan_completed(runtime, &target_plan_id, &path, &mut plan)?;
                    PlanFileState::Completed
                }
                Some("fail") | Some("partial") => {
                    warnings.extend(non_pass_code_review_guidance(&code_review_summary));
                    PlanFileState::Executing
                }
                Some("aborted") => {
                    warnings.push(
                        "code review 中止(aborted)，本次按 best-effort 直接收口 completed".into(),
                    );
                    finalize_plan_completed(runtime, &target_plan_id, &path, &mut plan)?;
                    PlanFileState::Completed
                }
                _ => {
                    warnings.push(
                        "code review 未返回可识别 verdict，已按 partial 处理；plan 保持 executing"
                            .into(),
                    );
                    warnings.extend(non_pass_code_review_guidance(&code_review_summary));
                    PlanFileState::Executing
                }
            }
        } else {
            warnings.push(format!(
                "code review rounds 已用尽（{}/{}），本次不再复审，按 best-effort 直接收口 completed",
                runtime.code_review_rounds(&target_plan_id),
                runtime.max_code_review_rounds()
            ));
            runtime.write_code_review_warning_transcript(
                &target_plan_id,
                "rounds_exhausted",
                runtime.code_review_rounds(&target_plan_id),
            );
            finalize_plan_completed(runtime, &target_plan_id, &path, &mut plan)?;
            PlanFileState::Completed
        }
    } else {
        plan.frontmatter.state
    };

    let panel_snapshot_id = crate::core::plan_runtime::panels::next_panel_snapshot_id();

    // E：fanout UI 刷新——advisory lock 在 write_plan 内已 release，这里仅同步通知
    // 已注册 panel；panel 自行决定如何渲染（CLI/IDE/noop）。
    let snapshot = crate::core::plan_runtime::panels::TodosPanelSnapshot {
        panel_snapshot_id,
        scope: format!("plan:{target_plan_id}"),
        items: plan.frontmatter.todos.clone(),
        warnings: warnings.clone(),
    };
    runtime.refresh_notifier().notify(&snapshot);

    if matches!(plan_state_before, PlanFileState::Completed)
        && matches!(plan_state_after, PlanFileState::Pending)
    {
        runtime.set_mode_pending_with_path(target_plan_id.clone(), Some(path.clone()));
    }

    let event_payload = crate::infra::events::PlanEventPayload {
        plan_id: target_plan_id.clone(),
        path: crate::infra::platform::format_home_path(&path),
        state: plan_state_after.as_str().to_string(),
    };
    if !(tx.derived_completed
        || matches!(plan_state_before, PlanFileState::Completed)
            && matches!(plan_state_after, PlanFileState::Pending))
    {
        runtime.write_transcript_custom(serde_json::json!({
            "event": crate::infra::wire::WIRE_PLAN_UPDATE,
            "plan_id": event_payload.plan_id,
            "path": event_payload.path,
            "state": event_payload.state,
        }));
    }
    if !tx.derived_completed {
        runtime.write_transcript_custom(serde_json::json!({
            "event": crate::infra::wire::WIRE_PLAN_TODOS,
            "plan_id": target_plan_id,
            "todos": items_json(&plan.frontmatter.todos),
        }));
    }

    Ok(serde_json::json!({
        "plan_id": target_plan_id,
        "path": crate::infra::platform::format_home_path(&path),
        "applied": applied,
        "replace": args.replace,
        "plan_state_before": plan_state_before.as_str(),
        "plan_state_after": plan_state_after.as_str(),
        "panel_snapshot_id": panel_snapshot_id,
        "warnings": warnings,
        "active_in_progress": active_in_progress,
        "items": items_json(&plan.frontmatter.todos),
        "code_review": code_review_json,
    }))
}

fn write_plan_progress_transcript(
    runtime: &PlanRuntime,
    target_plan_id: &str,
    path: &std::path::Path,
    plan: &crate::core::plan_runtime::file_store::PlanFile,
) {
    runtime.write_transcript_custom(serde_json::json!({
        "event": crate::infra::wire::WIRE_PLAN_UPDATE,
        "plan_id": target_plan_id,
        "path": crate::infra::platform::format_home_path(path),
        "state": PlanFileState::Executing.as_str(),
    }));
    runtime.write_transcript_custom(serde_json::json!({
        "event": crate::infra::wire::WIRE_PLAN_TODOS,
        "plan_id": target_plan_id,
        "todos": items_json(&plan.frontmatter.todos),
    }));
}

fn finalize_plan_completed(
    runtime: &PlanRuntime,
    target_plan_id: &str,
    path: &std::path::Path,
    plan: &mut crate::core::plan_runtime::file_store::PlanFile,
) -> Result<(), ToolError> {
    plan.frontmatter.state = PlanFileState::Completed;
    write_plan(path, plan, runtime.lock_timeout_ms())?;
    runtime.set_mode_completed_with_path(target_plan_id.to_string(), Some(path.to_path_buf()));
    let _ = runtime.finalize_completed_to_chat();
    Ok(())
}

fn non_pass_code_review_guidance(
    summary: &crate::core::plan_runtime::code_reviewer::CodeReviewSummary,
) -> Vec<String> {
    let verdict = summary.verdict.as_deref().unwrap_or("partial");
    let finding_hint = if summary.findings.is_empty() {
        "当前 findings 为空，请根据 code_review.summary 归纳一个修复点。"
    } else {
        "请直接根据 code_review.findings 落修复。"
    };
    vec![
        format!(
            "code review verdict={verdict}，plan 保持 executing。{finding_hint} 用 update_plan 重新打开一个已有 todo（set_status=in_progress），或新增一个修复 todo；修复完成后再次调用 update_plan 收口。"
        ),
        "当前默认只跑 1 轮 code review：这次修复后再次收口将不再复审，而是直接 best-effort completed。"
            .into(),
    ]
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
    if let Some(path) = runtime.active_plan_path() {
        return Ok(path);
    }
    if let PlanState::Executing { plan_id } | PlanState::Pending { plan_id } = runtime.mode() {
        return runtime
            .resolved_plan_path(&plan_id)
            .map_err(ToolError::BadArgs);
    }
    if let Some(id) = runtime.active_planning_plan_id() {
        return runtime.resolved_plan_path(&id).map_err(ToolError::BadArgs);
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
