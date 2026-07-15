//! Prompt 模板注册表：所有系统 / 计划 / reviewer / verifier 文本的唯一权威源。
//!
//! 模板文本存放在同目录 `templates/` 下，通过 `include_str!` 编译期嵌入二进制。
//! 运行期不读盘、不支持 env override，避免系统 prompt 被外部篡改。

/// 所有内置 prompt 模板的稳定键。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKey {
    SystemCoreIdentity,
    SystemToolInstructions,
    SystemParallelTools,
    SystemPagedReading,
    SystemBackgroundShellMonitor,
    SystemVerification,
    SystemAvailableSkills,
    SystemWorkspaceContext,
    SystemWorkspaceState,
    PlannerReminder,
    ExecutorReminderFmt,
    ReviewerPlan,
    ReviewerPlanBrief,
    ReviewerCode,
    ReviewerCodeBrief,
    Verifier,
    VerifierBrief,
}

/// 读取内置 prompt 模板原文。
pub fn load(key: PromptKey) -> &'static str {
    match key {
        PromptKey::SystemCoreIdentity => include_str!("templates/system/core_identity.txt"),
        PromptKey::SystemToolInstructions => include_str!("templates/system/tool_instructions.txt"),
        PromptKey::SystemParallelTools => include_str!("templates/system/parallel_tools.txt"),
        PromptKey::SystemPagedReading => include_str!("templates/system/paged_reading.txt"),
        PromptKey::SystemBackgroundShellMonitor => {
            include_str!("templates/system/background_shell_monitor.txt")
        }
        PromptKey::SystemVerification => include_str!("templates/system/verification.txt"),
        PromptKey::SystemAvailableSkills => include_str!("templates/system/available_skills.txt"),
        PromptKey::SystemWorkspaceContext => include_str!("templates/system/workspace_context.txt"),
        PromptKey::SystemWorkspaceState => include_str!("templates/system/workspace_state.txt"),
        PromptKey::PlannerReminder => include_str!("templates/plan/planner.txt"),
        PromptKey::ExecutorReminderFmt => include_str!("templates/plan/executor.txt"),
        PromptKey::ReviewerPlan => include_str!("templates/reviewer/plan_review.txt"),
        PromptKey::ReviewerPlanBrief => include_str!("templates/reviewer/review_brief.txt"),
        PromptKey::ReviewerCode => include_str!("templates/reviewer/code_review.txt"),
        PromptKey::ReviewerCodeBrief => include_str!("templates/reviewer/code_review_brief.txt"),
        PromptKey::Verifier => include_str!("templates/verifier/verify.txt"),
        PromptKey::VerifierBrief => include_str!("templates/verifier/verify_brief.txt"),
    }
}

/// 用简单的 `{name}` 占位符替换渲染模板。
pub fn render(key: PromptKey, vars: &[(&str, &str)]) -> String {
    let mut rendered = load(key).to_string();
    for (name, value) in vars {
        rendered = rendered.replace(&format!("{{{name}}}"), value);
    }
    rendered
}

#[cfg(test)]
mod tests;
