//! Plan 模式 LLM 工具实现（plan-runtime.md §5.3 / P2 PR-PLB）。
//!
//! 工具表（与 [`crate::core::tools::contract::catalog::BUILTIN_TOOL_CATALOG`] 中
//! 四个 `plan_only` 条目一一对应）：
//!
//! | 工具 | 可见模式 | 写入对象 | 说人话 |
//! |------|----------|----------|--------|
//! | [`create_plan`] | Planning | 整盘写入 `~/.tomcat/plans/<plan_id>.plan.md` | 第一稿计划 |
//! | [`update_plan`] | 任何模式 | 增量改 PlanFile.frontmatter.todos[] | 推进/编辑 |
//! | [`todos`] | 任何模式 | **仅** session TodoFile（与 PlanFile 无关；推进 plan 用 `update_plan`） | 我的待办 |
//! | `ask_question` | Planning + Chat/Pending/Completed（EXEC 隐藏） | 透传 UI | 结构化提问 |
//!
//! 每个函数都接 `&PlanRuntime + serde_json::Value (args) -> Result<serde_json::Value, ToolError>`，
//! 调用方（`tool_exec.rs` 在 P6 接入）负责把 OpenAI tool_call 参数透传。

pub mod ask_question;
pub mod create_plan;
pub(crate) mod shared_todo_ops;
pub mod todos;
pub mod update_plan;

#[cfg(test)]
mod tests;

pub use create_plan::CreatePlanArgs;
pub use todos::TodosArgs;
pub use update_plan::UpdatePlanArgs;

/// 所有 plan 工具的统一错误。可序列化到 ToolResult.content。
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("当前模式 {mode} 下 {tool} 不可见")]
    InvisibleInMode { tool: &'static str, mode: String },
    #[error("参数错误: {0}")]
    BadArgs(String),
    #[error("plan 文件错误: {0}")]
    PlanFile(#[from] crate::core::plan_runtime::file_store::PlanError),
    #[error("ops 错误: {0}")]
    Op(#[from] crate::core::plan_runtime::ops::OpError),
    #[error("跨 session 编辑被拒：{0}")]
    CrossSessionDenied(String),
    #[error("内部错误: {0}")]
    Internal(String),
}

impl ToolError {
    /// 序列化为 ToolResult.content；带 `is_error: true` 标识由 tool_exec 决定。
    pub fn to_tool_content(&self) -> String {
        serde_json::json!({
            "error": self.to_string(),
            "kind": match self {
                ToolError::InvisibleInMode { .. } => "invisible_in_mode",
                ToolError::BadArgs(_) => "bad_args",
                ToolError::PlanFile(_) => "plan_file",
                ToolError::Op(_) => "op",
                ToolError::CrossSessionDenied(_) => "cross_session_denied",
                ToolError::Internal(_) => "internal",
            }
        })
        .to_string()
    }
}
