//! `update_plan` 工具实现（plan-runtime.md §P2 / [update-plan.md] / G1+G2+N2 2026-05）。
//!
//! 语义：
//! - 任何模式可见；按 `plan_id` / `path` 路由（`plan_id` 优先，缺省取 active plan）。
//! - 入参 **仅认 `kind`**（D3 破坏性）：`upsert | set_status | remove`。
//! - Mode 矩阵闸门（G2 / `update-plan.md` §6.2）：
//!   - 目标 `plan.mode == completed` → 全拒（N2）。
//!   - `set_status: in_progress` 仅 `executing` 允许；planning / pending 一律拒。
//! - 跨 session 编辑规则：
//!   - 目标 plan `mode ∈ {planning, pending}`：允许（协作改稿）
//!   - 目标 plan `mode == executing` 且 `session_key != current_session_key`：拒
//! - 写盘后 EXEC 自动派生：所有 todos completed → 先写 `Executing`，若 code review
//!   轮次未耗尽则先派发 code reviewer；`verdict=pass` 时同回合 verifier，否则把
//!   `code_review` 返回给主 Agent。code review 轮次耗尽后直接走 verifier。
//! - 返回 JSON（G1）：`plan_id` / `path` / `applied` / `items[]` /
//!   `active_in_progress` / `plan_mode_before` / `plan_mode_after` / `warnings[]` /
//!   `panel_snapshot_id` / `code_review` / `verify`（节流后 panel 刷新版本；目前与 timestamp 等价）。

use std::path::PathBuf;

use serde::Deserialize;

use crate::core::plan_runtime::{
    file_store::{update_plan_locked, write_plan, PlanFileMode, TodoStatus},
    mode::PlanMode,
    ops, review, verify, PlanRuntime,
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
    let path = resolve_target_plan_path(runtime, args.plan_id, args.path)?;
    struct UpdateTxOutcome {
        plan: crate::core::plan_runtime::file_store::PlanFile,
        target_plan_id: String,
        plan_mode_before: PlanFileMode,
        warnings: Vec<String>,
        active_in_progress: Option<String>,
        derived_completed: bool,
    }

    let tx = match update_plan_locked(&path, runtime.lock_timeout_ms(), |plan| {
        let target_plan_id = plan.frontmatter.plan_id.clone();
        let plan_mode_before = plan.frontmatter.mode;

        // N2：completed 全拒。
        if matches!(plan_mode_before, PlanFileMode::Completed) {
            return Err(ToolError::CrossSessionDenied(format!(
                "plan {target_plan_id} 已 completed，无法再编辑"
            )));
        }

        enforce_cross_session_policy(runtime, &plan.frontmatter, plan_mode_before)?;

        // G2 mode 矩阵闸门：先做语义校验，再下沉到 ops 引擎。
        enforce_mode_matrix(plan_mode_before, &args.ops)?;

        apply_shared_todo_ops(&mut plan.frontmatter.todos, &args.ops, args.replace)?;

        let warnings: Vec<String> = Vec::new();
        let derived_completed = matches!(plan_mode_before, PlanFileMode::Executing)
            && ops::all_completed(&plan.frontmatter.todos);

        // E2：在 body 的 `## Todos Board` 标记区间内自动重写当前 todos 状态视图。
        rewrite_todos_board(&mut plan.body, &plan.frontmatter.todos);

        if derived_completed {
            // 第一写：todos 完成，但 mode 保持 Executing，确保 verifier/code reviewer 看到的是
            // 「已做完 todos、尚未正式收工」的磁盘态。
            plan.frontmatter.mode = PlanFileMode::Executing;
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
            plan_mode_before,
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
    let plan_mode_before = tx.plan_mode_before;
    let active_in_progress = tx.active_in_progress;
    let mut warnings = tx.warnings;

    let mut code_review_json = serde_json::Value::Null;
    let mut verify_json = serde_json::Value::Null;
    let plan_mode_after = if tx.derived_completed {
        if let Some(round) = runtime.try_begin_code_review_round(&target_plan_id) {
            let mut code_review_summary = runtime.dispatch_code_reviewer(&target_plan_id).await;
            warnings.extend(review::normalize_for_code_review_result(
                &mut code_review_summary,
            ));
            runtime.write_code_review_transcript(&target_plan_id, &code_review_summary, round);
            code_review_json = code_review_summary.to_json();

            if code_review_summary.verdict.as_deref() == Some("pass") {
                let (mode_after, verify_payload) = run_verifier_after_code_review(
                    runtime,
                    &target_plan_id,
                    &path,
                    &mut plan,
                    &mut warnings,
                )
                .await?;
                verify_json = verify_payload;
                mode_after
            } else {
                warnings.push(format!(
                    "code review verdict={}，plan 保持 executing，等待主 Agent 修复或重新 complete",
                    code_review_summary
                        .verdict
                        .as_deref()
                        .unwrap_or("partial")
                ));
                PlanFileMode::Executing
            }
        } else {
            warnings.push(format!(
                "code review rounds 已用尽（{}/{}），跳过 code review 直接 verifier",
                runtime.code_review_rounds(&target_plan_id),
                runtime.max_code_review_rounds()
            ));
            runtime.write_code_review_warning_transcript(
                &target_plan_id,
                "rounds_exhausted",
                runtime.code_review_rounds(&target_plan_id),
            );
            let (mode_after, verify_payload) = run_verifier_after_code_review(
                runtime,
                &target_plan_id,
                &path,
                &mut plan,
                &mut warnings,
            )
            .await?;
            verify_json = verify_payload;
            mode_after
        }
    } else {
        plan.frontmatter.mode
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

    Ok(serde_json::json!({
        "plan_id": target_plan_id,
        "path": crate::infra::platform::format_home_path(&path),
        "applied": applied,
        "replace": args.replace,
        "plan_mode_before": plan_mode_before.as_str(),
        "plan_mode_after": plan_mode_after.as_str(),
        "panel_snapshot_id": panel_snapshot_id,
        "warnings": warnings,
        "active_in_progress": active_in_progress,
        "items": items_json(&plan.frontmatter.todos),
        "code_review": code_review_json,
        "verify": verify_json,
    }))
}

async fn run_verifier_after_code_review(
    runtime: &PlanRuntime,
    target_plan_id: &str,
    path: &std::path::Path,
    plan: &mut crate::core::plan_runtime::file_store::PlanFile,
    warnings: &mut Vec<String>,
) -> Result<(PlanFileMode, serde_json::Value), ToolError> {
    let mut verify_summary = runtime.dispatch_verifier(target_plan_id).await;
    warnings.extend(verify::normalize_for_gate(&mut verify_summary));
    runtime.write_verify_transcript(target_plan_id, &verify_summary);
    let verify_json = verify_summary.to_json();

    let allow_complete = !(runtime.verify_gate_is_strict() && verify_summary.verdict == "fail");
    if allow_complete {
        plan.frontmatter.mode = PlanFileMode::Completed;
        write_plan(path, plan, runtime.lock_timeout_ms())?;
        runtime.set_mode_completed(target_plan_id.to_string());
        Ok((PlanFileMode::Completed, verify_json))
    } else {
        warnings.push("verifier verdict=fail 且 [plan].verify_gate=gate，plan 保持 executing".into());
        Ok((PlanFileMode::Executing, verify_json))
    }
}

fn resolve_target_plan_path(
    runtime: &PlanRuntime,
    explicit_plan_id: Option<String>,
    explicit_path: Option<String>,
) -> Result<PathBuf, ToolError> {
    if let Some(id) = explicit_plan_id {
        return runtime
            .resolved_plan_path(&id)
            .map_err(ToolError::BadArgs);
    }
    if let Some(path) = explicit_path {
        return crate::infra::platform::normalize_path(&path)
            .map_err(|e| ToolError::BadArgs(format!("update_plan path 非法：{e}")));
    }
    if let Some(path) = runtime.active_plan_path() {
        return Ok(path);
    }
    if let PlanMode::Executing { plan_id } | PlanMode::Pending { plan_id } = runtime.mode() {
        return runtime
            .resolved_plan_path(&plan_id)
            .map_err(ToolError::BadArgs);
    }
    if let Some(id) = runtime.active_planning_plan_id() {
        return runtime
            .resolved_plan_path(&id)
            .map_err(ToolError::BadArgs);
    }
    Err(ToolError::BadArgs(
        "update_plan 需要 plan_id 或 path；当前模式无 active plan".into(),
    ))
}

fn enforce_cross_session_policy(
    runtime: &PlanRuntime,
    fm: &crate::core::plan_runtime::file_store::PlanFileFrontmatter,
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
fn enforce_mode_matrix(plan_mode: PlanFileMode, ops_list: &[UpdateOp]) -> Result<(), ToolError> {
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
