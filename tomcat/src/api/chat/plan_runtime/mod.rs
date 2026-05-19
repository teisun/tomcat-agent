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

pub mod ask_question_panel;
pub mod catalog;
pub mod file_store;
pub mod mode;
pub mod ops;
pub mod prompts;
pub mod review;
pub mod safety;
pub mod session_prefix;
pub mod tools;

#[cfg(test)]
mod tests;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

pub use ask_question_panel::AskQuestionPanel;
pub use mode::PlanMode;
pub use review::ReviewSummary;

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
    session_key: String,
    /// 本回合 `CancellationToken` 的弱引用。chat_loop 每轮 readline 后重建 token，
    /// 必须立即 `attach_cancel_hook(&new_token)` 重挂，否则上一轮的 hook 监听
    /// 失效 → cancel→pending 不工作（D2 防御）。
    #[allow(dead_code)] // P7 接入
    cancel_token: Mutex<Option<CancellationToken>>,
    /// CHAT 模式下 `todos` 工具的 session-local scratchpad，**不**落盘 plan 文件
    /// （落盘文件路径由 P7 PR-PLD 引入 `~/.tomcat/agents/.../todos/*.todo.md`，
    /// 当前 P2 内存即可）。EXEC/Planning/Pending 模式下 `todos` 操作走 PlanFile。
    session_todos: Mutex<Vec<file_store::TodoItem>>,
    /// Planning 状态的 active plan_id。P1 的 `PlanMode::Planning` 没有携带 plan_id 字段；
    /// 这里用辅助字段保留 `create_plan` 写盘后的 plan_id，供后续 `update_plan` /
    /// `/plan build` 默认路由使用。EXEC/Pending 状态请直接读 `mode().active_plan_id()`。
    active_planning_plan_id: Mutex<Option<String>>,
    /// `[plan] lock_timeout_ms`：write_plan / dispatch_reviewer 共享。默认 2000。
    lock_timeout_ms: u64,
    /// 可选 reviewer 派发器。P4 时由 `ChatContext::from_config` 注入真实实现；
    /// 测试可注入 mock；未注入时 `create_plan` 返回 `aborted=true` 占位摘要。
    reviewer: Mutex<Option<Arc<dyn ReviewerDispatcher>>>,
    /// 计数 reviewer 派发轮次（用于 `[reviewer] max_review_rounds` 软上限 warning）。
    reviewer_rounds: parking_lot::Mutex<std::collections::HashMap<String, u32>>,
    /// 可选 `ask_question` UI 后端（P5）。生产由 `ChatContext::from_config`
    /// 注入 `CliAskQuestionPanel`（CLI MVP）/ T2-P0-008 完成后改注入 `IdeAskQuestionPanel`。
    /// 测试可注入 `MockAskQuestionPanel`。未注入时 `ask_question` 工具返回
    /// `cancelled: true` 兜底（避免 panic / 卡死）。
    ask_question_panel: Mutex<Option<Arc<dyn AskQuestionPanel>>>,
    /// 进入 EXEC 后**首轮**user message 装配阶段的注入标记（P6 PR-PLC §5.5 / §6 first_turn_user_meta）：
    /// `/plan build` 成功后置 true；`consume_first_exec_turn_user_meta` 第一次调
    /// 时返回 plan body 并置 false；后续轮次返回 None（避免重复注入超长 plan 全文）。
    first_exec_turn_pending: AtomicBool,
    /// 触发首轮注入的 plan body 缓存（与 `first_exec_turn_pending` 配对，
    /// 在 build_plan 写盘成功后预读，避免首轮 chat_loop 再做一次 disk IO）。
    first_exec_turn_body: Mutex<Option<String>>,
}

impl PlanRuntime {
    /// 构造一个绑定到 session_key 的 PlanRuntime。
    ///
    /// session_key 在 `ChatContext::from_config` 装配阶段已知（chat session 同生命周期）。
    /// 当前 P1 实现：`mode = Chat`，等待 `enter_planning` 或 `recover` 改写。
    pub fn new(session_key: impl Into<String>) -> Arc<Self> {
        Self::with_lock_timeout(session_key, file_store::DEFAULT_LOCK_TIMEOUT_MS)
    }

    /// 显式给 `lock_timeout_ms`（测试用；生产从 `[plan] lock_timeout_ms` 读取）。
    pub fn with_lock_timeout(
        session_key: impl Into<String>,
        lock_timeout_ms: u64,
    ) -> Arc<Self> {
        Arc::new(Self {
            mode: RwLock::new(PlanMode::Chat),
            session_key: session_key.into(),
            cancel_token: Mutex::new(None),
            session_todos: Mutex::new(Vec::new()),
            active_planning_plan_id: Mutex::new(None),
            lock_timeout_ms,
            reviewer: Mutex::new(None),
            reviewer_rounds: parking_lot::Mutex::new(std::collections::HashMap::new()),
            ask_question_panel: Mutex::new(None),
            first_exec_turn_pending: AtomicBool::new(false),
            first_exec_turn_body: Mutex::new(None),
        })
    }

    /// 本 runtime 绑定的 session_key（只读）。
    pub fn session_key(&self) -> &str {
        &self.session_key
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

    // ─── P2 PR-PLB 内部 API（供 tools/* 模块调用） ──────────────────────────

    /// 当前 `[plan] lock_timeout_ms`。
    pub fn lock_timeout_ms(&self) -> u64 {
        self.lock_timeout_ms
    }

    /// session_todos 的快照（克隆，供 `todos` CHAT 路径使用）。
    pub fn snapshot_session_todos(&self) -> Vec<file_store::TodoItem> {
        self.session_todos.lock().clone()
    }

    /// 整体替换 session_todos（不暴露细粒度 API，避免 ops 引擎语义被绕过）。
    pub fn replace_session_todos(&self, todos: Vec<file_store::TodoItem>) {
        *self.session_todos.lock() = todos;
    }

    /// Planning 模式下记 active_plan_id（写入 mode 内的 plan_id 影子字段）。
    ///
    /// 用 Planning(plan_id?) 容易破坏现有 `PlanMode` 形态（P1 已签合约：Planning 不带 plan_id）。
    /// 改为另存 `active_planning_plan_id`，仅在 Planning 状态有意义。
    pub fn set_active_planning_plan_id(&self, plan_id: String) {
        *self.active_planning_plan_id.lock() = Some(plan_id);
    }

    /// 读 Planning 模式下的 active_plan_id。EXEC/Pending 应直接看 `mode().active_plan_id()`。
    pub fn active_planning_plan_id(&self) -> Option<String> {
        self.active_planning_plan_id.lock().clone()
    }

    /// 内存切到 `Completed { plan_id }`；由 update_plan / todos 在所有 todo 完成时调用。
    pub fn set_mode_completed(&self, plan_id: String) {
        *self.mode.write() = PlanMode::Completed { plan_id };
        // active planning 已经收口，清空辅助字段
        *self.active_planning_plan_id.lock() = None;
    }

    /// 测试辅助：直接把内存 mode 切到 `Executing { plan_id }`，
    /// **不**做任何 frontmatter / disk 校验。仅供集成单测短路 `/plan build` 路径。
    /// 真实路径请等待 P6 PR-PLC 的 `build_plan` API。
    #[doc(hidden)]
    pub fn set_executing_for_test(&self, plan_id: String) {
        *self.mode.write() = PlanMode::Executing { plan_id };
    }

    // ─── P4 reviewer 派发 API（plan-runtime.md §P4） ──────────────────────

    /// 注入 reviewer 派发器（生产由 `ChatContext::from_config` 装配 reviewer 子 Agent 派发；
    /// 测试可注入 [`review::MockReviewerDispatcher`] / 自定义实现）。
    pub fn attach_reviewer(&self, dispatcher: Arc<dyn ReviewerDispatcher>) {
        *self.reviewer.lock() = Some(dispatcher);
    }

    /// 同步派发 reviewer（plan-runtime.md §P4 RV14）。语义：
    ///
    /// 1. **必须**在 `write_plan` 释放 advisory lock **之后**调用（防 D1 死锁）。
    /// 2. 读取 plan 文件 → 调 dispatcher → 解析 `<review>` block → 返回 `ReviewSummary`。
    /// 3. 失败 / parse 错 / max_turns / parent abort → `aborted=true`；
    ///    调用方（`create_plan` / `/plan build` 等）**不**因此失败。
    /// 4. 若 dispatcher 未注入（测试 / 简化场景）→ 返回 `placeholder_pending`。
    pub async fn dispatch_reviewer(
        &self,
        plan_id: &str,
        allow_review_edit: bool,
    ) -> review::ReviewSummary {
        let Some(dispatcher) = self.reviewer.lock().clone() else {
            return review::ReviewSummary::placeholder_pending();
        };
        // 软上限：默认 1 轮；超出 → warning（这里以摘要 prefix 表示，
        // chat_loop 在装配 transcript 时会写 `plan.review.warning`）
        let rounds = {
            let mut map = self.reviewer_rounds.lock();
            let v = map.entry(plan_id.to_string()).or_insert(0);
            *v += 1;
            *v
        };

        // 读 plan 文件作为 reviewer 上下文（不上 advisory lock；
        // 锁的 acquire 已由 write_plan 释放，reviewer 走只读）。
        let path = match file_store::plan_path_for_id(plan_id) {
            Ok(p) => p,
            Err(e) => return review::ReviewSummary::aborted_with(format!("plan_id 非法: {e}")),
        };
        let plan_text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                return review::ReviewSummary::aborted_with(format!("read plan 失败: {e}"))
            }
        };

        let cascade = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut summary = dispatcher
            .dispatch(plan_id, &plan_text, allow_review_edit, cascade)
            .await;
        if rounds > 1 {
            summary.summary = format!("[round {rounds}] {}", summary.summary);
        }
        summary
    }

    /// 用于单测 / 集成测：清除指定 plan_id 的 reviewer round 计数。
    pub fn reset_reviewer_rounds(&self, plan_id: &str) {
        self.reviewer_rounds.lock().remove(plan_id);
    }

    /// 用于单测：当前 plan_id 的 reviewer 派发轮次。
    pub fn reviewer_rounds(&self, plan_id: &str) -> u32 {
        self.reviewer_rounds
            .lock()
            .get(plan_id)
            .copied()
            .unwrap_or(0)
    }

    // ─── P5 ask_question 面板注入 ──────────────────────────────────────

    /// 注入 `ask_question` UI 面板。生产由 `ChatContext::from_config` 装配 `CliAskQuestionPanel`；
    /// 测试可注入 `MockAskQuestionPanel`。
    pub fn attach_ask_question_panel(&self, panel: Arc<dyn AskQuestionPanel>) {
        *self.ask_question_panel.lock() = Some(panel);
    }

    /// 取出当前注入的 panel（克隆 Arc）。`tool_exec.rs` 调 `ask_question::execute`
    /// 前从此处取；未注入时返回 None，工具层会回写 `cancelled: true`。
    pub fn ask_question_panel(&self) -> Option<Arc<dyn AskQuestionPanel>> {
        self.ask_question_panel.lock().clone()
    }

    // ─── P6 /plan build 五件事（plan-runtime.md §5.1 + §4.1 R7） ──────────

    /// `/plan build <plan_id>` 入口；执行 plan-runtime §5.1 的 5 件事 + 原子回滚。
    ///
    /// **闸门**（任一不通过 → `BuildBlocked`）：
    /// - 当前内存 mode ∈ `{Chat, Planning}`（已 Executing/Pending/Completed 拒）
    /// - 当前 session 无 active todos（`session_todos` 全 completed/cancelled 或空）
    /// - 当前 session 无 active plan_id 占用（mode 不是 Executing/Pending 即可，
    ///   Planning 期 active_planning_plan_id 与目标 plan_id 相同时不算冲突）
    /// - 目标 PlanFile 必须存在（不存在 → `BuildPlanNotFound`，附友好提示）
    /// - PlanFile.frontmatter.mode ∈ `{planning, pending}`（executing/completed 拒）
    ///
    /// **5 件事**：
    /// 1. 改 frontmatter.session_key = `self.session_key`；session_id = `session_id`
    ///    （pending 续跑时若 `prev_session_key != self.session_key` → push warning，仍执行）
    /// 2. 改 frontmatter.mode = `executing`
    /// 3. `write_plan`（atomic + advisory lock）；**失败时内存不动**，返回 PlanFile error
    /// 4. 写盘成功后切内存 `mode = Executing { plan_id }`、清 `active_planning_plan_id`
    /// 5. 设置 `first_exec_turn_pending = true`，缓存 plan body 供首轮 user_meta 注入；
    ///    （reminder/catalog/prefix swap 均由 mode 派生，chat_loop 装配阶段自动 pick up）
    ///
    /// **原子性**：盘 write 失败 → 内存不变；盘 write 成功后才动内存——
    /// 配合 advisory lock 保证 PlanFile 不会出现"executing 但内存仍 Chat"的半态。
    /// （注：写盘 OK 但内存切换前 panic 这条很窄的窗口由 D7 recover 兜底）。
    pub fn build_plan(
        &self,
        plan_id: &str,
        session_id: Option<String>,
    ) -> Result<BuildPlanOutcome, PlanRuntimeError> {
        safety::assert_plan_id_safe(plan_id)?;
        // ─── 闸门 1：内存 mode ─────────────────────────────────────────
        {
            let mode = self.mode.read();
            match &*mode {
                PlanMode::Chat | PlanMode::Planning => { /* 允许 */ }
                PlanMode::Executing { plan_id: cur } => {
                    return Err(PlanRuntimeError::BuildBlocked(format!(
                        "当前 session 已在 EXEC（plan_id={cur}）；先等结束或 cancel→pending"
                    )));
                }
                PlanMode::Pending { plan_id: cur } => {
                    return Err(PlanRuntimeError::BuildBlocked(format!(
                        "当前 session 有 pending plan {cur}；先续跑或 /plan exit"
                    )));
                }
                PlanMode::Completed { plan_id: cur } => {
                    return Err(PlanRuntimeError::BuildBlocked(format!(
                        "当前 session 已 completed plan {cur}；新计划请先 /plan exit"
                    )));
                }
            }
        }
        // ─── 闸门 2：active todos ──────────────────────────────────────
        {
            let session_todos = self.session_todos.lock();
            let has_active = session_todos.iter().any(|t| {
                matches!(
                    t.status,
                    file_store::TodoStatus::Pending | file_store::TodoStatus::InProgress
                )
            });
            if has_active {
                return Err(PlanRuntimeError::BuildBlocked(
                    "当前 session 有未完成 todos；先收口（todos remove / set completed）后再 build".into(),
                ));
            }
        }
        // ─── 闸门 3：active_planning_plan_id 冲突 ──────────────────────
        {
            if let Some(active) = self.active_planning_plan_id.lock().clone() {
                if active != plan_id {
                    return Err(PlanRuntimeError::BuildBlocked(format!(
                        "Planning 期已有 active plan {active}；先 /plan exit 或 build 同 plan_id"
                    )));
                }
            }
        }
        // ─── 读 PlanFile + 闸门 4/5：存在 + 合法 mode ──────────────────
        let path = file_store::plan_path_for_id(plan_id)
            .map_err(|e| PlanRuntimeError::Io(e.to_string()))?;
        let mut plan = match file_store::read_plan(&path) {
            Ok(p) => p,
            Err(file_store::PlanError::NotFound { .. }) => {
                return Err(PlanRuntimeError::BuildPlanNotFound {
                    plan_id: plan_id.to_string(),
                    hint: format!(
                        "未找到 ~/.tomcat/plans/{plan_id}.plan.md；先通过 PLAN 模式 create_plan 生成"
                    ),
                });
            }
            Err(e) => return Err(PlanRuntimeError::Io(e.to_string())),
        };
        let prev_disk_mode = plan.frontmatter.mode;
        match prev_disk_mode {
            file_store::PlanFileMode::Planning | file_store::PlanFileMode::Pending => {}
            file_store::PlanFileMode::Executing => {
                return Err(PlanRuntimeError::BuildBlocked(format!(
                    "PlanFile {plan_id} mode=executing；可能被其它进程占用，请稍后或手工修复"
                )));
            }
            file_store::PlanFileMode::Completed => {
                return Err(PlanRuntimeError::BuildBlocked(format!(
                    "PlanFile {plan_id} mode=completed；已完成的 plan 不可再 build"
                )));
            }
        }
        // ─── 准备五件事 ────────────────────────────────────────────────
        let mut warnings: Vec<String> = Vec::new();
        if matches!(prev_disk_mode, file_store::PlanFileMode::Pending) {
            if let Some(prev_key) = &plan.frontmatter.session_key {
                if prev_key != self.session_key.as_str() {
                    warnings.push(format!(
                        "pending plan {plan_id} 原绑定 session_key={prev_key}；本次将覆盖为 {}",
                        self.session_key
                    ));
                }
            }
        }
        // 1, 2: frontmatter 改 session_key/session_id/mode
        plan.frontmatter.session_key = Some(self.session_key.clone());
        plan.frontmatter.session_id = session_id;
        plan.frontmatter.mode = file_store::PlanFileMode::Executing;
        // 3: write_plan（原子）
        file_store::write_plan(&path, &plan, self.lock_timeout_ms)
            .map_err(PlanRuntimeError::from_plan_io)?;
        // 4: 切内存（写盘成功后才动）
        *self.mode.write() = PlanMode::Executing {
            plan_id: plan_id.to_string(),
        };
        *self.active_planning_plan_id.lock() = None;
        // 5: 首轮注入旗标 + 缓存 plan body（含 frontmatter 以便上下文可读）
        let plan_text = file_store::serialize_plan_file(&plan)
            .unwrap_or_else(|_| plan.body.clone());
        *self.first_exec_turn_body.lock() = Some(plan_text);
        self.first_exec_turn_pending.store(true, Ordering::Release);

        Ok(BuildPlanOutcome {
            plan_id: plan_id.to_string(),
            prev_disk_mode,
            warnings,
        })
    }

    /// 首轮 EXEC user message 装配时调用：返回 plan 全文（含 frontmatter）一次；
    /// 再次调用返回 None（防止重复注入超长上下文）。
    ///
    /// 标志 + 缓存语义：`build_plan` 成功后置 flag=true 并缓存 body。本方法 swap
    /// flag→false 后取走缓存（`take()`），返回前一次 build 缓存的 body；
    /// 与 catalog/reminder/prefix 派生平行——所有 EXEC 副作用均不可漏。
    pub fn consume_first_exec_turn_user_meta(&self) -> Option<String> {
        if self
            .first_exec_turn_pending
            .swap(false, Ordering::AcqRel)
        {
            self.first_exec_turn_body.lock().take()
        } else {
            None
        }
    }

    /// 测试 / debug：当前首轮注入是否仍 pending。
    #[doc(hidden)]
    pub fn first_exec_turn_pending_for_test(&self) -> bool {
        self.first_exec_turn_pending.load(Ordering::Acquire)
    }
}

/// `/plan build` 成功返回。
#[derive(Debug, Clone)]
pub struct BuildPlanOutcome {
    pub plan_id: String,
    /// 目标 PlanFile 的写前 mode（planning / pending）；命令层据此打印不同提示。
    pub prev_disk_mode: file_store::PlanFileMode,
    /// 非致命警告（如 pending 续跑 session_key 不一致）。
    pub warnings: Vec<String>,
}

/// reviewer 子 Agent 派发器 trait（解耦真实 LLM + AgentRegistry）。
///
/// **契约**：
/// - 调用方（`PlanRuntime::dispatch_reviewer`）保证：调度时 plan 文件 advisory lock 已 release（RV14）。
/// - dispatch 内部应通过 [`crate::core::agent_registry::AgentRegistry::spawn_subagent_internal`]
///   构造子 `AgentLoop`，把 `abort_signal` 透传给 `AgentLoopConfig`。
/// - 返回 `ReviewSummary`：成功 / aborted / parse_failed 都用同一形态承载。
/// - **不**写父 transcript（reviewer 子 Agent 持独立 session_id；transcript 隔离 D11）。
#[async_trait]
pub trait ReviewerDispatcher: Send + Sync {
    async fn dispatch(
        &self,
        plan_id: &str,
        plan_text: &str,
        allow_review_edit: bool,
        abort_signal: Arc<std::sync::atomic::AtomicBool>,
    ) -> review::ReviewSummary;
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
    /// `/plan build` 闸门未通过（active todos / active plan / disk mode 不合规等）。
    #[error("/plan build 闸门未通过：{0}")]
    BuildBlocked(String),
    /// `/plan build` 指定 plan_id 不存在；`hint` 给出友好引导（"先 create_plan"）。
    #[error("plan_id={plan_id} 不存在：{hint}")]
    BuildPlanNotFound { plan_id: String, hint: String },
}

impl PlanRuntimeError {
    /// 包装 PlanFile IO/lock 错误为 `Io`，保留细节给 chat_loop 打印。
    pub(crate) fn from_plan_io(e: file_store::PlanError) -> Self {
        match e {
            file_store::PlanError::NotFound { path } => {
                PlanRuntimeError::Io(format!("plan not found: {path}"))
            }
            other => PlanRuntimeError::Io(other.to_string()),
        }
    }
}
