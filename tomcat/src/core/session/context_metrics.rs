//! 上下文管理可观测性：瞬时指标类型定义在 `ContextState::live`（`ContextLiveMetrics`）。
//! 本会话累计见 `ContextState::session_obs`（`SessionContextObservation`）。

pub use super::manager::ContextLiveMetrics;

/// 与历史命名对齐的别名（即 `ContextLiveMetrics`）。
pub type ContextMetrics = ContextLiveMetrics;
