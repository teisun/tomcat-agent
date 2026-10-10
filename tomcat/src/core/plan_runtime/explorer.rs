//! # Explorer：只读勘察子 Agent
//!
//! ```text
//!            主 Agent                        Explorer 子 Agent
//!      ─────────────────────         ────────────────────────────
//!  工具  全集（含写）                 只读：read / search_files / list_dir / bash
//!  上下文 只增长「结论」               勘察读到的原文随子 Agent 一起销毁
//!  派发   一次可并行派发多个            各自勘察不同模块
//!  返回   文件+行号+结论+待确认项       禁止回原文
//! ```
//!
//! 主 Agent 自己读 20 个文件，20 份原文全部留在主上下文里，很快撞上预算被压缩；
//! 换成派发 4 个 Explorer，主上下文只增长 4 段几百字的结论。这是本模块存在的唯一理由。

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::core::prompts::{load as load_prompt, render as render_prompt, PromptKey};

/// Explorer 可见工具：只读。写工具不在列表里，`tool_exec` 还会再拦一道。
pub const EXPLORER_ALLOWED_TOOLS: &[&str] = &["read", "search_files", "list_dir", "bash"];

/// 一次 `dispatch_agent` 里最多并行派发的 Explorer 数量。
/// 上限不是性能考量而是可读性：超过 6 份结论回到主上下文就不再是"省 token"了。
pub const MAX_EXPLORER_TASKS: usize = 6;

/// 单条勘察任务。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExplorerTask {
    /// 调用方给的短标识，用于把结论对回问题。
    pub id: String,
    pub prompt: String,
}

/// 单个 Explorer 的产出。`aborted` 时 `report` 存的是失败原因。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExplorerReport {
    pub id: String,
    pub aborted: bool,
    pub report: String,
    #[serde(default)]
    pub turns_used: u32,
    #[serde(default)]
    pub turns_limit: u32,
    #[serde(default)]
    pub stop_reason: String,
    #[serde(default)]
    pub child_session_id: String,
}

impl ExplorerReport {
    pub fn aborted_with(id: &str, reason: impl Into<String>) -> Self {
        Self {
            id: id.to_string(),
            aborted: true,
            report: reason.into(),
            stop_reason: "aborted".into(),
            ..Default::default()
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "aborted": self.aborted,
            "report": self.report,
            "turns_used": self.turns_used,
            "turns_limit": self.turns_limit,
            "stop_reason": self.stop_reason,
            "child_session_id": self.child_session_id,
        })
    }
}

pub fn explorer_system_prompt_text() -> &'static str {
    load_prompt(PromptKey::Explorer)
}

pub fn build_explorer_prompt(task: &ExplorerTask, workspace_root: Option<&Path>) -> String {
    let workspace_hint = workspace_root
        .map(|path| {
            format!(
                "Project/workspace root (start here): `{}`\n",
                crate::infra::platform::format_home_path(path)
            )
        })
        .unwrap_or_default();
    render_prompt(
        PromptKey::ExplorerBrief,
        &[
            ("task_id", &task.id),
            ("workspace_hint", &workspace_hint),
            ("task_prompt", &task.prompt),
        ],
    )
}

/// 报告是否符合输出契约。不符合不算失败——勘察内容仍然有用——但要在返回体里标出来，
/// 让主 Agent 知道这份结论可能夹带原文、需要自己核对。
pub fn contract_violations(report: &str) -> Vec<String> {
    let mut issues = Vec::new();
    if !report.contains("## Findings") {
        issues.push("Missing `## Findings` section".to_string());
    }
    if !report.contains("## Conclusion") {
        issues.push("Missing `## Conclusion` section".to_string());
    }
    if let Some(fence_lines) = fenced_block_max_lines(report) {
        if fence_lines > 3 {
            issues.push(format!("Report contains a {fence_lines}-line code block; Explorer should return only `path:line` references and findings"));
        }
    }
    issues
}

/// 报告里最长一个围栏代码块的行数。没有围栏时返回 None。
fn fenced_block_max_lines(report: &str) -> Option<usize> {
    let mut max = None;
    let mut current: Option<usize> = None;
    for line in report.lines() {
        if line.trim_start().starts_with("```") {
            match current.take() {
                Some(count) => max = Some(max.unwrap_or(0).max(count)),
                None => current = Some(0),
            }
        } else if let Some(count) = current.as_mut() {
            *count += 1;
        }
    }
    // 未闭合的围栏也计入，否则"只开不闭"就能绕过检查。
    if let Some(count) = current {
        max = Some(max.unwrap_or(0).max(count));
    }
    max
}

/// 把一组报告渲染成主 Agent 看到的文本。刻意保持扁平：主 Agent 要的是能直接读的结论。
pub fn render_reports(reports: &[ExplorerReport]) -> String {
    let mut out = String::new();
    for (idx, report) in reports.iter().enumerate() {
        out.push_str(&format!(
            "=== [{}/{}] {} ===\n",
            idx + 1,
            reports.len(),
            report.id
        ));
        if report.aborted {
            out.push_str(&format!("ABORTED: {}\n", report.report.trim()));
        } else {
            let issues = contract_violations(&report.report);
            if !issues.is_empty() {
                out.push_str(&format!(
                    "(contract warning: {}; verify the following findings yourself)\n",
                    issues.join("；")
                ));
            }
            out.push_str(report.report.trim());
            out.push('\n');
        }
        out.push('\n');
    }
    out.trim_end().to_string()
}

#[cfg(test)]
#[path = "tests/explorer_test.rs"]
mod tests;
