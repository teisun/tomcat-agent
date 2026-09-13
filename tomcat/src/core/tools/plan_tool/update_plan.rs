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
    file_store::{
        read_plan, update_plan_locked, write_plan, GreenBuildEvidence, PlanFileState, TodoItem,
        TodoKind, TodoStatus, GATE_ACCEPTANCE_TODO_ID, GATE_CODE_REVIEW_TODO_ID,
    },
    review::{Finding, SeverityTier},
    NextAction, PlanRuntime,
};

use super::shared_todo_ops::{apply_shared_todo_ops, items_json, StatusUpdateMetadata};
use super::ToolError;

#[derive(Debug, Deserialize)]
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
    /// 主 Agent 对当轮 P1 finding 的书面申辩。修复不走此字段：改代码后重新收口，
    /// 让下一轮 reviewer 从代码复核。
    #[serde(default)]
    pub dispute_findings: Vec<DisputeFindingArg>,
    /// 仅在 verify skill 完成后置 true；运行时会用后台 bash 任务账本验证证据。
    #[serde(default)]
    pub green_build_pass: Option<bool>,
    #[serde(default)]
    pub green_build_evidence: Vec<GreenBuildEvidenceArg>,
}

pub use super::shared_todo_ops::SharedTodoOpArg as UpdateOp;

#[derive(Debug, Deserialize)]
pub struct DisputeFindingArg {
    /// JSON 仍使用 `ref`，避免把 Rust 关键字泄漏进工具契约。
    #[serde(rename = "ref")]
    pub reference: String,
    /// 只供人读和事后排查；匹配唯一认 `ref`。
    pub area: String,
    pub resolution: String,
    pub reason: String,
}

#[derive(Debug, Deserialize)]
pub struct GreenBuildEvidenceArg {
    /// 人类可读的验收命令；必须与后台任务实际启动的命令一致，防止伪造证据描述。
    pub command: String,
    pub task_id: String,
}

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

/// Apply a plan update through the visible close-out state machine.
///
/// Work todo completion deliberately does not dispatch a reviewer. The model must make the
/// runtime-created review gate visible and then start it; this keeps the only user-visible
/// scheduling surface (the todo list) aligned with the actual completion protocol.
pub async fn execute_for_tool(
    runtime: &PlanRuntime,
    args: UpdatePlanArgs,
    tool_call_id: &str,
) -> Result<serde_json::Value, ToolError> {
    let path = resolve_target_plan_path(runtime, args.plan_id.clone(), args.path.clone())?;
    let target_plan = read_plan(&path)
        .map_err(|error| ToolError::BadArgs(format!("读取目标 plan 失败：{error}")))?;
    let target_plan_id = target_plan.frontmatter.plan_id.clone();
    let prepared_disputes = prepare_disputes(
        &target_plan.frontmatter.code_review_open_findings,
        &args.dispute_findings,
        target_plan.frontmatter.code_review_handoff_acknowledged,
    )?;

    struct UpdateTxOutcome {
        plan: crate::core::plan_runtime::file_store::PlanFile,
        plan_state_before: PlanFileState,
        warnings: Vec<String>,
        gate_start: Option<GateStart>,
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
        let completion_evidence_warnings =
            completion_evidence_warnings(&plan.frontmatter.todos, &args.ops);

        let (gate_start, mut warnings) = apply_plan_todo_ops(
            &mut plan.frontmatter.todos,
            &args.ops,
            args.replace,
            plan.frontmatter.code_review_pass,
        )?;
        warnings.extend(completion_evidence_warnings);
        for (finding, reason) in &prepared_disputes {
            plan.frontmatter.code_review_disputed_findings.push(
                crate::core::plan_runtime::DisputedFinding {
                    reference: finding.reference.clone(),
                    severity: finding.severity.clone(),
                    area: finding.area.clone(),
                    note: finding.note.clone(),
                    resolution: "wontfix".into(),
                    reason: reason.clone(),
                },
            );
            plan.frontmatter
                .code_review_open_findings
                .retain(|item| item.reference != finding.reference);
        }

        // A reopened completed plan must become writable before the runtime can guide it through
        // a fresh close-out cycle. New plans always own their two gates, so this is derived from
        // the persisted todo state rather than from the incoming op shape.
        if matches!(plan_state_before, PlanFileState::Completed)
            && !plan_completion_ready(&plan.frontmatter.todos)
        {
            plan.frontmatter.state = PlanFileState::Pending;
            warnings.push(
                "plan was reopened because its close-out gates are no longer complete".into(),
            );
        }

        rewrite_todos_board(&mut plan.body, &plan.frontmatter.todos);
        Ok(UpdateTxOutcome {
            plan: plan.clone(),
            plan_state_before,
            warnings,
            gate_start,
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
    let mut warnings = tx.warnings;
    let mut code_review_json = serde_json::Value::Null;
    let mut diff_context = crate::core::plan_runtime::code_reviewer::CodeDiffContext::default();
    let mut unreviewed_edit_event = None;

    for (finding, reason) in prepared_disputes {
        let reference = finding.reference.clone();
        runtime.write_transcript_custom(serde_json::json!({
            "event": "plan.code_review.dispute",
            "plan_id": &target_plan_id,
            "finding": crate::core::plan_runtime::DisputedFinding {
                reference,
                severity: finding.severity,
                area: finding.area,
                note: finding.note,
                resolution: "wontfix".into(),
                reason,
            },
        }));
        warnings.push(format!(
            "P1 finding {} 已作为已知取舍记录；下一轮 reviewer 会收到“勿重报”说明",
            finding.reference
        ));
    }

    // Before the code-review budget is exhausted, code edits make both gates stale. Afterwards,
    // acceptance owns the remaining verification: reopening review only produces another
    // fail-open exhaustion event and prevents a fresh acceptance proof from being submitted.
    if code_gate_state_needs_freshness_check(&plan.frontmatter) {
        if let Some(workspace_root) = runtime.workspace_root() {
            diff_context = crate::core::plan_runtime::code_reviewer::collect_code_diff_context(
                &workspace_root,
            )
            .await;
            if let Some(mtime) = diff_context.newest_edit_mtime_ms {
                if code_review_is_stale(&plan.frontmatter, mtime) {
                    let had_previous_full_gate =
                        plan.frontmatter.code_review_pass && plan.frontmatter.green_build_pass;
                    let review_budget_exhausted =
                        runtime.code_review_budget_exhausted(&target_plan_id);
                    if review_budget_exhausted && !plan.frontmatter.green_build_pass {
                        // This is the normal acceptance-in-progress case. `pass_at_ms` records
                        // the same budget fail-open decision that the old reopen→exhausted round
                        // would have recorded, so this edit is not reconsidered on every later
                        // update_plan call. Keep residual findings: they remain relevant until
                        // the plan is completed.
                        plan.frontmatter.code_review_pass_at_ms = Some(now_unix_ms());
                        warnings.push(format!(
                            "code review 预算已用尽（{}/{}）；本次代码改动不再复审，继续用 Acceptance 的新鲜绿构建证据收口",
                            runtime.code_review_rounds(&target_plan_id),
                            runtime.max_code_review_rounds(),
                        ));
                        unreviewed_edit_event =
                            Some((diff_context.changed_code_files.clone(), mtime));
                    } else if had_previous_full_gate
                        && plan.frontmatter.completion_gate_cycles
                            >= runtime.max_completion_gate_cycles()
                    {
                        warnings.push(format!(
                            "代码在已通过门禁后再次修改，但验收重跑已达到上限 {}；按上限放行收口",
                            runtime.max_completion_gate_cycles()
                        ));
                        runtime_complete_all_gates(&mut plan.frontmatter.todos);
                        plan.frontmatter.state = PlanFileState::Completed;
                    } else if review_budget_exhausted && plan.frontmatter.green_build_pass {
                        invalidate_acceptance_gate_only(&mut plan.frontmatter);
                        plan.frontmatter.completion_gate_cycles =
                            plan.frontmatter.completion_gate_cycles.saturating_add(1);
                    } else {
                        invalidate_code_gates(&mut plan.frontmatter);
                        if had_previous_full_gate {
                            plan.frontmatter.completion_gate_cycles =
                                plan.frontmatter.completion_gate_cycles.saturating_add(1);
                        }
                    }
                    rewrite_todos_board(&mut plan.body, &plan.frontmatter.todos);
                    write_plan(&path, &plan, runtime.lock_timeout_ms())?;
                    runtime.refresh_active_plan_after_write(path.clone(), &plan);
                }
            }
        }
    }

    if let Some((changed_code_files, newest_edit_mtime_ms)) = unreviewed_edit_event {
        runtime.write_code_review_unreviewed_edit_transcript(
            &target_plan_id,
            runtime.code_review_rounds(&target_plan_id),
            &changed_code_files,
            newest_edit_mtime_ms,
        );
    }

    if matches!(tx.gate_start, Some(GateStart::Review)) {
        let next_action = runtime.next_action(&plan.frontmatter).await;
        if !matches!(next_action, NextAction::StartReview) {
            return Err(ToolError::BadArgs(next_action_hint(&next_action)));
        }
        if diff_context.changed_code_files.is_empty() && diff_context.newest_edit_mtime_ms.is_none()
        {
            if let Some(workspace_root) = runtime.workspace_root() {
                diff_context = crate::core::plan_runtime::code_reviewer::collect_code_diff_context(
                    &workspace_root,
                )
                .await;
            }
        }

        if runtime.workspace_root().is_none() || diff_context.changed_code_files.is_empty() {
            runtime_complete_all_gates(&mut plan.frontmatter.todos);
            plan.frontmatter.code_review_pass = true;
            plan.frontmatter.green_build_pass = true;
            finalize_plan_completed(runtime, &target_plan_id, &path, &mut plan)?;
        } else if runtime.review_infra_retries(&target_plan_id) > 2 {
            warnings
                .push("code review 连续技术故障已超过 2 次，gate 已重新打开并交还用户决定".into());
            runtime_set_gate_status(
                &mut plan.frontmatter.todos,
                TodoKind::GateCodeReview,
                TodoStatus::Pending,
            );
            write_code_review_handoff(
                runtime,
                &target_plan_id,
                runtime.code_review_rounds(&target_plan_id),
            );
            rewrite_todos_board(&mut plan.body, &plan.frontmatter.todos);
            write_plan(&path, &plan, runtime.lock_timeout_ms())?;
            runtime.refresh_active_plan_after_write(path.clone(), &plan);
        } else if runtime.has_code_reviewer()
            && !runtime.code_review_budget_exhausted(&target_plan_id)
        {
            let next_round = runtime
                .code_review_rounds(&target_plan_id)
                .saturating_add(1);
            let review_attempt_id = format!("{target_plan_id}:{next_round}");
            let is_incremental =
                next_round > 1 && plan.frontmatter.code_review_baseline_ms.is_some();
            let delta_file_count = if is_incremental {
                runtime
                    .workspace_root()
                    .map(|workspace_root| {
                        crate::core::plan_runtime::code_reviewer::changed_files_since(
                            &workspace_root,
                            &diff_context.changed_code_files,
                            plan.frontmatter.code_review_baseline_ms.unwrap_or_default(),
                        )
                        .len()
                    })
                    .unwrap_or(0)
            } else {
                diff_context.changed_code_files.len()
            };
            let review_lease = runtime
                .begin_code_review_round(
                    &target_plan_id,
                    runtime.code_review_rounds(&target_plan_id),
                    review_attempt_id.clone(),
                    tool_call_id.to_string(),
                    is_incremental,
                    delta_file_count,
                )
                .ok_or_else(|| {
                    ToolError::BadArgs(
                        "code review is already in progress or its review budget is exhausted"
                            .into(),
                    )
                })?;
            let round = review_lease.round();
            let review_baseline_ms = now_unix_ms();
            let dispatch = crate::core::plan_runtime::CodeReviewDispatchInfo {
                round,
                review_attempt_id: review_attempt_id.clone(),
                tool_call_id: tool_call_id.to_string(),
                is_incremental,
                delta_file_count,
            };
            let mut summary = runtime
                .dispatch_code_reviewer(
                    &target_plan_id,
                    &plan.frontmatter.code_review_open_findings,
                    &dispatch,
                )
                .await;
            warnings.extend(summary.normalize_for_result());
            if summary.verdict.as_deref() == Some("aborted") {
                summary.aborted = true;
            }
            code_review_json = summary.to_json();
            if summary.aborted {
                let retries = runtime.bump_review_infra_retry(&target_plan_id);
                if retries > 2 {
                    warnings.push(
                        "code review 连续技术故障已超过 2 次，gate 已重新打开并交还用户决定".into(),
                    );
                    write_code_review_handoff(runtime, &target_plan_id, round);
                } else {
                    warnings.push(format!(
                        "code review 技术故障（{}）：gate 已重新打开，将允许第 {}/2 次基础设施重试",
                        summary.reviewer_stop_reason, retries
                    ));
                }
                // `abort` 消费 lease；其 Drop 负责落一条终态 aborted event，
                // 不写 review 状态，也不消耗持久化轮次。
                review_lease.abort(summary);
            } else {
                let disputed = plan.frontmatter.code_review_disputed_findings.clone();
                let blocking = blocking_findings(&summary.findings, &disputed);
                let verdict_is_aborted = summary.verdict.as_deref() == Some("aborted");
                if !blocking.is_empty() || verdict_is_aborted {
                    if summary.verdict.as_deref() == Some("pass") && !blocking.is_empty() {
                        warnings.push(
                            "code reviewer verdict=pass 但仍返回未裁决 P0/P1 finding；运行时按 finding 阻止收口"
                                .into(),
                        );
                    }
                    plan.frontmatter.code_review_open_findings = blocking;
                    plan.frontmatter.code_review_pass = false;
                    plan.frontmatter.code_review_pass_at_ms = None;
                    runtime_set_gate_status(
                        &mut plan.frontmatter.todos,
                        TodoKind::GateCodeReview,
                        TodoStatus::Pending,
                    );
                } else {
                    plan.frontmatter.code_review_open_findings.clear();
                    plan.frontmatter.code_review_residual_findings.clear();
                    record_code_review_pass(&mut plan.frontmatter, false);
                    runtime_set_gate_status(
                        &mut plan.frontmatter.todos,
                        TodoKind::GateCodeReview,
                        TodoStatus::Completed,
                    );
                }
                plan.frontmatter.code_review_rounds = round;
                plan.frontmatter.code_review_baseline_ms = Some(review_baseline_ms);
                rewrite_todos_board(&mut plan.body, &plan.frontmatter.todos);
                write_plan(&path, &plan, runtime.lock_timeout_ms())?;
                runtime.refresh_active_plan_after_write(path.clone(), &plan);
                runtime.write_code_review_transcript(
                    &target_plan_id,
                    &summary,
                    round,
                    &review_attempt_id,
                    tool_call_id,
                    is_incremental,
                    delta_file_count,
                );
                review_lease.complete();
            }
        } else if !runtime.has_code_reviewer() || runtime.max_code_review_rounds() == 0 {
            warnings.push(format!(
                "code review 未启用（dispatcher={}，max_code_review_rounds = {}），记录为跳过复审",
                if runtime.has_code_reviewer() {
                    "available"
                } else {
                    "unavailable"
                },
                runtime.max_code_review_rounds(),
            ));
            plan.frontmatter.code_review_residual_findings.clear();
            record_code_review_pass(&mut plan.frontmatter, false);
            runtime_set_gate_status(
                &mut plan.frontmatter.todos,
                TodoKind::GateCodeReview,
                TodoStatus::Completed,
            );
            rewrite_todos_board(&mut plan.body, &plan.frontmatter.todos);
            write_plan(&path, &plan, runtime.lock_timeout_ms())?;
            runtime.refresh_active_plan_after_write(path.clone(), &plan);
        } else {
            let residual_findings =
                format_residual_findings(&runtime.unresolved_findings(&target_plan_id));
            let has_p0 = plan
                .frontmatter
                .code_review_open_findings
                .iter()
                .any(|finding| finding.tier() == SeverityTier::P0);
            plan.frontmatter.code_review_residual_findings = residual_findings.clone();
            if has_p0 {
                warnings.push(format!(
                    "code review 轮次预算已用尽（{}/{}）且仍有 P0；已暂停无人值守执行并交还用户",
                    runtime.code_review_rounds(&target_plan_id),
                    runtime.max_code_review_rounds(),
                ));
                plan.frontmatter.code_review_handoff = true;
                plan.frontmatter.code_review_pass = false;
                plan.frontmatter.code_review_pass_at_ms = None;
                runtime_set_gate_status(
                    &mut plan.frontmatter.todos,
                    TodoKind::GateCodeReview,
                    TodoStatus::Pending,
                );
                runtime_set_gate_status(
                    &mut plan.frontmatter.todos,
                    TodoKind::GateAcceptance,
                    TodoStatus::Pending,
                );
            } else {
                warnings.push(format!(
                    "code review 轮次预算已用尽（{}/{}）；仅剩 P1，带 {} 条残余 finding 进入 acceptance",
                    runtime.code_review_rounds(&target_plan_id),
                    runtime.max_code_review_rounds(),
                    residual_findings.len(),
                ));
                record_code_review_pass(&mut plan.frontmatter, false);
                runtime_set_gate_status(
                    &mut plan.frontmatter.todos,
                    TodoKind::GateCodeReview,
                    TodoStatus::Completed,
                );
                runtime.write_code_review_exhausted_transcript(
                    &target_plan_id,
                    runtime.code_review_rounds(&target_plan_id),
                    &residual_findings,
                );
            }
            rewrite_todos_board(&mut plan.body, &plan.frontmatter.todos);
            write_plan(&path, &plan, runtime.lock_timeout_ms())?;
            runtime.refresh_active_plan_after_write(path.clone(), &plan);
            if has_p0 {
                write_code_review_p0_handoff(
                    runtime,
                    &target_plan_id,
                    runtime.code_review_rounds(&target_plan_id),
                    &residual_findings,
                );
            }
        }
    }

    if matches!(tx.gate_start, Some(GateStart::Acceptance))
        && !runtime.begin_acceptance(&target_plan_id)
    {
        return Err(ToolError::BadArgs(
            "`[gate] Acceptance` is already in progress in this runtime; submit its task evidence or cancel the plan before starting it again"
                .into(),
        ));
    }

    if args.green_build_pass.is_some() {
        if !plan.frontmatter.code_review_pass {
            return Err(ToolError::BadArgs(
                "`[gate] review` 尚未通过，不能提交 acceptance 绿构建证据".into(),
            ));
        }
        if args.green_build_pass == Some(true) {
            if !runtime.acceptance_is_in_flight(&target_plan_id) {
                return Err(ToolError::BadArgs(
                    "只能在 `[gate] Acceptance` 已启动的当前运行时提交 green_build_pass；请先将该 gate 设为 in_progress，再运行 verify skill"
                        .into(),
                ));
            }
            if diff_context.newest_edit_mtime_ms.is_none() {
                if let Some(workspace_root) = runtime.workspace_root() {
                    diff_context =
                        crate::core::plan_runtime::code_reviewer::collect_code_diff_context(
                            &workspace_root,
                        )
                        .await;
                }
            }
            let newest_edit_mtime_ms = diff_context.newest_edit_mtime_ms.ok_or_else(|| {
                ToolError::BadArgs(
                    "当前没有可核验的代码 diff；docs-only 计划应由 `[gate] review` 自动跳过".into(),
                )
            })?;
            require_green_build_pass(runtime, &args, newest_edit_mtime_ms, &path, &mut plan)?;
            runtime.finish_acceptance(&target_plan_id);
            runtime_set_gate_status(
                &mut plan.frontmatter.todos,
                TodoKind::GateAcceptance,
                TodoStatus::Completed,
            );
            rewrite_todos_board(&mut plan.body, &plan.frontmatter.todos);
            if plan_completion_ready(&plan.frontmatter.todos) {
                finalize_plan_completed(runtime, &target_plan_id, &path, &mut plan)?;
            } else {
                write_plan(&path, &plan, runtime.lock_timeout_ms())?;
                runtime.refresh_active_plan_after_write(path.clone(), &plan);
            }
        } else {
            runtime.finish_acceptance(&target_plan_id);
            plan.frontmatter.green_build_pass = false;
            plan.frontmatter.green_build_evidence.clear();
            write_plan(&path, &plan, runtime.lock_timeout_ms())?;
            runtime.refresh_active_plan_after_write(path.clone(), &plan);
        }
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
        "code_review": code_review_json,
        "code_review_pass": plan.frontmatter.code_review_pass,
        "green_build_pass": plan.frontmatter.green_build_pass,
        "next_step": next_action_json(&next_action),
    }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GateStart {
    Review,
    Acceptance,
}

fn runtime_gate_kind_for_id(id: &str) -> Option<TodoKind> {
    match id {
        GATE_CODE_REVIEW_TODO_ID => Some(TodoKind::GateCodeReview),
        GATE_ACCEPTANCE_TODO_ID => Some(TodoKind::GateAcceptance),
        _ => None,
    }
}

fn required_gate(todos: &[TodoItem], kind: TodoKind) -> Result<&TodoItem, ToolError> {
    todos.iter().find(|todo| todo.kind == kind).ok_or_else(|| {
        ToolError::BadArgs(format!(
            "plan 缺少 runtime-managed {} gate；请重新 create_plan",
            kind.as_str()
        ))
    })
}

/// Extract runtime-owned gate transitions, then let the existing shared op engine mutate only
/// ordinary work items. `replace=true` is intentionally a reconstruction of work todos plus the
/// old gates: it cannot erase gates or synthesize a Work item with a gate id.
fn apply_plan_todo_ops(
    todos: &mut Vec<TodoItem>,
    ops_list: &[UpdateOp],
    replace: bool,
    code_review_pass: bool,
) -> Result<(Option<GateStart>, Vec<String>), ToolError> {
    let original_review = required_gate(todos, TodoKind::GateCodeReview)?.clone();
    let original_acceptance = required_gate(todos, TodoKind::GateAcceptance)?.clone();
    if original_review.id != GATE_CODE_REVIEW_TODO_ID
        || original_acceptance.id != GATE_ACCEPTANCE_TODO_ID
    {
        return Err(ToolError::BadArgs(
            "runtime gate ids are malformed; recreate the plan instead of editing gate todos"
                .into(),
        ));
    }

    let mut requested_gate_start = None;
    let mut work_ops = Vec::with_capacity(ops_list.len());
    for op in ops_list {
        let (id, has_metadata, status, is_remove) = match op {
            UpdateOp::Upsert {
                id,
                content,
                status,
            } => (id.as_str(), content.is_some(), *status, false),
            UpdateOp::SetStatus {
                id,
                content,
                status,
            } => (id.as_str(), content.is_some(), Some(*status), false),
            UpdateOp::Remove { id, content, .. } => (id.as_str(), content.is_some(), None, true),
        };
        let Some(gate_kind) = runtime_gate_kind_for_id(id) else {
            work_ops.push(op.clone());
            continue;
        };

        if replace {
            return Err(ToolError::BadArgs(
                "replace=true may contain only work todos; runtime re-injects both close-out gates"
                    .into(),
            ));
        }
        if is_remove || has_metadata || status != Some(TodoStatus::InProgress) {
            return Err(ToolError::BadArgs(format!(
                "{} is runtime-managed: it may only be set to in_progress",
                match gate_kind {
                    TodoKind::GateCodeReview => "[gate] review",
                    TodoKind::GateAcceptance => "[gate] Acceptance",
                    TodoKind::Work => unreachable!(),
                }
            )));
        }

        let start = match gate_kind {
            TodoKind::GateCodeReview => GateStart::Review,
            TodoKind::GateAcceptance => GateStart::Acceptance,
            TodoKind::Work => unreachable!(),
        };
        if let Some(previous) = requested_gate_start {
            if previous != start {
                return Err(ToolError::BadArgs(
                    "start one close-out gate per update_plan call".into(),
                ));
            }
        }
        requested_gate_start = Some(start);
    }

    if replace {
        apply_shared_todo_ops(todos, &work_ops, true)?;
        todos.push(original_review);
        todos.push(original_acceptance);
    } else {
        apply_shared_todo_ops(todos, &work_ops, false)?;
    }

    if let Some(gate_start) = requested_gate_start {
        match gate_start {
            GateStart::Review => {
                if !all_work_todos_terminal(todos) {
                    return Err(ToolError::BadArgs(
                        "`[gate] review` may start only after every work todo is completed or cancelled"
                            .into(),
                    ));
                }
                if required_gate(todos, TodoKind::GateCodeReview)?.status != TodoStatus::Pending {
                    return Err(ToolError::BadArgs(
                        "`[gate] review` is not pending and cannot be started again; if its configured review budget has already been exhausted, continue with `[gate] Acceptance` instead"
                            .into(),
                    ));
                }
            }
            GateStart::Acceptance => {
                if !code_review_pass {
                    return Err(ToolError::BadArgs(
                        "`[gate] Acceptance` may start only after `[gate] review` passes".into(),
                    ));
                }
                if required_gate(todos, TodoKind::GateAcceptance)?.status != TodoStatus::Pending {
                    return Err(ToolError::BadArgs(
                        "`[gate] Acceptance` is not pending and cannot be started again".into(),
                    ));
                }
            }
        }
    }

    let mut warnings = Vec::new();
    if replace {
        warnings.push(
            "replace=true preserved runtime-managed `[gate] review` and `[gate] Acceptance` todos"
                .into(),
        );
    }
    Ok((requested_gate_start, warnings))
}

fn all_work_todos_terminal(todos: &[TodoItem]) -> bool {
    todos
        .iter()
        .filter(|todo| matches!(todo.kind, TodoKind::Work))
        .all(|todo| matches!(todo.status, TodoStatus::Completed | TodoStatus::Cancelled))
}

fn gate_has_status(todos: &[TodoItem], kind: TodoKind, status: TodoStatus) -> bool {
    todos
        .iter()
        .any(|todo| todo.kind == kind && todo.status == status)
}

fn runtime_set_gate_status(todos: &mut [TodoItem], kind: TodoKind, status: TodoStatus) {
    if let Some(gate) = todos.iter_mut().find(|todo| todo.kind == kind) {
        gate.status = status;
    }
}

fn runtime_complete_all_gates(todos: &mut [TodoItem]) {
    runtime_set_gate_status(todos, TodoKind::GateCodeReview, TodoStatus::Completed);
    runtime_set_gate_status(todos, TodoKind::GateAcceptance, TodoStatus::Completed);
}

fn plan_completion_ready(todos: &[TodoItem]) -> bool {
    all_work_todos_terminal(todos)
        && gate_has_status(todos, TodoKind::GateCodeReview, TodoStatus::Completed)
        && gate_has_status(todos, TodoKind::GateAcceptance, TodoStatus::Completed)
}

fn code_gate_state_needs_freshness_check(
    frontmatter: &crate::core::plan_runtime::file_store::PlanFileFrontmatter,
) -> bool {
    frontmatter.code_review_pass || frontmatter.green_build_pass
}

fn code_review_is_stale(
    frontmatter: &crate::core::plan_runtime::file_store::PlanFileFrontmatter,
    newest_edit_mtime_ms: u128,
) -> bool {
    frontmatter
        .code_review_pass_at_ms
        .is_none_or(|passed_at| passed_at < newest_edit_mtime_ms)
}

fn acceptance_evidence_requirements() -> &'static str {
    "The gate validates only real background-task evidence: exit 0 and a task started after the newest edit."
}

fn format_residual_findings(findings: &[Finding]) -> Vec<String> {
    findings
        .iter()
        .map(|finding| {
            format!(
                "{} [{}] {}: {}",
                finding.reference, finding.severity, finding.area, finding.note
            )
        })
        .collect()
}

fn run_acceptance_hint(residual_findings: &[String]) -> String {
    let review_context = if residual_findings.is_empty() {
        "Code review passed.".to_string()
    } else {
        format!(
            "Code review reached its configured round budget after addressing findings during those rounds. Residual findings still requiring explicit acceptance handling:\n{}",
            residual_findings
                .iter()
                .map(|finding| format!("- {finding}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
    format!(
        "{review_context}\nVerify ALL changes with the project's acceptance commands: set the `[gate] Acceptance` todo to in_progress, then load_skill(verify) to discover and run the commands (scope proportional to the change), and submit green_build_pass with evidence. {}",
        acceptance_evidence_requirements()
    )
}

fn next_action_json(action: &NextAction) -> serde_json::Value {
    serde_json::json!({
        "phase": action.phase(),
        "hint": next_action_hint(action),
    })
}

fn next_action_hint(action: &NextAction) -> String {
    match action {
        NextAction::Done => String::new(),
        NextAction::HandOff {
            reason,
            open_findings,
        } => format!(
            "Stop unattended execution and hand this plan to the user ({reason}).{}",
            crate::core::plan_runtime::code_reviewer::render_open_findings_section(open_findings)
        ),
        NextAction::RunAcceptance { residual_findings } => run_acceptance_hint(residual_findings),
        NextAction::FixFindings { open_findings } => format!(
            "Fix the following unresolved code-review findings before starting another review:\n{}",
            crate::core::plan_runtime::code_reviewer::render_open_findings_section(open_findings)
        ),
        NextAction::StartReview => "All work todos are done. Set the `[gate] review` todo to in_progress to start close-out. If the change touches no code files, both gates are skipped automatically and the plan completes in this same call.".into(),
        NextAction::ContinueWork => "You are still implementing. Complete the remaining work todos with focused checks; the full acceptance suite belongs to the `[gate] Acceptance` step.".into(),
    }
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

fn now_unix_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn invalidate_code_gates(
    frontmatter: &mut crate::core::plan_runtime::file_store::PlanFileFrontmatter,
) {
    frontmatter.code_review_pass = false;
    frontmatter.code_review_pass_at_ms = None;
    frontmatter.code_review_residual_findings.clear();
    frontmatter.green_build_pass = false;
    frontmatter.green_build_evidence.clear();
    runtime_set_gate_status(
        &mut frontmatter.todos,
        TodoKind::GateCodeReview,
        TodoStatus::Pending,
    );
    runtime_set_gate_status(
        &mut frontmatter.todos,
        TodoKind::GateAcceptance,
        TodoStatus::Pending,
    );
}

/// A fresh verification proof is needed, but the completed review decision remains authoritative.
/// This is used only after the review budget was already exhausted.
fn invalidate_acceptance_gate_only(
    frontmatter: &mut crate::core::plan_runtime::file_store::PlanFileFrontmatter,
) {
    frontmatter.green_build_pass = false;
    frontmatter.green_build_evidence.clear();
    runtime_set_gate_status(
        &mut frontmatter.todos,
        TodoKind::GateAcceptance,
        TodoStatus::Pending,
    );
}

fn record_code_review_pass(
    frontmatter: &mut crate::core::plan_runtime::file_store::PlanFileFrontmatter,
    is_rerun_after_previous_full_gate: bool,
) {
    frontmatter.code_review_pass = true;
    frontmatter.code_review_pass_at_ms = Some(now_unix_ms());
    if is_rerun_after_previous_full_gate {
        frontmatter.completion_gate_cycles = frontmatter.completion_gate_cycles.saturating_add(1);
    }
}

fn green_build_guidance(residual_findings: &[String]) -> ToolError {
    ToolError::BadArgs(format!(
        "代码 diff 已通过（或跳过）code review，但绿构建验收尚未通过。{}",
        run_acceptance_hint(residual_findings)
    ))
}

fn require_green_build_pass(
    runtime: &PlanRuntime,
    args: &UpdatePlanArgs,
    newest_edit_mtime_ms: u128,
    path: &std::path::Path,
    plan: &mut crate::core::plan_runtime::file_store::PlanFile,
) -> Result<(), ToolError> {
    if plan.frontmatter.green_build_pass
        && plan
            .frontmatter
            .green_build_evidence
            .iter()
            .any(|evidence| evidence.started_at_ms >= newest_edit_mtime_ms)
    {
        return Ok(());
    }

    if args.green_build_pass != Some(true) {
        if args.green_build_pass == Some(false) {
            plan.frontmatter.green_build_pass = false;
            plan.frontmatter.green_build_evidence.clear();
            write_plan(path, plan, runtime.lock_timeout_ms())?;
            runtime.refresh_active_plan_after_write(path.to_path_buf(), plan);
        }
        return Err(green_build_guidance(
            &plan.frontmatter.code_review_residual_findings,
        ));
    }
    if args.green_build_evidence.is_empty() {
        return Err(ToolError::BadArgs(
            "green_build_pass=true 必须同时传入至少一个 green_build_evidence.task_id".into(),
        ));
    }
    let Some(registry) = runtime.bash_task_registry() else {
        return Err(ToolError::BadArgs(
            "当前运行时没有后台 bash 任务账本，无法核验绿构建证据；请重新执行 verify skill 中的命令"
                .into(),
        ));
    };

    let mut seen_task_ids = std::collections::BTreeSet::new();
    let mut verified = Vec::with_capacity(args.green_build_evidence.len());
    for evidence in &args.green_build_evidence {
        if evidence.command.trim().is_empty() {
            return Err(ToolError::BadArgs(
                "green_build_evidence.command 不能为空；请填写对应后台 bash 的实际命令".into(),
            ));
        }
        if !seen_task_ids.insert(evidence.task_id.trim()) {
            return Err(ToolError::BadArgs(format!(
                "green_build_evidence.task_id 重复：{}",
                evidence.task_id
            )));
        }
        let task = registry.get_info(evidence.task_id.trim()).ok_or_else(|| {
            ToolError::BadArgs(format!(
                "找不到后台 bash 任务 `{}`；只能引用本会话 verify skill 实际启动的 task_id",
                evidence.task_id
            ))
        })?;
        let crate::core::tools::primitive::BashTaskStatus::Finished { exit_code } = task.status
        else {
            return Err(ToolError::BadArgs(format!(
                "后台任务 `{}` 尚未成功结束；绿构建证据必须是 exit_code=0 的 Finished 任务",
                evidence.task_id
            )));
        };
        if exit_code != 0 {
            return Err(ToolError::BadArgs(format!(
                "后台任务 `{}` 的 exit_code={}，不能作为绿构建通过证据",
                evidence.task_id, exit_code
            )));
        }
        if evidence.command.trim() != task.command.trim() {
            return Err(ToolError::BadArgs(format!(
                "green_build_evidence.command 与后台任务 `{}` 的实际命令不一致；请原样提交任务启动命令",
                evidence.task_id
            )));
        }
        if task.started_at_unix_ms < newest_edit_mtime_ms {
            return Err(ToolError::BadArgs(format!(
                "后台任务 `{}` 启动于最新代码修改之前，证据已过期；请重新运行验收命令",
                evidence.task_id
            )));
        }
        verified.push(GreenBuildEvidence {
            command: task.command,
            task_id: task.task_id.to_string(),
            started_at_ms: task.started_at_unix_ms,
            exit_code,
        });
    }

    plan.frontmatter.green_build_pass = true;
    plan.frontmatter.green_build_evidence = verified;
    write_plan(path, plan, runtime.lock_timeout_ms())?;
    runtime.refresh_active_plan_after_write(path.to_path_buf(), plan);
    runtime.write_transcript_custom(serde_json::json!({
        "event": "plan.green_build",
        "plan_id": &plan.frontmatter.plan_id,
        "pass": true,
        "evidence": &plan.frontmatter.green_build_evidence,
    }));
    Ok(())
}

fn prepare_disputes(
    unresolved: &[Finding],
    disputes: &[DisputeFindingArg],
    p0_handoff_acknowledged: bool,
) -> Result<Vec<(Finding, String)>, ToolError> {
    let mut prepared = Vec::with_capacity(disputes.len());

    for dispute in disputes {
        if dispute.resolution != "wontfix" {
            return Err(ToolError::BadArgs(format!(
                "{} 的 resolution 只支持 \"wontfix\"（申辩）；标记已修请改代码后重新收口",
                dispute.reference
            )));
        }
        let Some(finding) = unresolved
            .iter()
            .find(|finding| finding.reference == dispute.reference)
            .cloned()
        else {
            return Err(ToolError::BadArgs(format!(
                "{}（area=\"{}\"）不在未决清单；它可能已修、已申辩，或只是不会阻塞的 P2",
                dispute.reference, dispute.area
            )));
        };
        match finding.tier() {
            SeverityTier::P0 => {
                if p0_handoff_acknowledged && !dispute.reason.trim().is_empty() {
                    prepared.push((finding, dispute.reason.trim().to_owned()));
                    continue;
                }
                return Err(ToolError::BadArgs(format!(
                    "{} 是 P0，不可申辩，必须修复或交还用户；交还后需由用户发送新消息并提供 reason",
                    dispute.reference
                )));
            }
            SeverityTier::P1 if dispute.reason.trim().is_empty() => {
                return Err(ToolError::BadArgs(format!(
                    "申辩 {} 必须提供 reason",
                    dispute.reference
                )));
            }
            SeverityTier::P1 => prepared.push((finding, dispute.reason.trim().to_owned())),
            SeverityTier::P2 => {
                return Err(ToolError::BadArgs(format!(
                    "{} 是 P2，不会阻塞收口，无需申辩",
                    dispute.reference
                )));
            }
        }
    }

    Ok(prepared)
}

fn normalize_finding_text(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn disputed_matches(
    disputed: &[crate::core::plan_runtime::DisputedFinding],
    finding: &Finding,
) -> bool {
    let area = normalize_finding_text(&finding.area);
    let note = normalize_finding_text(&finding.note);
    disputed.iter().any(|accepted| {
        normalize_finding_text(&accepted.area) == area
            && normalize_finding_text(&accepted.note) == note
    })
}

fn blocking_findings(
    findings: &[Finding],
    disputed: &[crate::core::plan_runtime::DisputedFinding],
) -> Vec<Finding> {
    findings
        .iter()
        .filter(|finding| finding.blocks())
        .filter(|finding| !disputed_matches(disputed, finding))
        .cloned()
        .collect()
}

fn write_code_review_handoff(runtime: &PlanRuntime, plan_id: &str, rounds: u32) {
    runtime.write_transcript_custom(serde_json::json!({
        "event": crate::infra::wire::WIRE_PLAN_CODE_REVIEW_EXHAUSTED,
        "plan_id": plan_id,
        "rounds": rounds,
        "max_code_review_rounds": runtime.max_code_review_rounds(),
        "outcome": "handoff_after_review_infrastructure_failure",
        "code_review_pass": false,
        "unresolved_findings": runtime.unresolved_finding_references(plan_id),
    }));
}

fn write_code_review_p0_handoff(
    runtime: &PlanRuntime,
    plan_id: &str,
    rounds: u32,
    residual_findings: &[String],
) {
    runtime.write_transcript_custom(serde_json::json!({
        "event": crate::infra::wire::WIRE_PLAN_CODE_REVIEW_EXHAUSTED,
        "plan_id": plan_id,
        "rounds": rounds,
        "max_code_review_rounds": runtime.max_code_review_rounds(),
        "outcome": "handoff_after_p0_residual",
        "code_review_pass": false,
        "residual_findings": residual_findings,
    }));
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

/// Completion without proof remains valid for compatibility, but is conspicuous to the executor
/// and reviewer. Gates never accept model-provided evidence and are excluded above.
fn completion_evidence_warnings(todos: &[TodoItem], ops_list: &[UpdateOp]) -> Vec<String> {
    ops_list
        .iter()
        .filter_map(|op| match op {
            UpdateOp::SetStatus {
                id,
                status: TodoStatus::Completed,
                content,
                ..
            } if content
                .as_ref()
                .and_then(StatusUpdateMetadata::evidence)
                .is_none_or(|evidence| evidence.is_empty())
                && todos
                    .iter()
                    .any(|todo| todo.id == *id && todo.kind == TodoKind::Work) =>
            {
                Some(format!(
                    "work todo `{id}` 已完成但未记录 evidence；请在下一次 update_plan 的 set_status 中补充实际验证或交付说明"
                ))
            }
            _ => None,
        })
        .collect()
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
    fn green_build_bad_args_reuses_the_run_acceptance_hint() {
        assert!(green_build_guidance(&[])
            .to_string()
            .contains(&run_acceptance_hint(&[])));
    }

    #[test]
    fn acceptance_hint_carries_residual_findings_without_claiming_a_fixed_findings_ledger() {
        let residuals = vec!["F01 [P1] oauth: retry path is untested".to_string()];
        let hint = run_acceptance_hint(&residuals);

        assert!(hint.contains("configured round budget"));
        assert!(hint.contains("addressing findings during those rounds"));
        assert!(hint.contains("Verify ALL changes"));
        assert!(hint.contains(&residuals[0]));
        assert!(!hint.contains("Fixed findings:"));
    }
}
