//! # PlanRuntime — per-session PLAN 模式编排器（T2-P1-002/003/004）
//!
//! `PlanRuntime` 与 `TodoRuntime` 是 PLAN 模式的两条 per-session 状态机：前者持有当前
//! `PlanMode`、active plan id、reviewer 派发逻辑；后者持有 CHAT 模式下的纯 todo 列表。
//! 它们都挂在 `ChatContext` 上，与 chat session 同生命周期（**不**每轮重建，否则 `mode`
//! 会被重置回 Chat，丢失 PLAN/EXEC 的持续语义）。
//!
//! ## 状态机（plan-runtime.md §4.1 R3 / R11）
//!
//! ```text
//!                    /plan "<obj>"
//!         Chat ─────────────────────► Planning
//!          ▲                              │
//!          │                  /plan exit  │
//!          │  /plan exit                  ▼
//!          ├────────────── Pending { plan_id }
//!          │                  ▲       │
//!          │  cancel_token    │       │ /plan build <plan_id>
//!          │  / Ctrl+C        │       ▼
//!          │              Executing { plan_id }
//!          │                      │
//!          │ all todos completed  │
//!          ▼                      ▼
//!         Chat ◄────────── Completed { plan_id }
//! ```
//!
//! ## 模块组织
//!
//! - [`mode`]：`PlanMode` 枚举 + 派生 helper（`as_str` / `active_plan_id` 等）
//! - [`catalog`]：`visible_tools_for_mode(PlanMode, base) -> Vec<Value>`，
//!   PLAN/EXEC 时合入 plan_only 工具；CHAT 时排除
//! - [`prompts`]：PLANNER / EXECUTOR `<system_reminder>` 常量
//! - [`session_prefix`]：`[mode: PLAN]` / `[mode: EXEC plan_id=…]` user-message 装饰
//! - [`safety`]：`assert_plan_id_safe`（防穿越 `../` / `/` / 控制字符）
//!
//! P2 起补 `file_store` / `ops`（todos op）；P4 起补 `dispatch_reviewer`；P5 起补
//! `tools::ask_question`；P6 起补 `/plan build` 五件事；P7 起补 `panel` / `checkpoint` /
//! `cancel`。

pub mod catalog;
pub mod mode;
pub mod prompts;
pub mod safety;
pub mod session_prefix;

#[cfg(test)]
mod tests;

use std::sync::Arc;

use parking_lot::RwLock;
use tokio_util::sync::CancellationToken;

pub use mode::PlanMode;

/// PLAN 模式 per-session 编排器骨架（P1）。
///
/// 当前 PR-PLA 范围只支持：
/// - `/plan "<obj>"` → `enter_planning`
/// - `/plan exit` → `exit_to_chat`
/// - `recover()`（启动时扫描 `~/.tomcat/plans/`）— 占位实现，P2 起接入 file_store
///
/// 后续 PR：`build_plan` / `cancel_to_pending` / `dispatch_reviewer` / `attach_cancel_hook` /
/// `decorate_messages` / `visible_tools_for_mode` 在 P2-P7 逐步补齐；本结构体公共字段
/// 在 P1 已定型，避免后续多次扩字段引发的连锁修改。
pub struct PlanRuntime {
    /// 当前模式。每轮 `chat_loop` 装配 `tool_definitions` / system reminder / user prefix
    /// 都基于此值；跨 turn 持久（**禁止**每轮重建 `PlanRuntime`）。
    mode: RwLock<PlanMode>,
    /// 本 PlanRuntime 绑定的 session_key（来自 `SessionManager::current_session_key`）。
    /// 用于 `recover()` 区分 executing 是当前 session 在跑（保留）还是异 session 残留
    /// （降级 pending + warning），实现 D6 防御。
    #[allow(dead_code)] // P2/P7 起 recover / build 时使用
    session_key: String,
    /// 本回合 `CancellationToken` 的弱引用。chat_loop 每轮 readline 后重建 token，
    /// 必须立即 `attach_cancel_hook(&new_token)` 重挂，否则上一轮的 hook 监听
    /// 失效 → cancel→pending 不工作（D2 防御）。
    #[allow(dead_code)] // P7 接入
    cancel_token: parking_lot::Mutex<Option<CancellationToken>>,
}

impl PlanRuntime {
    /// 构造一个绑定到 session_key 的 PlanRuntime。
    ///
    /// session_key 在 `ChatContext::from_config` 装配阶段已知（chat session 同生命周期）。
    /// 当前 P1 实现：`mode = Chat`，等待 `enter_planning` 或 `recover` 改写。
    pub fn new(session_key: impl Into<String>) -> Arc<Self> {
        Arc::new(Self {
            mode: RwLock::new(PlanMode::Chat),
            session_key: session_key.into(),
            cancel_token: parking_lot::Mutex::new(None),
        })
    }

    /// 读当前 mode（轻量 RwLock 读锁；不分配）。
    pub fn mode(&self) -> PlanMode {
        self.mode.read().clone()
    }

    /// `/plan "<objective>"` → 进入 Planning 模式。
    ///
    /// 在 P2 接入 `file_store` 前，本方法只做内存状态切换：`Chat | Completed { .. } → Planning`。
    /// 已在 `Planning` / `Executing` / `Pending` 时返回 `Err`（用户须先 `/plan exit` 或 `/plan build`）。
    pub fn enter_planning(&self, objective: &str) -> Result<(), PlanRuntimeError> {
        if objective.trim().is_empty() {
            return Err(PlanRuntimeError::EmptyObjective);
        }
        let mut mode = self.mode.write();
        match &*mode {
            PlanMode::Chat | PlanMode::Completed { .. } => {
                *mode = PlanMode::Planning;
                Ok(())
            }
            PlanMode::Planning => Err(PlanRuntimeError::AlreadyInMode("planning".into())),
            PlanMode::Executing { plan_id } => Err(PlanRuntimeError::AlreadyInMode(format!(
                "executing(plan_id={plan_id})"
            ))),
            PlanMode::Pending { plan_id } => Err(PlanRuntimeError::AlreadyInMode(format!(
                "pending(plan_id={plan_id})"
            ))),
        }
    }

    /// `/plan exit` → 退回 Chat（或 Pending 时直接退）。
    ///
    /// **不**删 plan 文件（PR-PLE：退出规划不删盘）。
    pub fn exit_to_chat(&self) -> Result<(), PlanRuntimeError> {
        let mut mode = self.mode.write();
        match &*mode {
            PlanMode::Chat => Err(PlanRuntimeError::AlreadyInMode("chat".into())),
            // Planning / Executing / Pending / Completed 都允许显式退出
            _ => {
                *mode = PlanMode::Chat;
                Ok(())
            }
        }
    }

    /// 启动 recover：扫描 `~/.tomcat/plans/` 还原 active plan（P2 起接入 file_store）。
    ///
    /// P1 占位：什么都不做，仅断言 `mode == Chat`（启动时初始态）。
    pub fn recover(&self) -> Result<(), PlanRuntimeError> {
        debug_assert!(matches!(*self.mode.read(), PlanMode::Chat));
        Ok(())
    }
}

/// `PlanRuntime` 操作错误。
#[derive(Debug, thiserror::Error)]
pub enum PlanRuntimeError {
    #[error("/plan 需要非空 objective")]
    EmptyObjective,
    #[error("当前已经在 {0} 模式，无法重复进入")]
    AlreadyInMode(String),
    #[error("plan_id 非法或不安全：{0}")]
    UnsafePlanId(String),
    /// PlanFile 文件 IO / serde 错误（P2 起细化）。
    #[error("plan io: {0}")]
    Io(String),
}
