//! `create_plan`：Plan 模式保存计划，并在明确同意后进行 advisory review。
//!
//! `execute` 校验并带锁写入 PlanFile，刷新 ActivePlan，记录创建/待办事件；
//! 产品入口 `execute_for_tool` 在释放文件锁后通过现有问答面板征求评审同意。
//! 跳过、中断和评审失败都保留已保存计划，不构成 `/plan build` 门禁。
//! 待办增量修改由 `update_plan` 负责，本工具不接收指定 plan_id 重写旧计划。

use serde::Deserialize;

use crate::core::plan_runtime::{
    file_store::{
        plan_path_for_id, validate_single_acceptance, write_plan, PlanFile, PlanFileFrontmatter,
        PlanFileState, TodoItem, TodoKind, TodoStatus, PLAN_FILE_SCHEMA_VERSION,
    },
    ops,
    safety::assert_plan_id_safe,
    PlanRuntime,
};
use crate::core::session::AgentMode;

use super::ToolError;

/// `create_plan` 入参 schema（与 catalog parameters 对齐）。
///
/// **D3 破坏性变更（2026-05）**：
/// - 移除 `plan_id`：由 runtime 通过 [`derive_plan_id`] 派生（slug + hash），
///   LLM 传 `plan_id` 将报 [`ToolError::BadArgs`]；
/// - `body` 重命名为 `draft`：Goal 与自动待办区之间的完整方案正文，
///   runtime 补外壳，仅兼容处理旧包装；
///   传 `body` 将报 [`ToolError::BadArgs`]；
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatePlanArgs {
    /// 高层目标（必填）。runtime 由此派生 plan_id。
    pub goal: String,
    /// 完整方案正文（必填），可包含二级章节。runtime 保留合法正文与层级，仅去旧包装。
    pub draft: String,
    /// 任务列表（必填，至少 1 项）。
    pub todos: Vec<TodoArg>,
}

#[derive(Debug, Deserialize)]
pub struct TodoArg {
    pub id: String,
    pub content: String,
    #[serde(default = "default_pending")]
    pub status: TodoStatus,
    /// `acceptance` only marks the one final acceptance todo in a coding plan.
    #[serde(default)]
    pub kind: TodoKind,
}

fn default_pending() -> TodoStatus {
    TodoStatus::Pending
}

use crate::infra::i18n::tr;

impl CreatePlanArgs {
    /// 从 OpenAI tool_call `arguments` JSON 反序列化。
    ///
    /// D3 破坏性：检测 LLM 误传旧字段 `plan_id` / `body` 时立即报错，避免静默覆盖派生 id。
    pub fn from_json(raw: &serde_json::Value) -> Result<Self, ToolError> {
        if let Some(obj) = raw.as_object() {
            if obj.contains_key("plan_id") {
                return Err(ToolError::BadArgs(
                    "create_plan no longer accepts plan_id; runtime derives it from goal".into(),
                ));
            }
            if obj.contains_key("body") {
                return Err(ToolError::BadArgs(
                    "create_plan field body was renamed to draft (complete plan-body content)"
                        .into(),
                ));
            }
        }
        serde_json::from_value(raw.clone())
            .map_err(|e| ToolError::BadArgs(format!("create_plan args: {e}")))
    }
}

/// 由 `goal` 派生稳定的 `plan_id`：slug（截 40 字符）+ 8 字符 xxh32 hex。
///
/// hash 输入混入当前 ms 时间戳，避免同一 goal 在毫秒内重复 create 产生同 id。
/// 派生结果通过 `assert_plan_id_safe` 校验。
pub fn derive_plan_id(goal: &str) -> String {
    let mut slug = String::new();
    let mut last_was_underscore = false;
    for c in goal.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
            last_was_underscore = false;
            continue;
        }
        if !last_was_underscore {
            slug.push('_');
            last_was_underscore = true;
        }
    }
    let slug = slug
        .trim_matches('_')
        .chars()
        .take(40)
        .collect::<String>()
        .trim_matches('_')
        .to_string();
    let slug = if slug.is_empty() {
        "plan".to_string()
    } else {
        slug
    };
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let seed = format!("{goal}{now_ms}");
    let hash = xxhash_rust::xxh32::xxh32(seed.as_bytes(), 0);
    format!("plan_{slug}_{hash:08x}")
}

/// 仅保存计划并返回核心信息与未派发的 review 占位。
/// 产品交互入口使用 [`execute_for_tool`]，在保存后征求同意，再决定是否评审。
pub fn execute(
    runtime: &PlanRuntime,
    args: CreatePlanArgs,
) -> Result<serde_json::Value, ToolError> {
    let mode = runtime.mode();
    if mode != AgentMode::Plan {
        return Err(ToolError::RejectedInMode {
            tool: "create_plan",
            mode: mode.as_str().to_string(),
            guidance: crate::infra::i18n::tr("planTool.createGuidance", &[]),
        });
    }
    if args.goal.trim().is_empty() {
        return Err(ToolError::BadArgs("goal cannot be empty".into()));
    }
    if args.draft.trim().is_empty() {
        return Err(ToolError::BadArgs("draft cannot be empty".into()));
    }
    if args.todos.is_empty() {
        return Err(ToolError::BadArgs(
            "todos must contain at least one item".into(),
        ));
    }
    if args
        .todos
        .iter()
        .any(|todo| matches!(todo.kind, TodoKind::Unknown))
    {
        return Err(ToolError::BadArgs(
            "create_plan.todos[].kind supports only work or acceptance".into(),
        ));
    }
    // G4：runtime 由 goal 派生 plan_id；LLM 不传 plan_id。
    let plan_id = derive_plan_id(&args.goal);
    assert_plan_id_safe(&plan_id)
        .map_err(|e| ToolError::BadArgs(format!("Derived plan_id is invalid: {e}")))?;

    let todos: Vec<TodoItem> = args
        .todos
        .iter()
        .map(|t| TodoItem {
            id: t.id.clone(),
            content: t.content.clone(),
            status: t.status,
            evidence: Vec::new(),
            kind: t.kind,
        })
        .collect();
    // 复用 ops 引擎的不变量校验：duplicate id / bounded in_progress
    let mut v = Vec::with_capacity(todos.len());
    let add_ops: Vec<_> = todos
        .iter()
        .map(|t| ops::TodoOp::AddTodo(t.clone()))
        .collect();
    ops::apply_todos_ops(&mut v, &add_ops)?;
    validate_single_acceptance(&todos).map_err(|error| ToolError::BadArgs(error.to_string()))?;

    let now = chrono::Local::now().to_rfc3339();
    let frontmatter = PlanFileFrontmatter {
        plan_id: plan_id.clone(),
        goal: args.goal.clone(),
        state: PlanFileState::Planning,
        session_key: None,
        session_id: None,
        created_at: now,
        schema_version: PLAN_FILE_SCHEMA_VERSION,
        todos,
        code_review_pass: false,
        code_review_rounds: 0,
        code_review_open_findings: Vec::new(),
        code_review_disputed_findings: Vec::new(),
        code_review_handoff: false,
        code_review_handoff_acknowledged: false,
        code_review_residual_findings: Vec::new(),
        unknown: serde_yaml::Mapping::new(),
    };
    let body = default_body(&args.goal, &args.draft);
    let plan = PlanFile { frontmatter, body };
    let path = plan_path_for_id(&plan_id)?;
    write_plan(&path, &plan, runtime.lock_timeout_ms())?;

    runtime.refresh_active_plan_after_write(path.clone(), &plan);
    let event_payload = crate::infra::events::PlanEventPayload {
        plan_id: plan_id.clone(),
        path: crate::infra::platform::format_home_path(&path),
        state: PlanFileState::Planning.as_str().to_string(),
    };
    runtime.write_transcript_custom(serde_json::json!({
        "event": crate::infra::wire::WIRE_PLAN_CREATE,
        "plan_id": event_payload.plan_id,
        "path": event_payload.path,
        "state": event_payload.state,
    }));
    runtime.write_transcript_custom(serde_json::json!({
        "event": crate::infra::wire::WIRE_PLAN_TODOS,
        "plan_id": plan_id,
        "todos": super::shared_todo_ops::items_json(&plan.frontmatter.todos),
    }));

    Ok(serde_json::json!({
        "plan_id": plan_id,
        "path": crate::infra::platform::format_home_path(&path),
        "state": "planning",
        "items": super::shared_todo_ops::items_json(&plan.frontmatter.todos),
        "review": crate::core::plan_runtime::plan_reviewer::PlanReviewSummary::placeholder_pending().to_json(),
    }))
}

/// Save first, then ask for explicit review consent through the existing question panel.
///
/// 顺序严格遵守 RV14：write_plan 完成（lock 已释放）→ dispatch_reviewer。
/// reviewer 解析失败 / max_turns / 父 abort 都不影响 create_plan 成功；
/// 摘要写入 ToolResult.review。
pub async fn execute_with_reviewer(
    runtime: &PlanRuntime,
    args: CreatePlanArgs,
    allow_review_edit: bool,
) -> Result<serde_json::Value, ToolError> {
    execute_for_tool(runtime, args, allow_review_edit, Default::default(), None).await
}

pub async fn execute_for_tool(
    runtime: &PlanRuntime,
    args: CreatePlanArgs,
    allow_review_edit: bool,
    termination: crate::core::plan_runtime::AskQuestionTermination,
    tool_call_id: Option<&str>,
) -> Result<serde_json::Value, ToolError> {
    use crate::core::plan_runtime::panels::{
        AskQuestionIdentity, AskQuestionOutcome, Question, QuestionOption,
    };
    use crate::core::plan_runtime::plan_reviewer::PlanReviewSummary;
    use crate::infra::i18n::tr;
    let mut out = execute(runtime, args)?;
    let plan_id = out["plan_id"].as_str().unwrap_or("").to_string();
    use crate::core::plan_runtime::PlanReviewPolicy;
    let policy = runtime.interaction_policy().review;
    let summary = if termination.reason().is_some() {
        let summary = PlanReviewSummary::interrupted();
        runtime.record_plan_review(&plan_id, &summary, 0);
        summary
    } else if policy == PlanReviewPolicy::Auto {
        runtime.dispatch_reviewer(&plan_id, allow_review_edit).await
    } else if policy == PlanReviewPolicy::Skip {
        let mut summary = PlanReviewSummary::skipped();
        summary.reviewer_stop_reason = "policy_skipped".into();
        summary.summary = tr("plan.review.policySkipped", &[]);
        runtime.record_plan_review(&plan_id, &summary, 0);
        summary
    } else if let Some(panel) = runtime.ask_question_panel() {
        let result = panel
            .ask_with_identity(
                AskQuestionIdentity {
                    session_id: runtime.current_session_id(),
                    tool_call_id: tool_call_id.map(str::to_owned),
                },
                vec![Question {
                    id: "plan-review".into(),
                    allow_custom: false,
                    prompt: tr("plan.review.prompt", &[]),
                    options: vec![
                        QuestionOption {
                            id: "review".into(),
                            label: tr("plan.review.accept", &[]),
                            recommended: true,
                        },
                        QuestionOption {
                            id: "skip".into(),
                            label: tr("plan.review.skip", &[]),
                            recommended: false,
                        },
                    ],
                }],
                termination.clone(),
            )
            .await;
        if termination.reason().is_some()
            || matches!(
                result.outcome,
                AskQuestionOutcome::Interrupted
                    | AskQuestionOutcome::HostDisconnected
                    | AskQuestionOutcome::CancelledUnknown
            )
        {
            let summary = PlanReviewSummary::interrupted();
            runtime.record_plan_review(&plan_id, &summary, 0);
            summary
        } else if result.is_single_choice("plan-review", "review") {
            runtime.dispatch_reviewer(&plan_id, allow_review_edit).await
        } else {
            let summary = PlanReviewSummary::skipped();
            runtime.record_plan_review(&plan_id, &summary, 0);
            summary
        }
    } else {
        let summary = PlanReviewSummary::interrupted();
        runtime.record_plan_review(&plan_id, &summary, 0);
        summary
    };
    // 若 reviewer 通过 update_plan / edit 改了计划文件，刷新 active plan 缓存。
    if summary.applied_changes {
        let _ = reload_after_review(runtime, &plan_id);
    }
    out["review"] = summary.to_json();
    Ok(out)
}

fn reload_after_review(runtime: &PlanRuntime, plan_id: &str) -> Result<(), ToolError> {
    use crate::core::plan_runtime::file_store::{plan_path_for_id, read_plan};
    let path = plan_path_for_id(plan_id)?;
    let plan = read_plan(&path)?;
    runtime.refresh_active_plan_after_write(path, &plan);
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DraftHeadingKind {
    Goal,
    Plan,
    Notes,
    Other,
}

fn top_level_heading(line: &str) -> Option<&str> {
    let content = line.trim_start_matches(' ');
    if line.len() - content.len() > 3 {
        return None;
    }
    content.strip_prefix("## ").map(str::trim)
}

fn classify_heading(heading: &str) -> DraftHeadingKind {
    match heading.trim().to_ascii_lowercase().as_str() {
        "goal" => DraftHeadingKind::Goal,
        "draft" | "plan" => DraftHeadingKind::Plan,
        "notes" => DraftHeadingKind::Notes,
        _ => DraftHeadingKind::Other,
    }
}

fn normalize_for_compare(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

// Drop blank boundary lines; the first content line's indentation is Markdown.
fn trim_blank_lines(text: &str) -> &str {
    let text = text.trim_end();
    let first = text.len() - text.trim_start().len();
    &text[text[..first].rfind('\n').map_or(0, |i| i + 1)..]
}

fn normalize_plan_body(goal: &str, draft: &str) -> String {
    let draft = trim_blank_lines(draft);
    // Ordinary chapters, including a standalone Notes chapter, are not wrappers.
    if !matches!(
        draft
            .lines()
            .next()
            .and_then(top_level_heading)
            .map(classify_heading),
        Some(DraftHeadingKind::Goal | DraftHeadingKind::Plan)
    ) {
        return draft.to_string();
    }

    // Keep byte ranges instead of rebuilding Markdown: only recognized legacy
    // wrapper headings (and an exactly duplicated goal) may be removed.
    let mut headings = Vec::new();
    let mut fence: Option<(u8, usize)> = None;
    let mut offset = 0;
    for line in draft.split_inclusive('\n') {
        let marker_line = line.trim_start_matches(' ');
        if line.len() - marker_line.len() <= 3 {
            if let Some(marker @ (b'`' | b'~')) = marker_line.bytes().next() {
                let width = marker_line.bytes().take_while(|b| *b == marker).count();
                if width >= 3 {
                    let tail = &marker_line[width..];
                    if let Some((open_marker, open_width)) = fence {
                        if marker == open_marker && width >= open_width && tail.trim().is_empty() {
                            fence = None;
                        }
                    } else if marker == b'~' || !tail.contains('`') {
                        fence = Some((marker, width));
                    }
                    offset += line.len();
                    continue;
                }
            }
        }
        if fence.is_none() {
            if let Some(heading) = top_level_heading(line) {
                headings.push((offset, offset + line.len(), classify_heading(heading)));
            }
        }
        offset += line.len();
    }

    let mut out = String::new();
    for (i, &(start, content_start, kind)) in headings.iter().enumerate() {
        let end = headings.get(i + 1).map_or(draft.len(), |h| h.0);
        let content = &draft[content_start..end];
        match kind {
            DraftHeadingKind::Goal
                if normalize_for_compare(content) == normalize_for_compare(goal) => {}
            DraftHeadingKind::Goal | DraftHeadingKind::Plan | DraftHeadingKind::Notes => {
                out.push_str(content);
            }
            DraftHeadingKind::Other => out.push_str(&draft[start..end]),
        }
    }
    trim_blank_lines(&out).to_string()
}

fn default_body(goal: &str, draft: &str) -> String {
    let plan = normalize_plan_body(goal, draft);
    let notice = tr("planTool.autoBoard", &[]);
    format!(
        "## Goal\n\n{goal}\n\n## Plan\n\n{plan}\n\n## Todos Board\n\n<!-- todos-board:auto:begin -->\n{notice}\n<!-- todos-board:auto:end -->\n"
    )
}
