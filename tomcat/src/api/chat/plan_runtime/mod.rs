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
//!                    /plan
//!         Chat ─────────────────────► Planning
//!          ▲                              │
//!          │                  /plan exit  │
//!          │  /plan exit                  ▼
//!          ├────────────── Pending { plan_id }
//!          │                  ▲       │
//!          │  cancel_token    │       │ /plan build <plan_id/path>
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
pub mod prod_reviewer;
pub mod prompts;
pub mod review;
pub mod safety;
pub mod session_prefix;
pub mod todo_runtime;
pub mod todos_panel;
pub mod tools;
pub mod verify;

#[cfg(test)]
mod tests;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

pub use ask_question_panel::AskQuestionPanel;
pub use mode::PlanMode;
pub use review::ReviewSummary;
pub use todos_panel::{
    CliTodosPanel, NoopTodosPanel, RefreshNotifier, TodosPanel, TodosPanelSnapshot,
};
pub use verify::VerifySummary;

/// PLAN 模式 per-session 编排器骨架（P1）。
///
/// 当前 PR-PLA 范围只支持：
/// - `/plan` → `enter_planning`
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
    /// 用于 `build_plan` / todos id 等固定 key 语义；当前实现里是 `DEFAULT_SESSION_KEY`。
    session_key: String,
    /// 当前 chat run 的真实 session_id。
    /// `recover()` / `reload_active_plan_from_disk()` 优先按这个字段判断 executing plan
    /// 是否属于本次 run，避免仅凭固定的 session_key 误认旧盘。
    current_session_id: Mutex<Option<String>>,
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
    /// 当前 active plan 的真实路径镜像。用于 EXEC/Planning 缺省目标解析与 user prefix
    /// 里的 `plan_path` 展示，尤其覆盖 `/plan build <plan_id/path>` 中的显式 path 场景。
    active_plan_path: Mutex<Option<PathBuf>>,
    /// `[plan] lock_timeout_ms`：write_plan / dispatch_reviewer 共享。默认 2000。
    lock_timeout_ms: u64,
    /// 可选 reviewer 派发器。P4 时由 `ChatContext::from_config` 注入真实实现；
    /// 测试可注入 mock；未注入时 `create_plan` 返回 `aborted=true` 占位摘要。
    reviewer: Mutex<Option<Arc<dyn ReviewerDispatcher>>>,
    /// 可选 verifier 派发器。PR-V1 由 `ChatContext::from_config` 注入真实实现；
    /// 测试可注入 mock；未注入时 `update_plan(all_completed)` 返回 `aborted` 占位摘要。
    verifier: Mutex<Option<Arc<dyn VerifierDispatcher>>>,
    /// `[plan].verify_gate` 当前值：`soft`（默认）或 `gate`。
    verify_gate_mode: RwLock<String>,
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
    /// `[ask_question].timeout_ms`：ask_question 等待用户回答的墙钟超时（毫秒）。
    /// `0` 表示无超时；生产由 `ChatContext::from_config` 写入；默认 0（按工具内置默认 300_000 处理）。
    ask_question_timeout_ms: std::sync::atomic::AtomicU64,
    /// Session-local todos 持久化目录根（如 `~/.tomcat/agents/<id>/`）。`None` 时 todos
    /// 工具仅写内存，不落盘——保留给纯单元测试与早期阶段使用。
    todos_persist_base: Mutex<Option<std::path::PathBuf>>,
    /// 当前 active todos 文件的 id（写入 sessions.json 的 `activeTodosId` 字段镜像）。
    /// 由 [`Self::ensure_active_todos_id`] 在首次写盘时生成 stable id；`todos.new_todos=true`
    /// 时可通过 [`Self::rotate_active_todos_id`] 显式切换到新文件。
    active_todos_id: Mutex<Option<String>>,
    /// E：UI 刷新广播——todos / update_plan 成功后，runtime 把 snapshot fanout 给所有
    /// 注册的 panel。生产由 `ChatContext::from_config` 注入 CLI/IDE 适配；测试可空。
    refresh_notifier: Arc<RefreshNotifier>,
    /// checkpoint store（默认 None；ChatContext::from_config 注入 ShadowGit/Noop）。
    /// 当前 plan runtime 仅在 `build_plan` 完成后按配置写
    /// `Manual{label="plan_build:<id>"}`；失败仅 warning。
    checkpoint_store: Mutex<Option<Arc<dyn crate::core::CheckpointStore>>>,
    /// `[plan].auto_checkpoint_on_build`：build_plan 时是否自动 record。默认 false。
    auto_checkpoint_on_build: AtomicBool,
    /// transcript 自定义事件 appender；由 `ChatContext::from_config` 装配
    /// `SessionManager::append_custom_entry` 的闭包。`None` 时 dispatch_reviewer 等不写
    /// transcript（单元测试 / 早期阶段）。
    transcript_appender: Mutex<Option<TranscriptAppender>>,
}

/// 由 PlanRuntime 调用，把 `serde_json::Value` 写入当前 transcript 的 `Custom` 行。
pub type TranscriptAppender =
    Arc<dyn Fn(serde_json::Value) -> Result<(), crate::infra::error::AppError> + Send + Sync>;

impl PlanRuntime {
    /// 构造一个绑定到 session_key 的 PlanRuntime。
    ///
    /// session_key 在 `ChatContext::from_config` 装配阶段已知（chat session 同生命周期）。
    /// 当前 P1 实现：`mode = Chat`，等待 `enter_planning` 或 `recover` 改写。
    pub fn new(session_key: impl Into<String>) -> Arc<Self> {
        Self::with_session_identity(
            session_key,
            None::<String>,
            file_store::DEFAULT_LOCK_TIMEOUT_MS,
        )
    }

    /// 显式给 `lock_timeout_ms`（测试用；生产从 `[plan] lock_timeout_ms` 读取）。
    pub fn with_lock_timeout(session_key: impl Into<String>, lock_timeout_ms: u64) -> Arc<Self> {
        Self::with_session_identity(session_key, None::<String>, lock_timeout_ms)
    }

    /// 生产装配入口：同时绑定固定 session_key 与本次 run 的真实 session_id。
    pub fn new_with_session_id(
        session_key: impl Into<String>,
        session_id: impl Into<String>,
    ) -> Arc<Self> {
        Self::with_session_identity(
            session_key,
            Some(session_id.into()),
            file_store::DEFAULT_LOCK_TIMEOUT_MS,
        )
    }

    fn with_session_identity(
        session_key: impl Into<String>,
        current_session_id: Option<String>,
        lock_timeout_ms: u64,
    ) -> Arc<Self> {
        Arc::new(Self {
            mode: RwLock::new(PlanMode::Chat),
            session_key: session_key.into(),
            current_session_id: Mutex::new(current_session_id),
            cancel_token: Mutex::new(None),
            session_todos: Mutex::new(Vec::new()),
            active_planning_plan_id: Mutex::new(None),
            active_plan_path: Mutex::new(None),
            lock_timeout_ms,
            reviewer: Mutex::new(None),
            verifier: Mutex::new(None),
            verify_gate_mode: RwLock::new("soft".into()),
            reviewer_rounds: parking_lot::Mutex::new(std::collections::HashMap::new()),
            ask_question_panel: Mutex::new(None),
            first_exec_turn_pending: AtomicBool::new(false),
            first_exec_turn_body: Mutex::new(None),
            ask_question_timeout_ms: std::sync::atomic::AtomicU64::new(0),
            todos_persist_base: Mutex::new(None),
            active_todos_id: Mutex::new(None),
            refresh_notifier: Arc::new(RefreshNotifier::new()),
            checkpoint_store: Mutex::new(None),
            auto_checkpoint_on_build: AtomicBool::new(false),
            transcript_appender: Mutex::new(None),
        })
    }

    fn owns_executing_plan(&self, plan: &file_store::PlanFile) -> bool {
        if let Some(current_id) = self.current_session_id.lock().clone() {
            return plan.frontmatter.session_id.as_deref() == Some(current_id.as_str());
        }
        plan.frontmatter.session_key.as_deref() == Some(self.session_key.as_str())
    }

    /// 注入 transcript 自定义事件 appender（由 `ChatContext::from_config` 装配）。
    pub fn attach_transcript_appender(&self, appender: TranscriptAppender) {
        *self.transcript_appender.lock() = Some(appender);
    }

    /// 写一条 transcript 自定义事件；appender 未注入时静默忽略（不阻塞主流程）。
    pub(crate) fn write_transcript_custom(&self, extra: serde_json::Value) {
        let appender = self.transcript_appender.lock().clone();
        if let Some(f) = appender {
            if let Err(e) = f(extra) {
                tracing::warn!(error = %e, "PlanRuntime::write_transcript_custom failed");
            }
        }
    }

    /// 注入 checkpoint store（生产 ShadowGit / 测试 Noop / Spy）。
    pub fn attach_checkpoint_store(&self, store: Arc<dyn crate::core::CheckpointStore>) {
        *self.checkpoint_store.lock() = Some(store);
    }

    /// 读 checkpoint store（克隆 Arc）。`None` 时跳过 record。
    pub fn checkpoint_store(&self) -> Option<Arc<dyn crate::core::CheckpointStore>> {
        self.checkpoint_store.lock().clone()
    }

    /// `[plan].auto_checkpoint_on_build` 当前值。
    pub fn auto_checkpoint_on_build(&self) -> bool {
        self.auto_checkpoint_on_build.load(Ordering::Acquire)
    }

    pub fn set_auto_checkpoint_on_build(&self, v: bool) {
        self.auto_checkpoint_on_build.store(v, Ordering::Release);
    }

    /// 注册一个 panel（CLI/IDE/test）；同一 runtime 可挂多个 panel，按注册顺序通知。
    pub fn register_todos_panel(&self, panel: Arc<dyn TodosPanel>) {
        self.refresh_notifier.register(panel);
    }

    /// 取出 `RefreshNotifier`（克隆 Arc）。`update_plan` / `todos` 写完后调
    /// `notify(&snapshot)` 触发 UI 刷新；调用方避免持锁时 notify（防 D2/D8 类回路）。
    pub fn refresh_notifier(&self) -> Arc<RefreshNotifier> {
        self.refresh_notifier.clone()
    }

    /// 注入 session-local todos 落盘根目录（生产由 `ChatContext::from_config` 通过
    /// `resolve_agent_trail_dir` 传入；测试与早期阶段可保持 `None`）。
    pub fn set_todos_persist_base(&self, base: Option<std::path::PathBuf>) {
        *self.todos_persist_base.lock() = base;
    }

    /// 读当前 todos 持久化根目录（克隆）。
    pub fn todos_persist_base(&self) -> Option<std::path::PathBuf> {
        self.todos_persist_base.lock().clone()
    }

    /// 当前 active todos 文件 id（mirrors `sessions.json.activeTodosId`）。
    pub fn active_todos_id(&self) -> Option<String> {
        self.active_todos_id.lock().clone()
    }

    /// 获取或派生当前 active todos id；首次调用时按"session_key + ms 时间戳"派生。
    pub fn ensure_active_todos_id(&self) -> String {
        let mut g = self.active_todos_id.lock();
        if let Some(id) = g.as_ref() {
            return id.clone();
        }
        let id = self.fresh_todos_id();
        *g = Some(id.clone());
        id
    }

    /// 强制切到一个新的 active todos id；供 `todos.new_todos=true` 使用。
    pub fn rotate_active_todos_id(&self) -> String {
        let mut g = self.active_todos_id.lock();
        let id = self.fresh_todos_id();
        *g = Some(id.clone());
        id
    }

    fn fresh_todos_id(&self) -> String {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        format!("td_{}_{now_ms}", self.session_key)
    }

    /// 读取 `[ask_question].timeout_ms`（N13 / B1 tool_exec 分发 `ask_question` 时使用）。
    /// 返回 `None` 表示「未配置」（工具按内置默认 300_000ms 处理）；`Some(0)` 表示无超时。
    pub fn ask_question_timeout_ms(&self) -> Option<u64> {
        let v = self
            .ask_question_timeout_ms
            .load(std::sync::atomic::Ordering::Acquire);
        if v == u64::MAX {
            None
        } else {
            Some(v)
        }
    }

    /// 由 `ChatContext::from_config` 在装配阶段写入。`None` → 内置默认；`Some(0)` → 无超时。
    pub fn set_ask_question_timeout_ms(&self, timeout_ms: Option<u64>) {
        let v = timeout_ms.unwrap_or(u64::MAX);
        self.ask_question_timeout_ms
            .store(v, std::sync::atomic::Ordering::Release);
    }

    /// 本 runtime 绑定的 session_key（只读）。
    pub fn session_key(&self) -> &str {
        &self.session_key
    }

    /// 读当前 mode（轻量 RwLock 读锁；不分配）。
    pub fn mode(&self) -> PlanMode {
        self.mode.read().clone()
    }

    /// `/plan` → 进入 Planning 模式。
    ///
    /// 在 P2 接入 `file_store` 前，本方法只做内存状态切换：`Chat | Completed { .. } → Planning`。
    /// 已在 `Planning` / `Executing` / `Pending` 时返回 `Err`（用户须先 `/plan exit` 或 `/plan build`）。
    pub fn enter_planning(&self) -> Result<(), PlanRuntimeError> {
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

    /// `/plan exit` → 退回 Chat。
    ///
    /// **N3（2026-05）**：仅在 `Planning` 模式下允许。Executing 期不允许通过 `/plan exit`
    /// 退出（需要用户发 SIGINT 或 Ctrl+C 走 cancel→pending 路径）；Pending / Completed
    /// 也不允许（已退出 plan 模式，再 exit 无意义；返回 `NotInPlanning`）。
    ///
    /// **不**删 plan 文件（PR-PLE：退出规划不删盘）。
    pub fn exit_to_chat(&self) -> Result<(), PlanRuntimeError> {
        let mut mode = self.mode.write();
        match &*mode {
            PlanMode::Planning => {
                *mode = PlanMode::Chat;
                Ok(())
            }
            PlanMode::Chat => Err(PlanRuntimeError::AlreadyInMode("chat".into())),
            other => Err(PlanRuntimeError::NotInPlanning(other.as_str().into())),
        }
    }

    /// 启动 recover（E8）：扫描 `~/.tomcat/plans/` 还原 active plan。
    ///
    /// 规则（plan-runtime.md §4.1 D7 / D6 / E8）：
    /// - PlanFile.mode == Executing：
    ///   - 当前 run 拥有（优先 `session_id == current_session_id`；测试旧入口无 session_id
    ///     时回退到 `session_key == self.session_key()`）→ 内存切回 `Executing { plan_id }`，
    ///     缓存 plan 全文供首轮 user_meta 注入（与 `build_plan` 一致）；
    ///   - 其它 session 遗留：
    ///     - 生产 fresh session（已有 `current_session_id`）→ **忽略**，不接管、也不改盘；
    ///     - 旧测试入口（无 `current_session_id`）→ 仍按 session_key 语义做 pending demote。
    /// - 其它 mode（Planning / Pending / Completed）→ 不动盘，不切内存（用户用 `/restore`
    ///   或新 /plan 再决定）。
    ///
    /// 防御：单条 plan 错误（解析失败 / lock 超时 / IO）只 warning 跳过，不 Err，
    /// 保证 chat 主流程能继续启动（D 防御：磁盘异常不阻 in-memory）。
    pub fn recover(&self) -> Result<(), PlanRuntimeError> {
        // 启动阶段允许 mode == Chat；recover 自己只读 + 选择性写。
        let plans_dir = match file_store::plans_dir() {
            Ok(p) => p,
            Err(_) => return Ok(()),
        };
        let entries = match std::fs::read_dir(&plans_dir) {
            Ok(e) => e,
            Err(_) => return Ok(()),
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() || !path.to_string_lossy().ends_with(".plan.md") {
                continue;
            }
            let plan = match file_store::read_plan(&path) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(target: "plan_runtime::recover",
                        "跳过损坏的 plan 文件 {}: {e}", path.display());
                    continue;
                }
            };
            let plan_id = plan.frontmatter.plan_id.clone();
            if plan.frontmatter.mode == file_store::PlanFileMode::Executing {
                if self.owns_executing_plan(&plan) {
                    // 当前 session 拥有 → 还原内存到 EXEC
                    let plan_text = file_store::serialize_plan_file(&plan)
                        .unwrap_or_else(|_| plan.body.clone());
                    *self.first_exec_turn_body.lock() = Some(plan_text);
                    self.first_exec_turn_pending.store(true, Ordering::Release);
                    *self.mode.write() = PlanMode::Executing { plan_id };
                    *self.active_plan_path.lock() = Some(path.clone());
                } else if self.current_session_id.lock().is_none() {
                    // 异 session 遗留 → 盘 demote 到 pending（保留 body / todos）
                    let mut demoted = plan.clone();
                    demoted.frontmatter.mode = file_store::PlanFileMode::Pending;
                    let owner_key = plan.frontmatter.session_key.as_deref().unwrap_or("");
                    let owner_id = plan.frontmatter.session_id.as_deref().unwrap_or("");
                    if let Err(e) = file_store::write_plan(&path, &demoted, self.lock_timeout_ms) {
                        tracing::warn!(target: "plan_runtime::recover",
                            "降级孤儿 executing plan {} 到 pending 失败: {e}", plan_id);
                    } else {
                        tracing::info!(target: "plan_runtime::recover",
                            "孤儿 executing plan {} 已降级为 pending（原 session_key={} session_id={}）",
                            plan_id,
                            owner_key,
                            owner_id);
                    }
                } else {
                    let current_session_id =
                        self.current_session_id.lock().clone().unwrap_or_default();
                    tracing::info!(target: "plan_runtime::recover",
                        "跳过其他 session_id 的 executing plan {}（disk session_id={} current session_id={}）",
                        plan_id,
                        plan.frontmatter.session_id.as_deref().unwrap_or(""),
                        current_session_id);
                }
            }
        }
        Ok(())
    }

    /// E7：`/restore` 命令完成 git 树恢复后，重新读取磁盘上的 active plan
    /// （优先 `session_id == current_session_id`；测试旧入口无 session_id 时回退 session_key）
    /// 并把内存 EXEC 状态对齐。
    ///
    /// 返回还原后的 `plan_id`（若未发现 active plan 返回 None）。
    pub fn reload_active_plan_from_disk(&self) -> Result<Option<String>, PlanRuntimeError> {
        let plans_dir = file_store::plans_dir().map_err(|e| PlanRuntimeError::Io(e.to_string()))?;
        let entries = match std::fs::read_dir(&plans_dir) {
            Ok(e) => e,
            Err(_) => return Ok(None),
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() || !path.to_string_lossy().ends_with(".plan.md") {
                continue;
            }
            let plan = match file_store::read_plan(&path) {
                Ok(p) => p,
                Err(_) => continue,
            };
            if matches!(plan.frontmatter.mode, file_store::PlanFileMode::Executing)
                && self.owns_executing_plan(&plan)
            {
                let plan_id = plan.frontmatter.plan_id.clone();
                let plan_text =
                    file_store::serialize_plan_file(&plan).unwrap_or_else(|_| plan.body.clone());
                *self.first_exec_turn_body.lock() = Some(plan_text);
                self.first_exec_turn_pending.store(true, Ordering::Release);
                *self.mode.write() = PlanMode::Executing {
                    plan_id: plan_id.clone(),
                };
                *self.active_plan_path.lock() = Some(path.clone());
                return Ok(Some(plan_id));
            }
        }
        // 无 active executing plan：保持当前内存 mode 不变（/restore 只是树恢复，
        // 并不强制改变 plan_runtime 状态机）。
        Ok(None)
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

    /// Planning 模式下记 active_plan_id 与真实 plan_path。
    ///
    /// 用 Planning(plan_id?) 容易破坏现有 `PlanMode` 形态（P1 已签合约：Planning 不带 plan_id）。
    /// 改为另存 `active_planning_plan_id` + `active_plan_path`，仅在 Planning 状态有意义。
    pub fn set_active_planning_plan(&self, plan_id: String, path: PathBuf) {
        *self.active_planning_plan_id.lock() = Some(plan_id);
        *self.active_plan_path.lock() = Some(path);
    }

    /// 读 Planning 模式下的 active_plan_id。EXEC/Pending 应直接看 `mode().active_plan_id()`。
    pub fn active_planning_plan_id(&self) -> Option<String> {
        self.active_planning_plan_id.lock().clone()
    }

    /// 当前 active plan 的真实路径；若本 session 还未绑定任何 plan，则返回 None。
    pub fn active_plan_path(&self) -> Option<PathBuf> {
        self.active_plan_path.lock().clone()
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
        if let Some(path) = self
            .mode()
            .active_plan_id()
            .and_then(|id| file_store::plan_path_for_id(id).ok())
        {
            *self.active_plan_path.lock() = Some(path);
        }
    }

    // ─── P4 reviewer 派发 API（plan-runtime.md §P4） ──────────────────────

    /// 注入 reviewer 派发器（生产由 `ChatContext::from_config` 装配 reviewer 子 Agent 派发；
    /// 测试可注入 [`review::MockReviewerDispatcher`] / 自定义实现）。
    pub fn attach_reviewer(&self, dispatcher: Arc<dyn ReviewerDispatcher>) {
        *self.reviewer.lock() = Some(dispatcher);
    }

    /// 注入 verifier 派发器（生产由 `ChatContext::from_config` 装配 verifier 子 Agent 派发；
    /// 测试可注入 mock / 自定义实现）。
    pub fn attach_verifier(&self, dispatcher: Arc<dyn VerifierDispatcher>) {
        *self.verifier.lock() = Some(dispatcher);
    }

    /// 设置 `[plan].verify_gate` 当前值。仅接受 `soft` / `gate`；其它值回落为 `soft`。
    pub fn set_verify_gate_mode(&self, value: impl Into<String>) {
        let normalized = match value.into().trim().to_ascii_lowercase().as_str() {
            "gate" => "gate",
            _ => "soft",
        };
        *self.verify_gate_mode.write() = normalized.to_string();
    }

    /// 当前 `[plan].verify_gate` 值（标准化后，仅 `soft` / `gate`）。
    pub fn verify_gate_mode(&self) -> String {
        self.verify_gate_mode.read().clone()
    }

    /// 是否处于 gate 严模式。
    pub fn verify_gate_is_strict(&self) -> bool {
        self.verify_gate_mode.read().as_str() == "gate"
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
            Err(e) => return review::ReviewSummary::aborted_with(format!("read plan 失败: {e}")),
        };

        let cascade = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut summary = dispatcher
            .dispatch(plan_id, &plan_text, allow_review_edit, cascade)
            .await;
        if rounds > 1 {
            summary.summary = format!("[round {rounds}] {}", summary.summary);
        }
        // 落 transcript 自定义事件（reviewer.md §11 / events::wire::WIRE_PLAN_REVIEW）。
        // 失败仅 warning，create_plan 主流程不受影响。
        let mut review_payload = summary.to_json();
        if let Some(obj) = review_payload.as_object_mut() {
            obj.insert(
                "event".to_string(),
                serde_json::Value::String(crate::infra::wire::WIRE_PLAN_REVIEW.to_string()),
            );
            obj.insert(
                "plan_id".to_string(),
                serde_json::Value::String(plan_id.to_string()),
            );
            obj.insert(
                "rounds".to_string(),
                serde_json::Value::Number(serde_json::Number::from(rounds)),
            );
        }
        self.write_transcript_custom(review_payload);
        // round > 1 时额外写一条 warning 事件，便于审计排查 "为何复盘了 N 次"。
        if rounds > 1 {
            let warn_payload = serde_json::json!({
                "event": crate::infra::wire::WIRE_PLAN_REVIEW_WARNING,
                "plan_id": plan_id,
                "rounds": rounds,
                "reviewer_turns_used": summary.reviewer_turns_used,
                "reviewer_turns_limit": summary.reviewer_turns_limit,
                "reviewer_stop_reason": summary.reviewer_stop_reason,
            });
            self.write_transcript_custom(warn_payload);
        }
        summary
    }

    /// 同步派发 verifier。语义与 reviewer 类似，但无 round 概念：
    ///
    /// 1. **必须**在 `write_plan` 释放 advisory lock **之后**调用。
    /// 2. 读取 plan 文件 → 调 dispatcher → 解析 `<verify>` block → 返回 `VerifySummary`。
    /// 3. 失败 / parse 错 / max_turns / parent abort → `verdict=aborted`；
    ///    调用方（`update_plan`）**不**因此失败，而是按 `verify_gate` 决定是否收工。
    /// 4. 若 dispatcher 未注入 → 返回 `placeholder_pending`。
    pub async fn dispatch_verifier(&self, plan_id: &str) -> verify::VerifySummary {
        let Some(dispatcher) = self.verifier.lock().clone() else {
            return verify::VerifySummary::placeholder_pending();
        };
        let path = match file_store::plan_path_for_id(plan_id) {
            Ok(p) => p,
            Err(e) => return verify::VerifySummary::aborted_with(format!("plan_id 非法: {e}")),
        };
        let plan_text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => return verify::VerifySummary::aborted_with(format!("read plan 失败: {e}")),
        };

        let cascade = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let summary = dispatcher.dispatch(plan_id, &plan_text, cascade).await;

        let mut payload = summary.to_json();
        if let Some(obj) = payload.as_object_mut() {
            obj.insert(
                "event".to_string(),
                serde_json::Value::String(crate::infra::wire::WIRE_PLAN_VERIFY.to_string()),
            );
            obj.insert(
                "plan_id".to_string(),
                serde_json::Value::String(plan_id.to_string()),
            );
        }
        self.write_transcript_custom(payload);
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

    fn looks_like_plan_path(plan_id_or_path: &str) -> bool {
        plan_id_or_path.contains('/')
            || plan_id_or_path.contains('\\')
            || plan_id_or_path.starts_with('.')
            || plan_id_or_path.starts_with('~')
            || plan_id_or_path.ends_with(".plan.md")
    }

    fn resolve_build_target(
        &self,
        plan_id_or_path: &str,
    ) -> Result<(PathBuf, file_store::PlanFile), PlanRuntimeError> {
        if !Self::looks_like_plan_path(plan_id_or_path) {
            safety::assert_plan_id_safe(plan_id_or_path)?;
            let path = file_store::plan_path_for_id(plan_id_or_path)
                .map_err(|e| PlanRuntimeError::Io(e.to_string()))?;
            let plan = match file_store::read_plan(&path) {
                Ok(p) => p,
                Err(file_store::PlanError::NotFound { .. }) => {
                    return Err(PlanRuntimeError::BuildPlanNotFound {
                        plan_id: plan_id_or_path.to_string(),
                        hint: format!(
                            "未找到 ~/.tomcat/plans/{plan_id_or_path}.plan.md；先通过 PLAN 模式 create_plan 生成"
                        ),
                    });
                }
                Err(e) => return Err(PlanRuntimeError::Io(e.to_string())),
            };
            return Ok((path, plan));
        }

        let path = crate::infra::platform::normalize_path(plan_id_or_path)
            .map_err(|e| PlanRuntimeError::Io(e.to_string()))?;
        let plan = match file_store::read_plan(&path) {
            Ok(p) => p,
            Err(file_store::PlanError::NotFound { .. }) => {
                return Err(PlanRuntimeError::BuildPlanPathNotFound {
                    path: crate::infra::platform::format_home_path(&path),
                    hint: "检查 plan path 是否正确，或改用 /plan build <plan_id/path>".into(),
                });
            }
            Err(e) => return Err(PlanRuntimeError::Io(e.to_string())),
        };
        safety::assert_plan_id_safe(&plan.frontmatter.plan_id)?;
        Ok((path, plan))
    }

    /// `/plan build <plan_id/path>` 入口；执行 plan-runtime §5.1 的 5 件事 + 原子回滚。
    ///
    /// **闸门**（任一不通过 → `BuildBlocked`）：
    /// - 当前内存 mode ∈ `{Chat, Planning}`（已 Executing/Pending/Completed 拒）
    /// - 当前 session 无 active todos（`session_todos` 全 completed/cancelled 或空）
    /// - 当前 session 无 active plan_id 占用（mode 不是 Executing/Pending 即可，
    ///   Planning 期 active_planning_plan_id 与目标 plan_id 相同时不算冲突）
    /// - 目标 PlanFile 必须存在（不存在 → `BuildPlanNotFound` / `BuildPlanPathNotFound`，附友好提示）
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
        plan_id_or_path: &str,
        session_id: Option<String>,
    ) -> Result<BuildPlanOutcome, PlanRuntimeError> {
        let (path, mut plan) = self.resolve_build_target(plan_id_or_path)?;
        let plan_id = plan.frontmatter.plan_id.clone();
        // ─── 闸门 1：内存 mode ─────────────────────────────────────────
        {
            let mode = self.mode.read();
            match &*mode {
                PlanMode::Chat | PlanMode::Planning => { /* 允许 */ }
                PlanMode::Pending { plan_id: cur } if cur == &plan_id => {
                    // N3（2026-05）：Pending 的本盘可直接续跑 build。
                }
                PlanMode::Executing { plan_id: cur } => {
                    return Err(PlanRuntimeError::BuildBlocked(format!(
                        "当前 session 已在 EXEC（plan_id={cur}）；先等结束或 cancel→pending"
                    )));
                }
                PlanMode::Pending { plan_id: cur } => {
                    return Err(PlanRuntimeError::BuildBlocked(format!(
                        "当前 session 有 pending plan {cur}；先续跑本盘或 /restore 切别盘"
                    )));
                }
                PlanMode::Completed { plan_id: cur } => {
                    return Err(PlanRuntimeError::BuildBlocked(format!(
                        "当前 session 已 completed plan {cur}；新计划请先 /restore 或 /plan"
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
                    "当前 session 有未完成 todos；先收口（todos remove / set completed）后再 build"
                        .into(),
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
        plan.frontmatter.session_id = session_id.clone();
        plan.frontmatter.mode = file_store::PlanFileMode::Executing;
        // 3: write_plan（原子）
        file_store::write_plan(&path, &plan, self.lock_timeout_ms)
            .map_err(PlanRuntimeError::from_plan_io)?;
        // 4: 切内存（写盘成功后才动）
        *self.mode.write() = PlanMode::Executing {
            plan_id: plan_id.to_string(),
        };
        *self.active_planning_plan_id.lock() = None;
        *self.active_plan_path.lock() = Some(path.clone());
        // 5: 首轮注入旗标 + 缓存 plan body（含 frontmatter 以便上下文可读）
        let plan_text =
            file_store::serialize_plan_file(&plan).unwrap_or_else(|_| plan.body.clone());
        *self.first_exec_turn_body.lock() = Some(plan_text);
        self.first_exec_turn_pending.store(true, Ordering::Release);

        // E6：`[plan].auto_checkpoint_on_build`（默认 false）→ 写 `Manual{label="plan_build:..."}`。
        // record 失败仅 warning（盘异常不阻 EXEC 推进，D 防御）。
        if self.auto_checkpoint_on_build() {
            if let Some(store) = self.checkpoint_store() {
                let req = crate::core::CheckpointRecordRequest {
                    session_id: session_id
                        .clone()
                        .unwrap_or_else(|| self.session_key.clone()),
                    turn_id: format!("plan_build-{plan_id}"),
                    kind: crate::core::CheckpointKind::Manual {
                        label: format!("plan_build:{plan_id}"),
                    },
                    message_anchor: None,
                    notes: Some(serde_json::json!({ "plan_id": plan_id })),
                };
                if let Err(e) = store.record(req) {
                    warnings.push(format!("plan_build checkpoint record 失败: {e}"));
                    tracing::warn!(target: "plan_runtime::build",
                        "plan_build checkpoint record 失败: {e}");
                }
            }
        }

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
        if self.first_exec_turn_pending.swap(false, Ordering::AcqRel) {
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

    // ─── P7 PR-PLF cancel→pending + 释放锁（plan-runtime.md §5.6） ───────

    /// 当 cancel_token 触发 / Ctrl+C 时调；只在 EXEC 模式生效。
    ///
    /// **副作用**（事务序）：
    /// 1. 读当前 plan 文件
    /// 2. 写 frontmatter.mode = pending（atomic + advisory lock；写完即释放，防 D1）
    /// 3. 内存 mode 切 `Pending { plan_id }`，清 `first_exec_turn_pending`
    /// 4. 返回 plan_id 给上层做 transcript `plan.cancel.demote_to_pending`
    ///
    /// **幂等**：非 EXEC 模式直接返回 Ok(None)。
    /// **错误**：磁盘读/写失败不修改内存 mode，返回 `Io`；上层应仅 warning（D8）。
    pub fn demote_to_pending_on_cancel(&self) -> Result<Option<String>, PlanRuntimeError> {
        // ① snapshot 当前 mode
        let plan_id = match &*self.mode.read() {
            PlanMode::Executing { plan_id } => plan_id.clone(),
            _ => return Ok(None),
        };
        // ② 改写磁盘
        let path = match self.active_plan_path() {
            Some(path) => path,
            None => file_store::plan_path_for_id(&plan_id)
                .map_err(|e| PlanRuntimeError::Io(e.to_string()))?,
        };
        let mut plan = file_store::read_plan(&path).map_err(PlanRuntimeError::from_plan_io)?;
        plan.frontmatter.mode = file_store::PlanFileMode::Pending;
        file_store::write_plan(&path, &plan, self.lock_timeout_ms)
            .map_err(PlanRuntimeError::from_plan_io)?;
        // ③ 内存切 Pending；reset 首轮注入
        *self.mode.write() = PlanMode::Pending {
            plan_id: plan_id.clone(),
        };
        self.first_exec_turn_pending.store(false, Ordering::Release);
        *self.first_exec_turn_body.lock() = None;
        Ok(Some(plan_id))
    }

    /// 挂接当前回合的 cancel_token；chat_loop 每轮 readline 后必须调（D2 防御）。
    ///
    /// 该 API 仅保存 token；真正的 cancel→pending 由 chat_loop 在 `select! cancel_token.cancelled()`
    /// 分支显式调 `demote_to_pending_on_cancel()` 触发——避免后台 spawn task 持 Arc<Self>
    /// 导致 PlanRuntime 生命周期跨 turn 泄漏。
    pub fn attach_cancel_hook(&self, token: CancellationToken) {
        *self.cancel_token.lock() = Some(token);
    }

    /// 当前回合的 cancel_token（克隆）。chat_loop 可以从这里取出，与新建的 token 比对，
    /// 决定是否需要重挂（D2：每轮 readline 后必须重挂，否则上一轮 hook 失效）。
    pub fn current_cancel_token(&self) -> Option<CancellationToken> {
        self.cancel_token.lock().clone()
    }

    // ─── P7 PR-PLE all-completed → CHAT 派生（plan-runtime.md §5.7） ─────

    /// 当 mode 已是 `Completed { plan_id }` 时，把内存复位到 CHAT、清首轮注入、清 active_planning；
    /// 通常由 chat_loop 在下一轮装配前调用，等价于"自然收口"。
    ///
    /// `update_plan` 写盘成功后会 `set_mode_completed`；本方法是从 Completed → Chat 的最后一跳；
    /// 也可在 chat_loop 收到 `plan.complete` 事件后立即调用，避免下一轮仍带 EXEC reminder。
    pub fn finalize_completed_to_chat(&self) -> Option<String> {
        let mut mode = self.mode.write();
        match &*mode {
            PlanMode::Completed { plan_id } => {
                let pid = plan_id.clone();
                *mode = PlanMode::Chat;
                self.first_exec_turn_pending.store(false, Ordering::Release);
                *self.first_exec_turn_body.lock() = None;
                *self.active_planning_plan_id.lock() = None;
                *self.active_plan_path.lock() = None;
                Some(pid)
            }
            _ => None,
        }
    }

    // ─── P7 PR-PLF raw edit 拦截（plan-runtime.md §5.6） ─────────────────

    /// PLAN/EXEC 模式下，`tool_exec::write`/`edit` 等 raw 写入路径调用此 helper
    /// 判断该路径是否允许写入。
    ///
    /// **规则**：
    /// - 不是 `~/.tomcat/plans/*.plan.md` → 允许（其他文件不归本 runtime 管）
    /// - 是 `~/.tomcat/plans/*.plan.md`：
    ///   - CHAT 模式 → 允许（无 PLAN/EXEC 守卫）
    ///   - Planning/Executing/Pending/Completed → 拒（必须走 `create_plan`/`update_plan`/`todos` 工具，
    ///     由 runtime 做 frontmatter diff / 锁等保护）
    ///
    /// 调用方负责把返回 false 的写入请求转成 ToolError，并提示"请使用 update_plan"。
    pub fn allow_raw_edit_to_path(&self, path: &std::path::Path) -> bool {
        let plans_dir = match file_store::plans_dir() {
            Ok(p) => p,
            Err(_) => return true,
        };
        // macOS `/var/folders` 实际是 `/private/var/folders` 的 symlink；只比较
        // canonical 形态可避免误放过 plan_dir 下的写入。两侧都尽量 canonicalize。
        let canon_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let canon_plans = plans_dir.canonicalize().unwrap_or(plans_dir);
        if !canon_path.starts_with(&canon_plans) {
            return true;
        }
        matches!(*self.mode.read(), PlanMode::Chat)
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

/// verifier 子 Agent 派发器 trait（解耦真实 LLM + AgentRegistry）。
///
/// **契约**：
/// - 调用方（`PlanRuntime::dispatch_verifier`）保证：调度时 plan 文件 advisory lock 已 release。
/// - dispatch 内部应通过 [`crate::core::agent_registry::AgentRegistry::spawn_subagent_internal`]
///   构造子 `AgentLoop`，把 `abort_signal` 透传给 `AgentLoopConfig`。
/// - 返回 `VerifySummary`：成功 / aborted / parse_failed 都用同一形态承载。
/// - **不**写父 transcript（verifier 子 Agent 持独立 session_id；transcript 隔离）。
#[async_trait]
pub trait VerifierDispatcher: Send + Sync {
    async fn dispatch(
        &self,
        plan_id: &str,
        plan_text: &str,
        abort_signal: Arc<std::sync::atomic::AtomicBool>,
    ) -> verify::VerifySummary;
}

/// `PlanRuntime` 操作错误。
#[derive(Debug, thiserror::Error)]
pub enum PlanRuntimeError {
    #[error("当前已经在 {0} 模式，无法重复进入")]
    AlreadyInMode(String),
    /// N3（2026-05）：`/plan exit` 只允许在 Planning 模式下使用。
    #[error("/plan exit 仅在 Planning 模式可用；当前模式 = {0}")]
    NotInPlanning(String),
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
    #[error("plan path={path} 不存在：{hint}")]
    BuildPlanPathNotFound { path: String, hint: String },
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
