//! Startup defaults for one ACP connection, echoed by the shared-service handshake.

use crate::core::plan_runtime::{
    InteractionPolicy, PlanReviewPolicy, PlanStartPolicy, QuestionPolicy,
};
use crate::core::session::AgentMode;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, clap::Args)]
#[serde(rename_all = "camelCase")]
pub struct AcpLaunchOptions {
    /// 初始会话模式：chat 为普通会话，plan 为规划。省略时保留载入会话的已保存模式；新会话为 chat
    #[arg(long, value_enum)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<AgentMode>,
    /// 保存计划后的独立评审：ask 先询问（默认），auto 自动评审，skip 跳过评审；不自动开始实施
    #[arg(long, value_enum)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_review: Option<PlanReviewPolicy>,
    /// 普通 ask_question 的回答策略：manual 人工作答（默认），recommended 自动采用推荐项；不代答评审、开工或工具权限确认
    #[arg(long, value_enum)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question_policy: Option<QuestionPolicy>,
    /// 在 Plan 模式中明确要求开工时：ask 额外询问（默认），auto 省略这次询问；保存或评审不会自动开工，工具权限不变
    #[arg(long, value_enum)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_start: Option<PlanStartPolicy>,
}

impl AcpLaunchOptions {
    pub fn provided(self) -> Option<Self> {
        (self != Self::default()).then_some(self)
    }

    pub fn interaction_policy(self) -> InteractionPolicy {
        InteractionPolicy {
            review: self.plan_review.unwrap_or_default(),
            questions: self.question_policy.unwrap_or_default(),
            start: self.plan_start.unwrap_or_default(),
        }
    }
}
