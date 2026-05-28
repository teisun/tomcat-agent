//! `PlanState` 枚举与派生 helper（plan-runtime.md §4.1 R3/R11）。

/// PLAN 模式的五态状态机。
///
/// **不变式**：
/// - `Executing` 和 `Pending` 必须携带 `plan_id`，便于查询时不必再回 `PlanRuntime` 查 active。
/// - `Completed` 也带 `plan_id`（便于收口瞬间仍可定位历史 plan）。
/// - `Chat` / `Planning` 不带 `plan_id`；Planning 期的 active id 由 `PlanRuntime`
///   内部 `active_planning_plan_id` 便利字段提供。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanState {
    Chat,
    Planning,
    Executing { plan_id: String },
    Pending { plan_id: String },
    Completed { plan_id: String },
}

impl PlanState {
    /// 用于 system reminder / catalog 过滤 / user prefix 的 stable 字符串。
    pub fn as_str(&self) -> &'static str {
        match self {
            PlanState::Chat => "chat",
            PlanState::Planning => "planning",
            PlanState::Executing { .. } => "executing",
            PlanState::Pending { .. } => "pending",
            PlanState::Completed { .. } => "completed",
        }
    }

    /// 取 active plan id（仅 `Executing` / `Pending` / `Completed` 有值）。
    pub fn active_plan_id(&self) -> Option<&str> {
        match self {
            PlanState::Executing { plan_id }
            | PlanState::Pending { plan_id }
            | PlanState::Completed { plan_id } => Some(plan_id.as_str()),
            PlanState::Chat | PlanState::Planning => None,
        }
    }

    /// 是否处于「与 plan 文件交互」的态（PLAN / EXEC / Pending）；用于 catalog / panel 决策。
    pub fn is_plan_attached(&self) -> bool {
        !matches!(self, PlanState::Chat | PlanState::Completed { .. })
    }

    /// 仅 Planning。
    pub fn is_planning(&self) -> bool {
        matches!(self, PlanState::Planning)
    }

    /// 仅 Executing。
    pub fn is_executing(&self) -> bool {
        matches!(self, PlanState::Executing { .. })
    }
}
