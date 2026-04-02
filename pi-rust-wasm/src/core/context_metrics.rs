//! 上下文管理可观测性指标。

use serde::Serialize;

/// 上下文管理运行时指标，每轮 cascade 后计算并通过 EventBus 推送。
#[derive(Debug, Clone, Default, Serialize)]
pub struct ContextMetrics {
    pub input_tokens_used: usize,
    pub context_utilization_ratio: f64,
    pub compaction_count: u32,
    pub compaction_tokens_freed: usize,
    pub total_tool_result_bytes_persisted: usize,
}
