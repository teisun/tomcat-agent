//! Startup defaults for one ACP connection, echoed by the shared-service handshake.

use crate::core::plan_runtime::{
    InteractionPolicy, PlanReviewPolicy, PlanStartPolicy, QuestionPolicy,
};
use crate::core::session::AgentMode;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, clap::Args)]
#[serde(rename_all = "camelCase")]
pub struct AcpLaunchOptions {
    /// 初始会话模式；未设置时保留恢复模式（新会话为 chat）
    #[arg(long, value_enum)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<AgentMode>,
    /// 保存计划后的评审方式（默认 ask）
    #[arg(long, value_enum)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_review: Option<PlanReviewPolicy>,
    /// 普通问题人工作答或自动采用推荐项（默认 manual）
    #[arg(long, value_enum)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question_policy: Option<QuestionPolicy>,
    /// Plan 模式中明确开工时的确认方式（默认 ask）
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
