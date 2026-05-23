//! # reviewer 子 Agent 同步派发（plan-runtime.md §P4 / tools/reviewer.md）
//!
//! reviewer 通过 [`crate::core::agent_registry::AgentRegistry::spawn_subagent_internal`] 派发；
//! `kind=Plan` 保持现有 `allow_review_edit = true`（reviewer.md §5.2 / §5.5 拍板）——
//! `allowed_tools` 为 `{read, search_files, list_dir, todos, update_plan, edit}`；
//! `kind=Code` 则切到只读代码审查模式，`allowed_tools` 为
//! `{read, search_files, list_dir, bash}`。两者都不含 `create_plan` / `write` /
//! `dispatch_agent` / `checkpoint`。
//!
//! 子 Agent 必须最终消息体里 emit 一个 `<review>` block：
//!
//! ```text
//! <review>
//! findings:
//!   - { severity: nit|suggestion|concern, area: "<short>", note: "<one-line>" }
//!   - ...
//! summary: <≤600 chars 自由文本>
//! changes_summary: <none|none-but-noted|applied:<short>>
//! applied_changes: <true|false>
//! </review>
//! ```
//!
//! 解析失败 / 超 `max_turns` / 父 cascade abort → `ReviewSummary { aborted: true, .. }`；
//! `create_plan` 视为成功（plan 文件已落盘），仅在 ToolResult.review 中暴露 `aborted=true`。

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

/// 生产路径 reviewer 改稿权固定开启（reviewer.md §5.2 / §5.5）；
/// `false` 仅供 Mock 单测验证只读工具集 + 守卫。
pub const REVIEWER_ALLOW_REVIEW_EDIT: bool = true;

pub const PLAN_REVIEWER_ALLOWED_TOOLS: &[&str] = &[
    "read",
    "search_files",
    "list_dir",
    "todos",
    "update_plan",
    "edit",
];

pub const CODE_REVIEWER_ALLOWED_TOOLS: &[&str] = &["read", "search_files", "list_dir", "bash"];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReviewKind {
    #[default]
    Plan,
    Code,
}

impl ReviewKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Code => "code",
        }
    }
}

pub fn reviewer_allowed_tools_for(kind: ReviewKind) -> &'static [&'static str] {
    match kind {
        ReviewKind::Plan => PLAN_REVIEWER_ALLOWED_TOOLS,
        ReviewKind::Code => CODE_REVIEWER_ALLOWED_TOOLS,
    }
}

/// reviewer 系统 prompt（对齐 reviewer.md §5.1，已固定 allow_review_edit=true）。
///
/// runtime 在装配 reviewer `AgentLoopConfig` 时把它拼到 system message 尾部；env
/// `TOMCAT_REVIEWER_SYSTEM_PROMPT_OVERRIDE_PATH` 优先级更高。`{{max_turns}}` 占位
/// 由 dispatcher 在拼装时替换为实际配置值。
pub const REVIEWER_SYSTEM_PROMPT: &str = r#"
You are an internal review subagent. You are NOT the user-facing agent.
Your output is advisory — you do not gate or approve downstream workflow steps.

You receive a review brief in the initial user message (what to review, scope,
constraints, and any artifact paths). Treat that brief as the source of truth.

Workflow:
1. Inspect the subject with `read` / `search_files` / `list_dir` (and any
   other read-only tools granted by runtime). The catalog name `search_files`
   covers grep- and find-style queries.
2. Record findings as you go (nit / suggestion / concern).
3. For substantive issues: explore the repo, reason about root cause, and
   formulate fixes. You have edit-capable tools (`edit`, `update_plan`)
   granted — apply proportionate fixes directly; runtime enforces path/scope
   guards (`edit` may modify the target plan body under
   `~/.tomcat/plans/<id>.plan.md` but must NOT raw-edit frontmatter;
   `update_plan` only touches the target plan's todos).
4. End with the required output block below (review opinion + changes summary).

Tools (runtime-filtered; only granted tools are callable):
- read, search_files, list_dir, todos (personal scratchpad only)
- edit, update_plan (write tools, guarded by tool_exec); out-of-scope edits
  return tool errors — adjust and retry or fall back to read-only recommendations.
- create_plan / bash / write / dispatch_agent / checkpoint are NEVER available.

Output contract (must be the final assistant message, exact format):

<review>
findings:
  - { severity: nit|suggestion|concern, area: "<short label>", note: "<one-line concrete remark>" }
  - ...
summary: <review opinion — what you found and overall assessment, <= 600 chars>
changes_summary: <what you changed and why; use "none" if read-only or no edits>
applied_changes: <true | false>
</review>

Rules:
1. Advisory only. Do NOT emit gate verdicts (approve/reject/block) or prescribe
   what the parent agent or user should do next in their workflow.
2. Severities: nit (style/cleanup), suggestion (worth adjusting), concern
   (substantive risk or gap).
3. Stay within the review brief; do not expand scope or invent requirements.
4. Do not modify repository source unless a write tool was granted and runtime
   accepts the target path. When writes are denied, put fix guidance in
   changes_summary as recommendations, not as claims of applied edits.
5. Stay within max_turns reasoning turns; if you cannot finish, emit findings
   gathered so far, note what remains unclear in summary, and set
   changes_summary to "none" or partial edits only.
"#;

pub const CODE_REVIEW_SYSTEM_PROMPT: &str = r#"
You are an internal code review subagent. You are NOT the user-facing agent.
Your output is advisory — the parent agent decides whether to change code, reopen
todos, or continue to verification.

You receive a code review brief in the initial user message with the exact plan,
workspace root, and a runtime-injected git diff summary. Treat that brief as the
source of truth.

Workflow:
1. Inspect the plan, changed files, and nearby implementation with `read` /
   `search_files` / `list_dir`.
2. Use `bash` only for read-only inspection or lightweight evidence gathering
   (for example `git diff`, `cargo test -- --list`, `rg`, or manifest lookup).
3. Record concrete findings as you go (nit / suggestion / concern).
4. You MUST stay read-only: do NOT edit the plan, todos, or repository files.
5. End with the required output block below, including a `verdict`.

Tools (runtime-filtered; only granted tools are callable):
- read, search_files, list_dir, bash
- todos / update_plan / edit / write / create_plan / dispatch_agent / checkpoint
  are NEVER available in code review mode.

Output contract (must be the final assistant message, exact format):

<review>
findings:
  - { severity: nit|suggestion|concern, area: "<short label>", note: "<one-line concrete remark>" }
  - ...
verdict: pass|fail|partial|aborted
summary: <review opinion — what you found and overall assessment, <= 600 chars>
changes_summary: none
applied_changes: false
</review>

Rules:
1. `pass` means you found no material issue that should block immediate verifier follow-up.
2. `fail` means you found at least one concrete bug / risky mismatch / correctness issue
   the parent agent should address before claiming the implementation is solid.
3. `partial` means inspection was inconclusive (for example missing context or weak evidence).
4. `aborted` is reserved for cases where you truly could not continue the review.
5. Stay within the review brief; do not expand scope or invent requirements.
6. Do not claim any edits were applied. `changes_summary` must stay `none` and
   `applied_changes` must stay `false`.
"#;

/// 单条 finding（reviewer.md §5.3）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub severity: String,
    pub area: String,
    pub note: String,
}

/// reviewer 摘要（ToolResult.review 与 transcript.plan.review 共用）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewSummary {
    /// review 类型：Plan（create_plan 后计划审稿）或 Code（verifier 前代码审查）。
    #[serde(default)]
    pub kind: ReviewKind,
    /// 是否被中止（parse 失败 / max_turns / parent abort / spawn 失败）。
    pub aborted: bool,
    /// code reviewer 的描述性 verdict；Plan reviewer 永远为 None。
    #[serde(default)]
    pub verdict: Option<String>,
    /// 自由文本摘要；aborted=true 时含失败原因。
    pub summary: String,
    /// 改动语义说明（`none` / `none-but-noted` / `applied:<short>`）。
    #[serde(default)]
    pub changes_summary: String,
    /// 是否真有改动（reviewer 通过 update_plan / edit 改了文件）。
    pub applied_changes: bool,
    /// 离散挑刺清单（reviewer.md §5.3）。
    #[serde(default)]
    pub findings: Vec<Finding>,
    /// 本次子 AgentLoop 实际跑的 LLM reasoning 轮数（reasoning_loop turn_index 终值）。
    /// 默认 0；占位 summary（未派发）保持 0 即可。
    #[serde(default)]
    pub reviewer_turns_used: u32,
    /// 本次 dispatcher 配置的 max_turns 上限（与 AgentLoopConfig.max_tool_rounds 同档）。
    #[serde(default)]
    pub reviewer_turns_limit: u32,
    /// 收口原因：`completed` / `max_turns` / `parse_error` / `parent_abort` / `spawn_error`。
    #[serde(default)]
    pub reviewer_stop_reason: String,
    /// reviewer 子 Agent session_id（便于 transcript 关联）。
    #[serde(default)]
    pub child_session_id: String,
}

impl ReviewSummary {
    /// 用于占位的"未派发"摘要（PlanRuntime 未注入 dispatcher 时回退）。
    pub fn placeholder_pending() -> Self {
        Self::placeholder_pending_for(ReviewKind::Plan)
    }

    pub fn placeholder_pending_for(kind: ReviewKind) -> Self {
        Self {
            kind,
            aborted: true,
            verdict: match kind {
                ReviewKind::Plan => None,
                ReviewKind::Code => Some("aborted".into()),
            },
            summary: "reviewer 子 Agent 将在 P4 接入；当前阶段返回 aborted 占位".into(),
            changes_summary: "none".into(),
            applied_changes: false,
            findings: Vec::new(),
            reviewer_turns_used: 0,
            reviewer_turns_limit: 0,
            reviewer_stop_reason: "not_dispatched".into(),
            child_session_id: String::new(),
        }
    }

    pub fn aborted_with(reason: impl Into<String>) -> Self {
        Self::aborted_with_kind(ReviewKind::Plan, reason)
    }

    pub fn aborted_with_kind(kind: ReviewKind, reason: impl Into<String>) -> Self {
        Self {
            kind,
            aborted: true,
            verdict: match kind {
                ReviewKind::Plan => None,
                ReviewKind::Code => Some("aborted".into()),
            },
            summary: reason.into(),
            changes_summary: "none".into(),
            applied_changes: false,
            findings: Vec::new(),
            reviewer_turns_used: 0,
            reviewer_turns_limit: 0,
            reviewer_stop_reason: "aborted".into(),
            child_session_id: String::new(),
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "kind": self.kind.as_str(),
            "aborted": self.aborted,
            "verdict": self.verdict,
            "summary": self.summary,
            "changes_summary": self.changes_summary,
            "applied_changes": self.applied_changes,
            "findings": self.findings,
            "reviewer_turns_used": self.reviewer_turns_used,
            "reviewer_turns_limit": self.reviewer_turns_limit,
            "reviewer_stop_reason": self.reviewer_stop_reason,
            "child_session_id": self.child_session_id,
        })
    }
}

/// 严格解析 `<review>...</review>` 块。失败返回 None；多块 → 取**最后一个**。
///
/// 解析约束：
/// - `summary:` 必填，截断到 600 字符
/// - `changes_summary:` 必填，常见值 `none` / `none-but-noted` / `applied:<x>`
/// - `applied_changes:` 必填，`true` / `false`（大小写不敏感）
/// - `verdict:` 可选，若出现则必须是 `pass|fail|partial|aborted`
/// - `findings:` 可选——失败不挡 summary 三必填字段
pub fn parse_review_block(text: &str) -> Option<ReviewSummary> {
    let last_block = find_last_review_block(text)?;
    let mut summary = None;
    let mut changes_summary = None;
    let mut applied = None;
    let mut verdict = None;
    let mut findings: Vec<Finding> = Vec::new();
    let mut in_findings = false;

    for raw_line in last_block.lines() {
        let line = raw_line.trim();
        if let Some(rest) = line.strip_prefix("summary:") {
            in_findings = false;
            summary = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("verdict:") {
            in_findings = false;
            let normalized = rest.trim().to_ascii_lowercase();
            if !matches!(
                normalized.as_str(),
                "pass" | "fail" | "partial" | "aborted"
            ) {
                return None;
            }
            verdict = Some(normalized);
        } else if let Some(rest) = line.strip_prefix("changes_summary:") {
            in_findings = false;
            changes_summary = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("applied_changes:") {
            in_findings = false;
            applied = match rest.trim().to_ascii_lowercase().as_str() {
                "true" => Some(true),
                "false" => Some(false),
                _ => return None,
            };
        } else if line == "findings:" || line.starts_with("findings:") {
            // `findings:` 起始；列表项在后续行
            in_findings = true;
        } else if in_findings {
            if let Some(item) = parse_finding_line(line) {
                findings.push(item);
            }
        }
    }
    let mut summary = summary?;
    if summary.len() > 600 {
        summary.truncate(600);
    }
    let changes_summary = changes_summary?;
    let applied = applied?;
    Some(ReviewSummary {
        kind: ReviewKind::Plan,
        aborted: false,
        verdict,
        summary,
        changes_summary,
        applied_changes: applied,
        findings,
        reviewer_turns_used: 0,
        reviewer_turns_limit: 0,
        reviewer_stop_reason: "completed".into(),
        child_session_id: String::new(),
    })
}

pub fn normalize_for_code_review_result(summary: &mut ReviewSummary) -> Vec<String> {
    let mut warnings = Vec::new();
    summary.kind = ReviewKind::Code;

    if summary.aborted {
        if summary.verdict.as_deref() != Some("aborted") {
            summary.verdict = Some("aborted".into());
            warnings.push("code review 中止，verdict 已规范化为 aborted".into());
        }
        summary.applied_changes = false;
        return warnings;
    }

    match summary.verdict.clone() {
        Some(verdict) if matches!(verdict.as_str(), "pass" | "fail" | "partial" | "aborted") => {}
        Some(other) => {
            summary.verdict = Some("aborted".into());
            warnings.push(format!(
                "code review verdict `{other}` 非法，已规范化为 aborted"
            ));
        }
        None => {
            summary.verdict = Some("partial".into());
            warnings.push("code review 未返回 verdict，已规范化为 partial".into());
        }
    }

    if summary.applied_changes {
        summary.applied_changes = false;
        warnings.push("code reviewer 不允许直接改动，applied_changes 已重置为 false".into());
    }
    if summary.changes_summary.trim().is_empty() {
        summary.changes_summary = "none".into();
        warnings.push("code review 未返回 changes_summary，已规范化为 none".into());
    }

    warnings
}

/// 解析 `- { severity: ..., area: "...", note: "..." }` 这种 YAML-flow 风格的行。
/// 解析失败返回 None（单条失败不影响其它 finding）。
fn parse_finding_line(line: &str) -> Option<Finding> {
    let trimmed = line.trim_start_matches('-').trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return None;
    }
    // 用 serde_yaml 流模式不好维护依赖；这里手工提取 severity/area/note 三字段。
    let body = &trimmed[1..trimmed.len() - 1];
    let mut severity = None;
    let mut area = None;
    let mut note = None;
    for part in split_top_level_commas(body) {
        let (k, v) = part.split_once(':')?;
        let key = k.trim().trim_matches('"');
        let val = v
            .trim()
            .trim_matches(|c: char| c == '"' || c == '\'')
            .to_string();
        match key {
            "severity" => severity = Some(val),
            "area" => area = Some(val),
            "note" => note = Some(val),
            _ => {}
        }
    }
    Some(Finding {
        severity: severity.unwrap_or_else(|| "suggestion".into()),
        area: area.unwrap_or_default(),
        note: note?,
    })
}

/// 顶层 `,` 切分（忽略引号内的逗号）。极简实现，足够覆盖 reviewer 输出格式。
fn split_top_level_commas(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let bytes = s.as_bytes();
    let mut in_quote: Option<u8> = None;
    for (i, &b) in bytes.iter().enumerate() {
        match in_quote {
            Some(q) if q == b => in_quote = None,
            Some(_) => {}
            None => {
                if b == b'"' || b == b'\'' {
                    in_quote = Some(b);
                } else if b == b',' {
                    out.push(&s[start..i]);
                    start = i + 1;
                }
            }
        }
    }
    out.push(&s[start..]);
    out
}

fn find_last_review_block(text: &str) -> Option<&str> {
    let start_tag = "<review>";
    let end_tag = "</review>";
    let mut last_start = None;
    let mut search_from = 0;
    while let Some(s) = text[search_from..].find(start_tag) {
        last_start = Some(search_from + s);
        search_from = search_from + s + start_tag.len();
    }
    let start = last_start?;
    let body_start = start + start_tag.len();
    let end_rel = text[body_start..].find(end_tag)?;
    Some(&text[body_start..body_start + end_rel])
}

/// 从 BUILTIN_TOOL_CATALOG 中筛出 `allowed` 名单内的工具，输出 OpenAI function 定义。
///
/// 用于 reviewer 子 AgentLoopConfig.tool_definitions——硬白名单收紧，确保
/// `create_plan` / `bash` / `write` / `dispatch_agent` / `checkpoint` 永不出现。
pub fn resolve_internal_tools(allowed: &[&str]) -> Vec<Value> {
    use crate::core::tools::contract::catalog::BUILTIN_TOOL_CATALOG;
    BUILTIN_TOOL_CATALOG
        .iter()
        .filter(|entry| allowed.contains(&entry.name))
        .map(|entry| {
            serde_json::json!({
                "type": "function",
                "function": {
                    "name": entry.name,
                    "description": entry.description,
                    "parameters": (entry.parameters)(),
                }
            })
        })
        .collect()
}

/// 构造 reviewer 子 Agent 的 initial user prompt（"review brief"）。
/// 复用 reviewer.md §5.1 中"You receive a review brief in the initial user message"约束。
pub fn build_review_prompt(
    plan_id: &str,
    plan_text: &str,
    plan_path: &Path,
    workspace_root: Option<&Path>,
) -> String {
    let plan_path = crate::infra::platform::format_home_path(plan_path);
    let workspace_hint = workspace_root
        .map(|path| {
            format!(
                "         - Project/workspace root (start repo inspection here first): `{}`\n\
                 - Access note: this is the startup workspace snapshot; reads may still require runtime authorization (`workspace_roots` or a session grant) before they succeed.\n",
                crate::infra::platform::format_home_path(path)
            )
        })
        .unwrap_or_default();
    format!(
        "Review the following PlanFile (plan_id = `{plan_id}`).\n\
         Artifact paths (use these first; do not guess alternate locations):\n\
         - Exact plan file path: `{}`\n\
         {}\
         Scope:\n\
         - Inspect frontmatter `goal`, `todos[]`, and the markdown body.\n\
         - Flag risks (unclear acceptance criteria, missing dependencies, oversized todos).\n\
         - Read the exact plan file path above before broad search if you need to confirm current disk content.\n\
         - You may use `update_plan` to adjust `todos[]`, or `edit` the exact plan file above\n  \
           in-place; runtime enforces path guards and rejects raw frontmatter edits.\n\
         - If you inspect repository files, start from the project/workspace root above instead of wandering.\n\
         - End with the required <review> output block.\n\n\
         ----- BEGIN PLAN -----\n{plan_text}\n----- END PLAN -----\n"
        ,
        plan_path,
        workspace_hint,
    )
}

pub fn build_code_review_prompt(
    plan_id: &str,
    plan_text: &str,
    plan_path: &Path,
    workspace_root: Option<&Path>,
    diff_stat: &str,
    changed_files: &[String],
) -> String {
    let plan_path = crate::infra::platform::format_home_path(plan_path);
    let workspace_hint = workspace_root
        .map(|path| {
            format!(
                "         - Project/workspace root (start repo inspection here first): `{}`\n\
                 - Access note: reads and bash still follow runtime authorization / permission rules.\n",
                crate::infra::platform::format_home_path(path)
            )
        })
        .unwrap_or_default();
    let diff_section = if diff_stat.trim().is_empty() {
        "         Runtime git diff summary: unavailable (git diff injection failed or found no tracked changes).\n".to_string()
    } else {
        format!(
            "         Runtime git diff summary (`git diff --stat HEAD`):\n\
             ```text\n{diff_stat}\n```\n"
        )
    };
    let changed_files_section = if changed_files.is_empty() {
        "         Runtime changed files list: unavailable.\n".to_string()
    } else {
        let joined = changed_files
            .iter()
            .take(80)
            .map(|path| format!("         - `{path}`"))
            .collect::<Vec<_>>()
            .join("\n");
        let suffix = if changed_files.len() > 80 {
            format!(
                "\n         - ... {} more file(s) omitted",
                changed_files.len() - 80
            )
        } else {
            String::new()
        };
        format!("         Runtime changed files list:\n{joined}{suffix}\n")
    };
    format!(
        "Review the finished implementation for PlanFile `{plan_id}` before verifier runs.\n\
         Artifact paths (use these first; do not guess alternate locations):\n\
         - Exact plan file path: `{}`\n\
         {}\
         {}\
         {}\
         Scope:\n\
         - Treat all plan todos as already marked completed; check whether the actual implementation appears consistent with that claim.\n\
         - Review changed files first, then inspect neighboring implementation only as needed.\n\
         - You are STRICTLY read-only: do not edit plan files, todos, or repository files.\n\
         - If you conclude the implementation is ready for verifier, use `verdict: pass`.\n\
         - If you find a concrete issue the parent agent should fix first, use `verdict: fail`.\n\
         - If evidence is inconclusive, use `verdict: partial`.\n\
         - End with the required <review> output block.\n\n\
         ----- BEGIN PLAN -----\n{plan_text}\n----- END PLAN -----\n"
        ,
        plan_path,
        workspace_hint,
        diff_section,
        changed_files_section,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_review_block_happy_path() {
        let text = "noise\n<review>\nsummary: ok looks good\nchanges_summary: none\napplied_changes: false\n</review>\ntail";
        let r = parse_review_block(text).unwrap();
        assert!(!r.aborted);
        assert_eq!(r.kind, ReviewKind::Plan);
        assert_eq!(r.verdict, None);
        assert_eq!(r.summary, "ok looks good");
        assert_eq!(r.changes_summary, "none");
        assert!(!r.applied_changes);
    }

    #[test]
    fn parse_review_block_picks_last_block() {
        let text = "<review>\nsummary: old\nchanges_summary: none\napplied_changes: false\n</review>\n<review>\nsummary: new\nchanges_summary: applied:fix\napplied_changes: true\n</review>";
        let r = parse_review_block(text).unwrap();
        assert_eq!(r.summary, "new");
        assert_eq!(r.changes_summary, "applied:fix");
        assert!(r.applied_changes);
    }

    #[test]
    fn parse_review_block_missing_required_field_returns_none() {
        let text = "<review>\nsummary: only summary\n</review>";
        assert!(parse_review_block(text).is_none());
        let text = "<review>\nchanges_summary: none\napplied_changes: false\n</review>";
        assert!(parse_review_block(text).is_none());
    }

    #[test]
    fn parse_review_block_invalid_applied_changes_returns_none() {
        let text = "<review>\nsummary: x\nchanges_summary: none\napplied_changes: maybe\n</review>";
        assert!(parse_review_block(text).is_none());
    }

    #[test]
    fn parse_review_block_unclosed_returns_none() {
        let text = "<review>\nsummary: x\nchanges_summary: none\napplied_changes: false";
        assert!(parse_review_block(text).is_none());
    }

    #[test]
    fn parse_review_block_truncates_summary_to_600() {
        let body = "a".repeat(800);
        let text = format!(
            "<review>\nsummary: {body}\nchanges_summary: none\napplied_changes: false\n</review>"
        );
        let r = parse_review_block(&text).unwrap();
        assert_eq!(r.summary.len(), 600);
    }

    #[test]
    fn parse_review_block_with_findings() {
        let text = "<review>\nfindings:\n  - { severity: nit, area: \"style\", note: \"trailing space\" }\n  - { severity: concern, area: \"todos\", note: \"missing acceptance\" }\nsummary: see findings\nchanges_summary: none\napplied_changes: false\n</review>";
        let r = parse_review_block(text).unwrap();
        assert_eq!(r.findings.len(), 2);
        assert_eq!(r.findings[0].severity, "nit");
        assert_eq!(r.findings[1].area, "todos");
        assert_eq!(r.summary, "see findings");
    }

    #[test]
    fn aborted_summary_serializes_correctly() {
        let s = ReviewSummary::aborted_with("timeout");
        let j = s.to_json();
        assert_eq!(j["kind"], "plan");
        assert_eq!(j["aborted"], serde_json::Value::Bool(true));
        assert_eq!(j["summary"], "timeout");
        assert_eq!(j["reviewer_stop_reason"], "aborted");
    }

    #[test]
    fn reviewer_system_prompt_contains_constraints() {
        let p = REVIEWER_SYSTEM_PROMPT;
        assert!(p.contains("<review>"));
        assert!(p.contains("applied_changes"));
        // 必含禁用工具表
        assert!(p.contains("create_plan") && p.contains("bash"));
        assert!(!p.contains("{{#if"));
        assert!(p.contains("update_plan"));
    }

    #[test]
    fn code_review_system_prompt_contains_verdict_and_bash() {
        let p = CODE_REVIEW_SYSTEM_PROMPT;
        assert!(p.contains("verdict: pass|fail|partial|aborted"));
        assert!(p.contains("read, search_files, list_dir, bash"));
        assert!(p.contains("STRICTLY read-only") || p.contains("stay read-only"));
    }

    #[test]
    fn build_review_prompt_includes_plan_and_workspace_paths() {
        let prompt = build_review_prompt(
            "plan-1",
            "body",
            Path::new("/tmp/plan-1.plan.md"),
            Some(Path::new("/repo/root")),
        );
        assert!(prompt.contains("/tmp/plan-1.plan.md"));
        assert!(prompt.contains("/repo/root"));
        assert!(prompt.contains("workspace_roots"));
        assert!(prompt.contains("do not guess"));
    }

    #[test]
    fn build_code_review_prompt_includes_diff_context() {
        let prompt = build_code_review_prompt(
            "plan-1",
            "body",
            Path::new("/tmp/plan-1.plan.md"),
            Some(Path::new("/repo/root")),
            " src/lib.rs | 2 +-\n 1 file changed, 1 insertion(+), 1 deletion(-)",
            &["src/lib.rs".into(), "tests/lib.rs".into()],
        );
        assert!(prompt.contains("git diff --stat HEAD"));
        assert!(prompt.contains("src/lib.rs"));
        assert!(prompt.contains("tests/lib.rs"));
        assert!(prompt.contains("STRICTLY read-only"));
    }

    #[test]
    fn parse_review_block_with_verdict() {
        let text = "<review>\nfindings:\n  - { severity: concern, area: \"logic\", note: \"missing branch\" }\nverdict: fail\nsummary: found issue\nchanges_summary: none\napplied_changes: false\n</review>";
        let r = parse_review_block(text).unwrap();
        assert_eq!(r.verdict.as_deref(), Some("fail"));
        assert_eq!(r.findings.len(), 1);
    }

    #[test]
    fn normalize_for_code_review_fills_missing_verdict() {
        let mut summary = ReviewSummary {
            aborted: false,
            summary: "needs follow-up".into(),
            changes_summary: "none".into(),
            applied_changes: false,
            ..Default::default()
        };
        let warnings = normalize_for_code_review_result(&mut summary);
        assert_eq!(summary.kind, ReviewKind::Code);
        assert_eq!(summary.verdict.as_deref(), Some("partial"));
        assert!(warnings.iter().any(|w| w.contains("未返回 verdict")));
    }

    #[test]
    fn normalize_for_code_review_forces_aborted() {
        let mut summary = ReviewSummary::aborted_with_kind(ReviewKind::Code, "timeout");
        summary.verdict = None;
        let warnings = normalize_for_code_review_result(&mut summary);
        assert_eq!(summary.verdict.as_deref(), Some("aborted"));
        assert!(warnings.iter().any(|w| w.contains("verdict 已规范化为 aborted")));
    }

    #[test]
    fn reviewer_allowed_tools_change_by_kind() {
        assert_eq!(
            reviewer_allowed_tools_for(ReviewKind::Plan),
            PLAN_REVIEWER_ALLOWED_TOOLS
        );
        assert_eq!(
            reviewer_allowed_tools_for(ReviewKind::Code),
            CODE_REVIEWER_ALLOWED_TOOLS
        );
    }

    #[test]
    fn resolve_internal_tools_filters_plan_allowed_tools() {
        let tools = resolve_internal_tools(reviewer_allowed_tools_for(ReviewKind::Plan));
        let names: std::collections::BTreeSet<String> = tools
            .iter()
            .map(|v| v["function"]["name"].as_str().unwrap().to_string())
            .collect();
        assert!(names.contains("read"));
        assert!(names.contains("search_files"));
        assert!(names.contains("update_plan"));
        assert!(names.contains("edit"));
        assert!(!names.contains("create_plan"));
        assert!(!names.contains("bash"));
        assert!(!names.contains("write"));
        assert!(!names.contains("dispatch_agent"));
    }

    #[test]
    fn resolve_internal_tools_filters_code_allowed_tools() {
        let tools = resolve_internal_tools(reviewer_allowed_tools_for(ReviewKind::Code));
        let names: std::collections::BTreeSet<String> = tools
            .iter()
            .map(|v| v["function"]["name"].as_str().unwrap().to_string())
            .collect();
        assert!(names.contains("read"));
        assert!(names.contains("search_files"));
        assert!(names.contains("list_dir"));
        assert!(names.contains("bash"));
        assert!(!names.contains("todos"));
        assert!(!names.contains("update_plan"));
        assert!(!names.contains("edit"));
        assert!(!names.contains("create_plan"));
        assert!(!names.contains("write"));
    }
}
