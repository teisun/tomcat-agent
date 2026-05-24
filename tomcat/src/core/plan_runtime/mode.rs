//! `PlanMode` 枚举与派生 helper（plan-runtime.md §4.1 R3/R11）。

/// PLAN 模式的五态状态机。
///
/// **不变式**：
/// - `Executing` 和 `Pending` 必须携带 `plan_id`，便于查询时不必再回 `PlanRuntime` 查 active。
/// - `Completed` 也带 `plan_id`（便于 mode→completed 派生后仍可 `/plan show` 查看历史）。
/// - `Chat` / `Planning` 不带 `plan_id`（`Planning` 期 create_plan 后会立即落盘并迁移到 `Planning` 仍保留——
///   注：本期 `Planning` 不携带 plan_id；plan 已落盘但用户尚未 `/plan build`，active id 由 PlanRuntime 内部
///   `active_plan_id` 字段提供——见 PR-PLB）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanMode {
    Chat,
    Planning,
    Executing { plan_id: String },
    Pending { plan_id: String },
    Completed { plan_id: String },
}

impl PlanMode {
    /// 用于 system reminder / catalog 过滤 / user prefix 的 stable 字符串。
    pub fn as_str(&self) -> &'static str {
        match self {
            PlanMode::Chat => "chat",
            PlanMode::Planning => "planning",
            PlanMode::Executing { .. } => "executing",
            PlanMode::Pending { .. } => "pending",
            PlanMode::Completed { .. } => "completed",
        }
    }

    /// 取 active plan id（仅 `Executing` / `Pending` / `Completed` 有值）。
    pub fn active_plan_id(&self) -> Option<&str> {
        match self {
            PlanMode::Executing { plan_id }
            | PlanMode::Pending { plan_id }
            | PlanMode::Completed { plan_id } => Some(plan_id.as_str()),
            PlanMode::Chat | PlanMode::Planning => None,
        }
    }

    /// 是否处于「与 plan 文件交互」的态（PLAN / EXEC / Pending）；用于 catalog / panel 决策。
    pub fn is_plan_attached(&self) -> bool {
        !matches!(self, PlanMode::Chat | PlanMode::Completed { .. })
    }

    /// 仅 Planning。
    pub fn is_planning(&self) -> bool {
        matches!(self, PlanMode::Planning)
    }

    /// 仅 Executing。
    pub fn is_executing(&self) -> bool {
        matches!(self, PlanMode::Executing { .. })
    }
}
