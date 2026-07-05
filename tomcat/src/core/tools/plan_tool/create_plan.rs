//! `create_plan` 工具实现（plan-runtime.md §P2 / [create-plan.md]）。
//!
//! 语义：
//! - 仅 `Planning` 模式可见；EXEC/CHAT/Pending/Completed 调用 → `InvisibleInMode`。
//! - 整盘写入 `~/.tomcat/plans/<plan_id>.plan.md`；runtime 拼 frontmatter（state/session/created_at/schema_version）。
//! - P4 前 `review` 字段返回 `aborted: true` 占位（reviewer 子 Agent 在 P4 接入）。
//! - 写盘后 PlanRuntime 内存切换为 `Planning`（已是 Planning 时不变），保持 active_plan_id。
//!
//! 注意：本 P2 PR-PLB 实现尚**不**直接在 `tool_exec.rs` 注册分发；将在 P6 PR-PLC
//! 把 4 个 plan 工具同时接入 LLM tool_call 路径。当前仅作为 PlanRuntime 内部 API
//! 给 §9.3B / §9.4 单测与集成测调用。

use serde::Deserialize;

use crate::core::plan_runtime::{
    file_store::{
        plan_path_for_id, write_plan, PlanFile, PlanFileFrontmatter, PlanFileState, TodoItem,
        TodoStatus, PLAN_FILE_SCHEMA_VERSION,
    },
    ops,
    safety::assert_plan_id_safe,
    state::PlanState,
    PlanRuntime,
};

use super::ToolError;

/// `create_plan` 入参 schema（与 catalog parameters 对齐）。
///
/// **D3 破坏性变更（2026-05）**：
/// - 移除 `plan_id`：由 runtime 通过 [`derive_plan_id`] 派生（slug + hash），
///   LLM 传 `plan_id` 将报 [`ToolError::BadArgs`]；
/// - `body` 重命名为 `draft`：保留为入参名，但其内容会被规范化后写入 `## Plan` 段，
///   其它段落由模板拼接，
///   传 `body` 将报 [`ToolError::BadArgs`]；
#[derive(Debug, Deserialize)]
pub struct CreatePlanArgs {
    /// 高层目标（必填）。runtime 由此派生 plan_id。
    pub goal: String,
    /// 计划正文要点（必填）。runtime 会把它规范化后写入 `## Plan` 段。
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
}

fn default_pending() -> TodoStatus {
    TodoStatus::Pending
}

impl CreatePlanArgs {
    /// 从 OpenAI tool_call `arguments` JSON 反序列化。
    ///
    /// D3 破坏性：检测 LLM 误传旧字段 `plan_id` / `body` 时立即报错，避免静默覆盖派生 id。
    pub fn from_json(raw: &serde_json::Value) -> Result<Self, ToolError> {
        if let Some(obj) = raw.as_object() {
            if obj.contains_key("plan_id") {
                return Err(ToolError::BadArgs(
                    "create_plan 不再接受 plan_id；runtime 由 goal 派生".into(),
                ));
            }
            if obj.contains_key("body") {
                return Err(ToolError::BadArgs(
                    "create_plan 字段 body 已重命名为 draft（承载计划正文要点，落盘到 `## Plan` 段）".into(),
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

/// `create_plan` 执行体；返回 ToolResult 内容 JSON：
///
/// ```json
/// { "plan_id": "...", "path": "...", "state": "planning",
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
    if !matches!(mode, PlanState::Planning) {
        return Err(ToolError::InvisibleInMode {
            tool: "create_plan",
            mode: mode.as_str().to_string(),
        });
    }
    if args.goal.trim().is_empty() {
        return Err(ToolError::BadArgs("goal 不可为空".into()));
    }
    if args.draft.trim().is_empty() {
        return Err(ToolError::BadArgs("draft 不可为空".into()));
    }
    if args.todos.is_empty() {
        return Err(ToolError::BadArgs("todos 至少 1 项".into()));
    }
    // G4：runtime 由 goal 派生 plan_id；LLM 不传 plan_id。
    let plan_id = derive_plan_id(&args.goal);
    assert_plan_id_safe(&plan_id)
        .map_err(|e| ToolError::BadArgs(format!("派生 plan_id 非法: {e}")))?;

    let todos: Vec<TodoItem> = args
        .todos
        .iter()
        .map(|t| TodoItem {
            id: t.id.clone(),
            content: t.content.clone(),
            status: t.status,
        })
        .collect();
    // 复用 ops 引擎的不变量校验：duplicate id / single in_progress
    let mut v = Vec::with_capacity(todos.len());
    let add_ops: Vec<_> = todos
        .iter()
        .map(|t| ops::TodoOp::AddTodo(t.clone()))
        .collect();
    ops::apply_todos_ops(&mut v, &add_ops)?;

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
        unknown: serde_yaml::Mapping::new(),
    };
    let body = default_body(&args.goal, &args.draft);
    let plan = PlanFile { frontmatter, body };
    let path = plan_path_for_id(&plan_id)?;
    write_plan(&path, &plan, runtime.lock_timeout_ms())?;

    runtime.set_active_planning_plan(plan_id.clone(), path.clone());
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
        "review": crate::core::plan_runtime::review::ReviewSummary::placeholder_pending().to_json(),
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
    use crate::core::plan_runtime::file_store::{plan_path_for_id, read_plan};
    let path = plan_path_for_id(plan_id)?;
    // 当前 PlanRuntime 不缓存 plan 内容（todos 直接在 disk 上读改）；
    // 这里仅做 read 一次以验证仍可解析（防御 D7：reviewer 写坏文件）。
    let _ = read_plan(&path)?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DraftHeadingKind {
    Goal,
    Plan,
    Notes,
    Other,
}

fn top_level_heading(line: &str) -> Option<String> {
    line.trim()
        .strip_prefix("## ")
        .map(|heading| heading.trim().to_string())
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

fn push_normalized_section(
    goal: &str,
    heading: Option<&str>,
    lines: &mut Vec<String>,
    out: &mut Vec<String>,
) {
    let body = lines.join("\n").trim().to_string();
    lines.clear();
    if body.is_empty() {
        return;
    }

    match heading {
        None => out.push(body),
        Some(heading) => match classify_heading(heading) {
            DraftHeadingKind::Goal => {
                if normalize_for_compare(&body) != normalize_for_compare(goal) {
                    out.push(body);
                }
            }
            DraftHeadingKind::Plan | DraftHeadingKind::Notes => out.push(body),
            DraftHeadingKind::Other => out.push(format!("### {heading}\n\n{body}")),
        },
    }
}

fn strip_top_level_headings(draft: &str) -> String {
    let mut out = Vec::new();
    for line in draft.lines() {
        match top_level_heading(line) {
            Some(heading) => {
                if classify_heading(&heading) == DraftHeadingKind::Other {
                    out.push(format!("### {heading}"));
                }
            }
            None => out.push(line.to_string()),
        }
    }
    out.join("\n").trim().to_string()
}

fn normalize_plan_body(goal: &str, draft: &str) -> String {
    let draft = draft.trim();
    if draft.is_empty() {
        return String::new();
    }

    let mut saw_top_level_heading = false;
    let mut current_heading: Option<String> = None;
    let mut current_lines: Vec<String> = Vec::new();
    let mut sections: Vec<String> = Vec::new();

    for line in draft.lines() {
        if let Some(heading) = top_level_heading(line) {
            saw_top_level_heading = true;
            push_normalized_section(
                goal,
                current_heading.as_deref(),
                &mut current_lines,
                &mut sections,
            );
            current_heading = Some(heading);
        } else {
            current_lines.push(line.to_string());
        }
    }
    push_normalized_section(
        goal,
        current_heading.as_deref(),
        &mut current_lines,
        &mut sections,
    );

    if !saw_top_level_heading {
        return draft.to_string();
    }

    let normalized = sections
        .into_iter()
        .filter(|section| !section.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
        .trim()
        .to_string();
    if !normalized.is_empty() {
        return normalized;
    }

    let stripped = strip_top_level_headings(draft);
    if stripped.is_empty() {
        draft.to_string()
    } else {
        stripped
    }
}

fn default_body(goal: &str, draft: &str) -> String {
    let plan = normalize_plan_body(goal, draft);
    format!(
        "## Goal\n\n{goal}\n\n## Plan\n\n{plan}\n\n## Todos Board\n\n<!-- todos-board:auto:begin -->\n（由 update_plan 自动维护，请勿手工编辑标记之间内容）\n<!-- todos-board:auto:end -->\n"
    )
}
