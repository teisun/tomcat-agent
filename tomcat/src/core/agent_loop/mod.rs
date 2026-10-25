//! # Agent Loop 核心结构化实现
//!
//! 三层循环：Conversation → Reasoning → Request Retry。
//! 与 `docs/architecture/agent-loop.md` 对齐；消息统一为 ChatMessage。
//!
//! ```text
//! run.rs：用户输入 / FollowUp / Completed、Interrupted、Failed 收尾
//!   └─ reasoning_loop.rs：保留已接受正文和工具轮次
//!        ├─ request_retry.rs：为下一份可接受回复分配有限额度
//!        │    guard → 组装 → 消费完整流 → 终态检查
//!        │      ↑            失败且有额度 ─┘
//!        └─ 提交正文 / 执行工具 → 下一逻辑请求
//! ```
//!
//! `max_attempts` 含首次，交互默认4；无人值守临时故障与空答为10。
//! 压缩、附件降级、网络错误与空答共享当前请求计数，不刷新额度。
//! `max_tool_rounds` 属于 Reasoning；重试不重置工具轮次或已接受正文。
//! 失败流的工具预览不执行；提交失败在请求重试之外报错。
//!
//! `auto_retry_end` 只结束本次请求的重试，不能让会话先于工具完成而结束。
//! 取消、Steering、权限和原生写入的 commit-aware 停止沿用既有边界。
//! 错误分类统一经过 `infra::error::classify_llm_failure`，不在这里建另一个识别器。
//! VS Code 的五分钟 Retry/Resume 等待由扩展宿主拥有，不属于本模块。
//!
//! ## 消息类型
//!
//! 消息统一使用 `ChatMessage`（OpenAI wire format），通过 `MessageKind` 字段区分语义：
//!
//! ```text
//!   ChatMessage (role + kind)
//!   ──────────────────────────────────────────────
//!   role=user,      kind=Normal             普通用户消息
//!   role=user,      kind=Steering           Steering 指令
//!   role=user,      kind=CompactionSummary  Compaction 摘要
//!   role=system                             System prompt
//!   role=assistant                          LLM 回复 (含 tool_calls)
//!   role=tool                               工具执行结果
//! ```

mod accessors;
pub mod config_backend;
mod current_tail_guard;
mod error_classifier;
mod reasoning_loop;
mod request_retry;
mod run;
mod steering_injection;
mod stream_handler;
mod tool_dispatcher;
mod tool_exec;
mod tool_summary_update;
mod turn_finalize;
mod turn_summary;
mod types;

#[cfg(test)]
mod tests;

/// Narrow cross-module test seam for verifying that context compaction invalidates
/// `read` stamps before the next real tool invocation. It is test-only so production
/// callers still go through the agent loop.
#[cfg(test)]
pub(crate) async fn execute_tool_for_cross_module_test(
    primitive: &std::sync::Arc<dyn crate::core::tools::primitive::PrimitiveExecutor>,
    read_file_state: &std::sync::Arc<crate::core::tools::pipeline::read_state::ReadFileState>,
    tool_call: &types::ToolCallInfo,
) -> (String, bool, Vec<crate::core::llm::ChatMessageContentPart>) {
    tool_exec::execute_tool(primitive, &None, &None, Some(read_file_state), tool_call).await
}

pub use config_backend::{
    ConfigBackend, PackageInstallBackend, PackageInstallRequest, PackageInstallStatus,
    PackageInstallToolResource, PackageInstallToolResult, SharedConfigBackend,
    SharedPackageInstallBackend,
};
pub use current_tail_guard::{build_collapse_summary_artifacts_for_test, CollapseSummaryArtifacts};
pub use steering_injection::{SteeringInbox, SteeringInput};
pub use types::{
    AgentLoop, AgentLoopConfig, AgentRunOutcome, AgentRunResult, BackgroundCompletionRoutes,
    CompletionRoute, EphemeralTailProvider, LoopError, SubagentType, ToolCallInfo,
};
