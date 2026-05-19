//! 生产 `ReviewerDispatcher` 实现（plan-runtime.md §P4 / D 部分）。
//!
//! 当前阶段（D-stub）：保留 trait 接线，但**仍**返回结构化的 aborted summary——
//! 明确告知调用方"生产 reviewer 已挂载、但 LLM 子 Agent 派发尚未启用"。完整接线
//! （`AgentRegistry::spawn_subagent_internal` + 受限工具集 + `<review>` 解析）需要
//! `core::agent_registry::AgentRegistry`、LLM `Arc<dyn LlmClient>`、`ChatContext`
//! 配置快照齐备后才能挂；这部分 chat_loop 顶层装配在后续 PR 收口（见 status.md "D 收尾"
//! 段落）。
//!
//! 行为契约：
//! - `aborted = true`、`summary` 包含原因；不阻 `create_plan` 成功。
//! - reviewer rounds 计数 + lock release 等附属逻辑由 `PlanRuntime::dispatch_reviewer`
//!   提供——本 dispatcher 不重复实现。

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use async_trait::async_trait;

use crate::api::chat::plan_runtime::{review::ReviewSummary, ReviewerDispatcher};

/// 生产环境 `ReviewerDispatcher` 占位实现。
///
/// **不直接持有** LLM / AgentRegistry——后续启用 LLM 子 Agent 派发时，把它们
/// 替换为完整字段即可，无需调整 chat_loop 装配点。
pub struct ProdReviewerDispatcher {
    /// 说明性字段：标识 dispatcher 来自哪一层装配（"chat_context" / "test_harness"），
    /// 进入 transcript / log 便于排查。
    pub origin: &'static str,
}

impl ProdReviewerDispatcher {
    pub fn new(origin: &'static str) -> Self {
        Self { origin }
    }
}

#[async_trait]
impl ReviewerDispatcher for ProdReviewerDispatcher {
    async fn dispatch(
        &self,
        plan_id: &str,
        _plan_text: &str,
        _allow_review_edit: bool,
        _abort_signal: Arc<AtomicBool>,
    ) -> ReviewSummary {
        ReviewSummary::aborted_with(format!(
            "[{}] 生产 reviewer 子 Agent 派发尚未启用（plan_id={plan_id}）；create_plan 已成功落盘，建议人工 review 后再 /plan build",
            self.origin
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn prod_reviewer_returns_aborted_with_origin() {
        let d = ProdReviewerDispatcher::new("test_origin");
        let r = d
            .dispatch(
                "demo",
                "noop",
                false,
                Arc::new(AtomicBool::new(false)),
            )
            .await;
        assert!(r.aborted);
        assert!(r.summary.contains("test_origin"));
        assert!(r.summary.contains("demo"));
        assert!(!r.applied_changes);
    }
}
