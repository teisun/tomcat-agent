//! # PlanRuntime — per-session PLAN 模式编排器（T2-P1-002/003/004）
//!
//! `PlanRuntime` 与 `TodosRuntime` 是 PLAN/CHAT 相关的两条 per-session 运行态：前者持有当前
//! 会话模式、active plan 缓存、reviewer 派发逻辑，以及 session-local todos 的内存态；
//! 后者只负责把这份 session-local todos 持久化到 agent 级 `.todo.md`。
//! 它们都挂在 `ChatContext` 上，与 chat session 同生命周期（**不**每轮重建，否则 `mode`
//! 会被重置回 Chat，丢失 PLAN 的持续语义）。
//!
//! ## 正交状态
//!
//! ```text
//! AgentMode:       Chat ── /plan ──► Plan ── /plan exit ──► Chat
//! PlanFile.state:  Planning ─ build ─► Executing ─ complete ─► Completed
//!                                      │
//!                                      └─ cancel ─► Pending ─ build ─► Executing
//! ```
//!
//! ## 模块组织
//!
//! - [`active_plan`]：当前绑定计划的标识、路径和最后同步的文件生命周期状态
//! - [`catalog`]：会话可用工具目录
//! - [`reminders`]：PLANNER / EXECUTOR `<system_reminder>` 常量
//! - [`safety`]：`assert_plan_id_safe`（防穿越 `../` / `/` / 控制字符）
//!
//! P2 起补 `file_store` / `ops`（todos op）；P4 起补 `dispatch_reviewer`；P5 起补
//! `tools::ask_question`；P6 起补 `/plan build` 五件事；P7 起补 `panel` / `checkpoint` /
//! `cancel`。

pub mod active_plan;
pub mod catalog;
pub mod code_reviewer;
pub mod explorer;
pub mod file_store;
pub mod ops;
pub mod panels;
pub mod plan_reviewer;
pub mod prod_reviewer;
pub mod reminders;
pub mod review;
pub mod safety;
pub mod todo_runtime;
pub mod verify;

#[cfg(test)]
mod tests;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::core::session::manager::{AgentMode, ResumeControlState};

pub use active_plan::ActivePlan;
pub use code_reviewer::CodeReviewSummary;
pub use panels::{
    Answer, AskQuestionIdentity, AskQuestionOutcome, AskQuestionPanel, AskQuestionResult,
    AskQuestionTermination, AskQuestionTerminationReason, MockAskQuestionPanel, NoopTodosPanel,
    Question, QuestionOption, RefreshNotifier, TodosPanel, TodosPanelSnapshot, CUSTOM_OPTION_ID,
};
pub use plan_reviewer::{PlanReviewSummary, REVIEWER_ALLOW_REVIEW_EDIT};
pub use review::Finding;
pub use verify::VerifySummary;

/// 主 Agent 对一条 P1 finding 的明确取舍。
///
/// 只存 P1：P0 必须修复或交还用户，P2 从不阻塞、不需要申辩。`reference` 是本轮
/// `F0X` 审计标签，不是跨轮稳定 id；跨轮去重依靠 reviewer brief 中的完整文本与理由。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisputedFinding {
    pub reference: String,
    pub severity: String,
    pub area: String,
    pub note: String,
    pub resolution: String,
    pub reason: String,
}

/// The single authoritative todo source preserved in a compaction summary.
///
/// A readable active PlanFile wins even when it has no todos: that empty list is
/// still meaningful and must not silently fall back to an unrelated session
/// scratchpad. Only when no readable plan exists may non-empty scratchpad todos
/// provide Progress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProgressSource {
    PlanFile { todos: Vec<file_store::TodoItem> },
    SessionScratchpad { todos: Vec<file_store::TodoItem> },
}

/// 会话控制态快照。摘要的 `<control_state>` 机器区块（第 3 章）与恢复日志共用这一份取数，
/// 保证"UI 看到的模式"和"喂给模型的模式"永远来自同一个地方。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlSnapshot {
    /// chat | plan
    pub mode: AgentMode,
    pub plan_path: Option<PathBuf>,
    /// 计划文件 frontmatter 里的 state；读不到计划文件时为 None。
    pub plan_file_state: Option<String>,
    pub plan_id: Option<String>,
    pub model: Option<String>,
    /// Runtime-selected Progress source. See [`ProgressSource`] for the
    /// PlanFile-first, scratchpad-fallback invariant.
    pub progress: Option<ProgressSource>,
}

/// 恢复时读计划文件；不存在或读不动都当作"没有计划"，由调用方决定兜底。
fn read_plan_for_restore(path: &std::path::Path) -> Option<file_store::PlanFile> {
    if !path.is_file() {
        tracing::warn!(
            target: "plan_runtime::recover",
            path = %path.display(),
            "resume 记录的 plan 文件不存在"
        );
        return None;
    }
    match file_store::read_plan(path) {
        Ok(plan) => Some(plan),
        Err(err) => {
            tracing::warn!(
                target: "plan_runtime::recover",
                path = %path.display(),
                error = %err,
                "resume 记录的 plan 文件无法读取"
            );
            None
        }
    }
}

/// PLAN 模式 per-session 编排器骨架（P1）。
///
/// 当前 PR-PLA 范围只支持：
/// - `/plan` → `enter_plan`
/// - `/plan exit` → `exit_plan`
/// - `recover()`（启动时扫描 `~/.tomcat/plans/`）— 占位实现，P2 起接入 file_store
///
/// 后续 PR：`build_plan` / `cancel_to_pending` / `dispatch_reviewer` / `attach_cancel_hook` /
/// `decorate_messages` 在 P2-P7 逐步补齐；工具目录现保持稳定，由 handler 按 mode
/// 执行 policy；本结构体公共字段
/// 在 P1 已定型，避免后续多次扩字段引发的连锁修改。
pub struct PlanRuntime {
    /// 当前模式。每轮 `chat_loop` 装配 `tool_definitions` / system reminder / user prefix
    /// 都基于此值；跨 turn 持久（**禁止**每轮重建 `PlanRuntime`）。
    mode: RwLock<AgentMode>,
    /// 串行化“append transcript → 修改 mode → 通知”的提交窗口。
    ///
    /// 单独的 `mode` 读写锁无法保护先验检查与 append 之间的间隙；没有这把锁，两个并发
    /// `/plan` 都可能读到 Chat 并重复写出 mode changed 事件。
    agent_mode_transition_lock: Mutex<()>,
    /// 本 PlanRuntime 绑定的 session_key（来自 `SessionManager::current_session_key`）。
    /// 用于 `build_plan` / todos id 等固定 key 语义；当前实现里是 `DEFAULT_SESSION_KEY`。
    session_key: String,
    /// 当前 chat run 的真实 session_id。
    /// `recover()` / `sync_active_plan_from_disk()` 优先按这个字段判断 executing plan
    /// 是否属于本次 run，避免仅凭固定的 session_key 误认旧盘。
    current_session_id: Mutex<Option<String>>,
    /// 本回合 `CancellationToken` 的弱引用。chat_loop 每轮 readline 后重建 token，
    /// 必须立即 `attach_cancel_hook(&new_token)` 重挂，否则上一轮的 hook 监听
    /// 失效 → cancel→pending 不工作（D2 防御）。
    #[allow(dead_code)] // P7 接入
    cancel_token: Mutex<Option<CancellationToken>>,
    /// `todos` 工具的 session-local scratchpad，适用于所有模式（含 EXEC）；
    /// **绝不**写入 `PlanFile.frontmatter.todos[]`。plan 文件推进统一由 `update_plan`
    /// 负责；`.todo.md` 的持久化由独立的 `TodosRuntime` 接管。
    session_todos: Mutex<Vec<file_store::TodoItem>>,
    /// 当前绑定计划的文件状态缓存。计划文件是唯一权威；写盘后和每回合开始前刷新。
    active_plan: RwLock<Option<ActivePlan>>,
    /// `[plan] lock_timeout_ms`：write_plan / dispatch_reviewer 共享。默认 2000。
    lock_timeout_ms: u64,
    /// 可选 plan reviewer 派发器。未注入时 `create_plan` 返回 `aborted=true` 占位摘要。
    plan_reviewer: Mutex<Option<Arc<dyn PlanReviewerDispatcher>>>,
    /// 可选 code reviewer 派发器。未注入时 `update_plan(all_completed)` 返回占位摘要。
    code_reviewer: Mutex<Option<Arc<dyn CodeReviewerDispatcher>>>,
    /// 可选 verifier 派发器。PR-V1 由 `ChatContext::from_config` 注入真实实现；
    /// 测试可注入 mock；未注入时 `update_plan(all_completed)` 返回 `aborted` 占位摘要。
    verifier: Mutex<Option<Arc<dyn VerifierDispatcher>>>,
    /// 只读勘察子 Agent 派发器（`dispatch_agent` 工具的后端）。
    explorer: Mutex<Option<Arc<dyn ExplorerDispatcher>>>,
    /// `[plan].verify_gate` 当前值：`soft`（默认）或 `gate`。
    verify_gate_mode: RwLock<String>,
    /// green build 前 code reviewer 的最大尝试轮次。默认 2；0 表示直接跳过 code review。
    max_code_review_rounds: AtomicU32,
    /// 代码编辑使上一轮验收失效后，允许重新运行 review + green build 的最大周期。
    max_completion_gate_cycles: AtomicU32,
    /// 计数 reviewer 派发轮次（用于 `[reviewer] max_review_rounds` 软上限 warning）。
    reviewer_rounds: parking_lot::Mutex<std::collections::HashMap<String, u32>>,
    /// 计数 verifier 前 code reviewer 实际派发轮次。
    code_review_rounds: parking_lot::Mutex<std::collections::HashMap<String, u32>>,
    /// 执行中 plan 的 provider 缓存观测；用于在 `plan.complete` 留下本次 build 的实测比率。
    plan_cache_observations:
        parking_lot::Mutex<std::collections::HashMap<String, PlanCacheObservation>>,
    /// 上一次 code reviewer 派发的 wall-clock 时间，用 mtime 划定下一轮增量范围。
    last_code_review_dispatch_ms: parking_lot::Mutex<std::collections::HashMap<String, u128>>,
    /// 上一轮 code review 留下的未清 finding。下一轮 reviewer 逐条按语义核销已修项；
    /// 不用不稳定的跨轮 finding id。completion guard 也据此说明尚未解决的问题。
    unresolved_findings:
        parking_lot::Mutex<std::collections::HashMap<String, Vec<review::Finding>>>,
    /// 主 Agent 已书面申辩、用户需要看到的 P1 取舍。仅在当前计划执行期间保留；
    /// 重新 build 同一计划意味着重新开始一次审查，因此与未清 finding 一起清空。
    disputed_findings: parking_lot::Mutex<std::collections::HashMap<String, Vec<DisputedFinding>>>,
    /// 技术故障不会消耗正常 review 预算，但也不能无限重试。
    review_infra_retries: parking_lot::Mutex<std::collections::HashMap<String, u32>>,
    /// 根 Agent 工作区；无工作区（大多数纯单测 / 非 Git 项目）时跳过代码 diff 门禁。
    workspace_root: Mutex<Option<PathBuf>>,
    /// 后台 bash 任务账本，用于核验 green build 证据。
    bash_task_registry: Mutex<Option<Arc<crate::core::tools::primitive::BashTaskRegistry>>>,
    /// 当前会话正在用的主模型。run loop 每回合写入，reviewer / verifier 派发时读取——
    /// 这样会话中途换模型，子 Agent 下一次派发就跟着换。
    session_model: parking_lot::Mutex<Option<String>>,
    /// 可选 `ask_question` UI 后端（P5）。CLI 默认由 `ChatContext::from_config`
    /// 注入 `CliAskQuestionPanel`；宿主若要接 IDE / 测试 bridge，可通过 overrides
    /// 显式注入别的 `AskQuestionPanel`。未注入时 `ask_question` 工具返回
    /// `cancelled: true` 兜底（避免 panic / 卡死）。
    ask_question_panel: Mutex<Option<Arc<dyn AskQuestionPanel>>>,
    /// 当前 active todos scratchpad 的逻辑 id（不再参与磁盘文件命名）。
    /// `todos.new_todos=true` 时通过 [`Self::rotate_active_todos_id`] 切换，便于 tool result
    /// / panel 在内存层感知“新白板”。
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
    /// `[skills].expose_to_reviewer`：是否允许 reviewer/verifier 子 Agent 暴露技能目录与
    /// `load_skill` 工具。默认 false，由 `ChatContext::from_config` 装配。
    expose_skills_to_reviewer: AtomicBool,
    /// transcript 自定义事件 appender；由 `ChatContext::from_config` 装配
    /// `SessionManager::append_custom_entry` 的闭包。`None` 时 dispatch_reviewer 等不写
    /// transcript（单元测试 / 早期阶段）。
    transcript_appender: Mutex<Option<TranscriptAppender>>,
    /// 将已落盘 transcript 自定义事件广播给宿主。单独保存是为了让 mode transition
    /// 坚持「先落盘 → 改内存 → 广播」而不是让 appender 把三个动作混在一起。
    transcript_event_notifier: Mutex<Option<TranscriptEventNotifier>>,
}

/// 由 PlanRuntime 调用，把 `serde_json::Value` 写入当前 transcript 的 `Custom` 行。
pub type TranscriptAppender =
    Arc<dyn Fn(serde_json::Value) -> Result<(), crate::infra::error::AppError> + Send + Sync>;
pub type TranscriptEventNotifier = Arc<dyn Fn(serde_json::Value) + Send + Sync>;

/// Provider-reported cache observations collected only while one plan is executing.
///
/// This mirrors the session-level counters held by `ContextState`, but is deliberately
/// keyed by plan so `plan.complete` can emit the build's totals without reaching into the
/// mutable agent loop.
#[derive(Debug, Clone, Default)]
struct PlanCacheObservation {
    prompt_tokens_total: u64,
    cache_read_tokens_total: u64,
    cache_observed_prompt_tokens_total: u64,
    cache_observed_request_count: u32,
    consecutive_cache_miss: u32,
    consecutive_cache_miss_max: u32,
    tail_changed_count: u32,
    tail_change_miss_tokens: u64,
}

impl PlanCacheObservation {
    fn observe(&mut self, prompt_tokens: u32, cache_read_tokens: Option<u32>, tail_changed: bool) {
        self.prompt_tokens_total += u64::from(prompt_tokens);
        if tail_changed {
            self.tail_changed_count = self.tail_changed_count.saturating_add(1);
            self.tail_change_miss_tokens += u64::from(prompt_tokens);
        }
        let Some(cache_read_tokens) = cache_read_tokens else {
            return;
        };
        self.cache_observed_request_count = self.cache_observed_request_count.saturating_add(1);
        self.cache_observed_prompt_tokens_total += u64::from(prompt_tokens);
        self.cache_read_tokens_total += u64::from(cache_read_tokens);
        if cache_read_tokens == 0 {
            self.consecutive_cache_miss = self.consecutive_cache_miss.saturating_add(1);
            self.consecutive_cache_miss_max = self
                .consecutive_cache_miss_max
                .max(self.consecutive_cache_miss);
        } else {
            self.consecutive_cache_miss = 0;
        }
    }

    fn to_json(&self) -> serde_json::Value {
        let cache_hit_ratio = (self.cache_observed_prompt_tokens_total > 0).then(|| {
            self.cache_read_tokens_total as f64 / self.cache_observed_prompt_tokens_total as f64
        });
        serde_json::json!({
            "promptTokensTotal": self.prompt_tokens_total,
            "cacheReadTokensTotal": self.cache_read_tokens_total,
            "cacheObservedPromptTokensTotal": self.cache_observed_prompt_tokens_total,
            "cacheObservedRequestCount": self.cache_observed_request_count,
            "cacheHitRatio": cache_hit_ratio,
            "consecutiveMissMax": self.consecutive_cache_miss_max,
            "tailChangedCount": self.tail_changed_count,
            "tailChangeMissTokens": self.tail_change_miss_tokens,
        })
    }
}

impl PlanRuntime {
    /// 构造一个绑定到 session_key 的 PlanRuntime。
    ///
    /// session_key 在 `ChatContext::from_config` 装配阶段已知（chat session 同生命周期）。
    /// 运行时初始为 Chat；只有 `enter_plan` 或恢复的侧车状态会改写会话模式。
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
            mode: RwLock::new(AgentMode::Chat),
            agent_mode_transition_lock: Mutex::new(()),
            session_key: session_key.into(),
            current_session_id: Mutex::new(current_session_id),
            cancel_token: Mutex::new(None),
            session_todos: Mutex::new(Vec::new()),
            active_plan: RwLock::new(None),
            lock_timeout_ms,
            plan_reviewer: Mutex::new(None),
            code_reviewer: Mutex::new(None),
            verifier: Mutex::new(None),
            explorer: Mutex::new(None),
            verify_gate_mode: RwLock::new("soft".into()),
            max_code_review_rounds: AtomicU32::new(2),
            max_completion_gate_cycles: AtomicU32::new(3),
            reviewer_rounds: parking_lot::Mutex::new(std::collections::HashMap::new()),
            code_review_rounds: parking_lot::Mutex::new(std::collections::HashMap::new()),
            plan_cache_observations: parking_lot::Mutex::new(std::collections::HashMap::new()),
            last_code_review_dispatch_ms: parking_lot::Mutex::new(std::collections::HashMap::new()),
            unresolved_findings: parking_lot::Mutex::new(std::collections::HashMap::new()),
            disputed_findings: parking_lot::Mutex::new(std::collections::HashMap::new()),
            review_infra_retries: parking_lot::Mutex::new(std::collections::HashMap::new()),
            workspace_root: Mutex::new(None),
            bash_task_registry: Mutex::new(None),
            session_model: parking_lot::Mutex::new(None),
            ask_question_panel: Mutex::new(None),
            active_todos_id: Mutex::new(None),
            refresh_notifier: Arc::new(RefreshNotifier::new()),
            checkpoint_store: Mutex::new(None),
            auto_checkpoint_on_build: AtomicBool::new(false),
            expose_skills_to_reviewer: AtomicBool::new(false),
            transcript_appender: Mutex::new(None),
            transcript_event_notifier: Mutex::new(None),
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

    /// 注入已落盘 transcript 事件的宿主广播器。
    pub fn attach_transcript_event_notifier(&self, notifier: TranscriptEventNotifier) {
        *self.transcript_event_notifier.lock() = Some(notifier);
    }

    fn append_transcript_custom(&self, extra: serde_json::Value) -> Result<(), PlanRuntimeError> {
        if let Some(appender) = self.transcript_appender.lock().clone() {
            appender(extra).map_err(|error| PlanRuntimeError::Io(error.to_string()))?;
        }
        Ok(())
    }

    fn notify_transcript_event(&self, extra: serde_json::Value) {
        if let Some(notifier) = self.transcript_event_notifier.lock().clone() {
            notifier(extra);
        }
    }

    /// 写一条 transcript 自定义事件；appender 未注入时静默忽略（不阻塞主流程）。
    pub(crate) fn write_transcript_custom(&self, extra: serde_json::Value) {
        if let Err(error) = self.append_transcript_custom(extra.clone()) {
            tracing::warn!(error = %error, "PlanRuntime::write_transcript_custom failed");
            return;
        }
        self.notify_transcript_event(extra);
    }

    fn emit_plan_state_event(
        &self,
        event: &str,
        state: &str,
        plan_id: Option<&str>,
        explicit_path: Option<PathBuf>,
    ) {
        let path = explicit_path
            .or_else(|| self.active_plan_path())
            .or_else(|| plan_id.and_then(|id| file_store::plan_path_for_id(id).ok()))
            .map(|path| crate::infra::platform::format_home_path(&path));
        let mut payload = serde_json::Map::new();
        payload.insert(
            "event".to_string(),
            serde_json::Value::String(event.to_string()),
        );
        payload.insert(
            "state".to_string(),
            serde_json::Value::String(state.to_string()),
        );
        if let Some(plan_id) = plan_id {
            payload.insert(
                "plan_id".to_string(),
                serde_json::Value::String(plan_id.to_string()),
            );
        }
        if let Some(path) = path {
            payload.insert("path".to_string(), serde_json::Value::String(path));
        }
        self.write_transcript_custom(serde_json::Value::Object(payload));
    }

    fn transition_agent_mode(
        &self,
        expected: AgentMode,
        next: AgentMode,
    ) -> Result<(), PlanRuntimeError> {
        let _transition = self.agent_mode_transition_lock.lock();
        if self.mode() != expected {
            return Err(PlanRuntimeError::AlreadyInMode(self.mode().as_str().into()));
        }
        let event = serde_json::json!({
            "event": crate::infra::wire::WIRE_SESSION_AGENT_MODE_CHANGED,
            "agentMode": next.as_str(),
        });
        self.append_transcript_custom(event.clone())?;
        *self.mode.write() = next;
        self.notify_transcript_event(event);
        Ok(())
    }

    /// Build completes the Plan → Chat part of the mode transition only when Plan is still the
    /// current mode. A concurrent exit may have already performed it; that is a valid no-op and
    /// must not fail a plan file that was successfully promoted to executing.
    fn leave_plan_for_build(&self) -> Result<bool, PlanRuntimeError> {
        let _transition = self.agent_mode_transition_lock.lock();
        if self.mode() != AgentMode::Plan {
            return Ok(false);
        }
        let event = serde_json::json!({
            "event": crate::infra::wire::WIRE_SESSION_AGENT_MODE_CHANGED,
            "agentMode": AgentMode::Chat.as_str(),
        });
        self.append_transcript_custom(event.clone())?;
        *self.mode.write() = AgentMode::Chat;
        self.notify_transcript_event(event);
        Ok(true)
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

    /// 当前 active todos scratchpad id（mirrors 历史上的 `activeTodosId` 语义，但不再是文件名）。
    pub fn active_todos_id(&self) -> Option<String> {
        self.active_todos_id.lock().clone()
    }

    /// 获取或派生当前 active todos scratchpad id；首次调用时按"session_key + ms 时间戳"派生。
    pub fn ensure_active_todos_id(&self) -> String {
        let mut g = self.active_todos_id.lock();
        if let Some(id) = g.as_ref() {
            return id.clone();
        }
        let id = self.fresh_todos_id();
        *g = Some(id.clone());
        id
    }

    /// 强制切到一个新的 active todos scratchpad id；供 `todos.new_todos=true` 使用。
    pub fn rotate_active_todos_id(&self) -> String {
        let mut g = self.active_todos_id.lock();
        let id = self.fresh_todos_id();
        *g = Some(id.clone());
        id
    }

    /// 生成一个新的内存逻辑 scratchpad id；**不**参与 `.todo.md` 文件命名。
    fn fresh_todos_id(&self) -> String {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        format!("td_{}_{now_ms}", self.session_key)
    }

    pub fn current_session_id(&self) -> Option<String> {
        self.current_session_id.lock().clone()
    }

    /// 本 runtime 绑定的 session_key（只读）。
    pub fn session_key(&self) -> &str {
        &self.session_key
    }

    /// 读当前会话模式（轻量 RwLock 读锁；不分配）。
    pub fn mode(&self) -> AgentMode {
        *self.mode.read()
    }

    /// 当前会话绑定的计划文件缓存。
    pub fn active_plan(&self) -> Option<ActivePlan> {
        self.active_plan.read().clone()
    }

    /// 仅当绑定计划的磁盘状态为 executing 时返回其 id。
    pub fn executing_plan_id(&self) -> Option<String> {
        self.active_plan()
            .filter(ActivePlan::is_executing)
            .map(|plan| plan.id)
    }

    /// `/plan` → 进入 Plan 会话模式。
    pub fn enter_plan(&self) -> Result<(), PlanRuntimeError> {
        self.transition_agent_mode(AgentMode::Chat, AgentMode::Plan)
    }

    /// `/plan exit` → 退回 Chat 会话模式。计划文件保持原样。
    pub fn exit_plan(&self) -> Result<(), PlanRuntimeError> {
        self.transition_agent_mode(AgentMode::Plan, AgentMode::Chat)
    }

    /// 启动恢复：模式只来自 sidecar；计划文件只恢复 active plan 缓存。
    pub fn attach_from_resume_state(
        &self,
        state: ResumeControlState,
    ) -> Result<(), PlanRuntimeError> {
        *self.mode.write() = state.mode.unwrap_or(AgentMode::Chat);
        let active_plan = state.plan_path.as_ref().and_then(|path| {
            read_plan_for_restore(path).map(|plan| ActivePlan::from_file(path.clone(), &plan))
        });
        if state.plan_path.is_some() && active_plan.is_none() {
            self.write_transcript_custom(serde_json::json!({
                "event": crate::infra::wire::WIRE_PLAN_RESTORE,
                "state": "unavailable",
                "basis": "resume_plan_file_unreadable",
            }));
        }
        *self.active_plan.write() = active_plan;
        Ok(())
    }

    /// 记录当前会话主模型。run loop 每回合解析完 `LlmScene::Main` 后调用。
    pub fn set_session_model(&self, model: &str) {
        if model.is_empty() {
            return;
        }
        *self.session_model.lock() = Some(model.to_string());
    }

    /// 当前会话主模型；会话还没跑过任何一回合时为 None。
    pub fn session_model(&self) -> Option<String> {
        self.session_model.lock().clone()
    }

    /// 当前控制态快照。`model` 由调用方注入（PlanRuntime 不持有模型信息）。
    pub fn control_snapshot(&self, model: Option<&str>) -> ControlSnapshot {
        let mode = self.mode();
        let active_plan = self.active_plan();
        let plan_path = active_plan.as_ref().map(|plan| plan.path.clone());
        let plan = plan_path.as_deref().and_then(read_plan_for_restore);
        let plan_file_state = plan
            .as_ref()
            .map(|plan| plan.frontmatter.state.as_str().to_string());
        let progress = if let Some(plan) = plan {
            Some(ProgressSource::PlanFile {
                todos: plan.frontmatter.todos,
            })
        } else {
            let todos = self.snapshot_session_todos();
            (!todos.is_empty()).then_some(ProgressSource::SessionScratchpad { todos })
        };
        let plan_id = active_plan.map(|plan| plan.id);
        ControlSnapshot {
            mode,
            plan_path,
            plan_file_state,
            plan_id,
            model: model.map(str::to_string),
            progress,
        }
    }

    /// 兼容旧调用口：v4-g 起 recover 不再扫盘，仅保持默认 Chat。
    pub fn recover(&self) -> Result<(), PlanRuntimeError> {
        self.attach_from_resume_state(ResumeControlState::default())
    }

    /// 从磁盘刷新 active plan 缓存。
    ///
    /// 已绑定路径时只刷新该文件；尚未绑定时为 `/restore` 扫描当前 session 所属的 executing
    /// plan。无论哪条路径，都不会改会话模式。
    pub fn sync_active_plan_from_disk(&self) -> Result<Option<String>, PlanRuntimeError> {
        if let Some(active) = self.active_plan() {
            let Some(plan) = read_plan_for_restore(&active.path) else {
                *self.active_plan.write() = None;
                return Ok(None);
            };
            let refreshed = ActivePlan::from_file(active.path, &plan);
            let plan_id = refreshed.id.clone();
            *self.active_plan.write() = Some(refreshed);
            return Ok(Some(plan_id));
        }

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
            if matches!(plan.frontmatter.state, file_store::PlanFileState::Executing)
                && self.owns_executing_plan(&plan)
            {
                let plan_id = plan.frontmatter.plan_id.clone();
                *self.active_plan.write() = Some(ActivePlan::from_file(path, &plan));
                return Ok(Some(plan_id));
            }
        }
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

    /// After a successful plan-file write, synchronize the cache with the exact file that was
    /// committed. The caller must not invoke this before the write succeeds.
    pub(crate) fn refresh_active_plan_after_write(
        &self,
        path: PathBuf,
        plan: &file_store::PlanFile,
    ) {
        *self.active_plan.write() = Some(ActivePlan::from_file(path, plan));
    }

    #[cfg(test)]
    pub(crate) fn bind_plan_file_for_test(&self, path: PathBuf) {
        let plan = file_store::read_plan(&path).expect("test plan file must be readable");
        self.refresh_active_plan_after_write(path, &plan);
    }

    /// Unit-test-only cache fixture for tests that exercise a consumer without a plan-file write
    /// path. Production code must populate this cache through a successful plan-file read/write.
    #[cfg(test)]
    pub(crate) fn seed_active_plan_for_test(
        &self,
        plan_id: String,
        state: file_store::PlanFileState,
    ) {
        let path = file_store::plan_path_for_id(&plan_id)
            .unwrap_or_else(|_| PathBuf::from(format!("{plan_id}.plan.md")));
        *self.active_plan.write() = Some(ActivePlan {
            id: plan_id,
            path,
            state,
        });
    }

    /// 当前 active plan 的真实路径；若本 session 还未绑定任何 plan，则返回 None。
    pub fn active_plan_path(&self) -> Option<PathBuf> {
        self.active_plan().map(|plan| plan.path)
    }

    // ─── P4 reviewer 派发 API（plan-runtime.md §P4） ──────────────────────

    /// 注入 plan reviewer 派发器。
    pub fn attach_plan_reviewer(&self, dispatcher: Arc<dyn PlanReviewerDispatcher>) {
        *self.plan_reviewer.lock() = Some(dispatcher);
    }

    /// 注入 code reviewer 派发器。
    pub fn attach_code_reviewer(&self, dispatcher: Arc<dyn CodeReviewerDispatcher>) {
        *self.code_reviewer.lock() = Some(dispatcher);
    }

    /// Whether a code-review dispatcher is available for a close-out gate.
    pub fn has_code_reviewer(&self) -> bool {
        self.code_reviewer.lock().is_some()
    }

    /// 注入 verifier 派发器（生产由 `ChatContext::from_config` 装配 verifier 子 Agent 派发；
    /// 测试可注入 mock / 自定义实现）。
    pub fn attach_verifier(&self, dispatcher: Arc<dyn VerifierDispatcher>) {
        *self.verifier.lock() = Some(dispatcher);
    }

    /// 注入 explorer 派发器（`dispatch_agent` 的后端）。未注入时该工具直接报错，
    /// 不做静默降级——让模型自己去读，比返回一份空结论更诚实。
    pub fn attach_explorer(&self, dispatcher: Arc<dyn ExplorerDispatcher>) {
        *self.explorer.lock() = Some(dispatcher);
    }

    /// 并行派发一批 Explorer。任何一个失败只影响它自己那条报告，其余照常返回。
    pub async fn dispatch_explorers(
        &self,
        tasks: &[explorer::ExplorerTask],
    ) -> Result<Vec<explorer::ExplorerReport>, PlanRuntimeError> {
        let Some(dispatcher) = self.explorer.lock().clone() else {
            return Err(PlanRuntimeError::Io(
                "dispatch_agent 不可用：explorer 子 Agent 派发器未注入".into(),
            ));
        };
        Ok(
            futures_util::future::join_all(tasks.iter().map(|task| dispatcher.dispatch(task)))
                .await,
        )
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

    pub fn set_max_code_review_rounds(&self, value: u32) {
        self.max_code_review_rounds.store(value, Ordering::Release);
    }

    pub fn max_code_review_rounds(&self) -> u32 {
        self.max_code_review_rounds.load(Ordering::Acquire)
    }

    pub fn set_max_completion_gate_cycles(&self, value: u32) {
        self.max_completion_gate_cycles
            .store(value.max(1), Ordering::Release);
    }

    pub fn max_completion_gate_cycles(&self) -> u32 {
        self.max_completion_gate_cycles.load(Ordering::Acquire)
    }

    pub fn attach_workspace_root(&self, workspace_root: PathBuf) {
        *self.workspace_root.lock() = Some(workspace_root);
    }

    pub fn workspace_root(&self) -> Option<PathBuf> {
        self.workspace_root.lock().clone()
    }

    pub fn attach_bash_task_registry(
        &self,
        registry: Arc<crate::core::tools::primitive::BashTaskRegistry>,
    ) {
        *self.bash_task_registry.lock() = Some(registry);
    }

    pub fn bash_task_registry(
        &self,
    ) -> Option<Arc<crate::core::tools::primitive::BashTaskRegistry>> {
        self.bash_task_registry.lock().clone()
    }

    /// `[skills].expose_to_reviewer` 当前值：为 true 时 reviewer/verifier 可见技能目录并允许
    /// `load_skill`，否则保持默认禁用。
    pub fn expose_skills_to_reviewer(&self) -> bool {
        self.expose_skills_to_reviewer.load(Ordering::Acquire)
    }

    /// 由 `ChatContext::from_config` 在装配阶段写入。
    pub fn set_expose_skills_to_reviewer(&self, value: bool) {
        self.expose_skills_to_reviewer
            .store(value, Ordering::Release);
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
    ) -> plan_reviewer::PlanReviewSummary {
        let Some(dispatcher) = self.plan_reviewer.lock().clone() else {
            return plan_reviewer::PlanReviewSummary::placeholder_pending();
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
        //
        // 这里刻意仍走 `plan_path_for_id(plan_id)`，不复用 `resolved_plan_path()`：
        // 当前 `dispatch_reviewer()` 仅由 `create_plan` 在写盘成功后立即调用，而
        // `create_plan` 总是先把 plan 写到 canonical `~/.tomcat/plans/<plan_id>.plan.md`，
        // 再刷新 active plan 缓存。也就是说，Planning 阶段当前
        // 不存在“disk 真正路径与 plan_id 推导路径不一致”的合法场景。
        //
        // 若未来 planner 支持“从外部草稿导入后直接进入 Planning 并派发 reviewer”，
        // 这里再切到 `resolved_plan_path()`，与 code reviewer / verifier 对齐。
        let path = match file_store::plan_path_for_id(plan_id) {
            Ok(p) => p,
            Err(e) => {
                return plan_reviewer::PlanReviewSummary::aborted_with(format!(
                    "plan_id 非法: {e}"
                ));
            }
        };
        let plan_text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                return plan_reviewer::PlanReviewSummary::aborted_with(format!(
                    "read plan 失败: {e}"
                ));
            }
        };

        let mut summary = dispatcher
            .dispatch(plan_id, &plan_text, allow_review_edit)
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

    /// 同步派发 verifier 前的 code reviewer。调用方负责：
    /// 1. 先判断 / 递增 `code_review_rounds`
    /// 2. 调用 `CodeReviewSummary::normalize_for_result()`
    /// 3. 再写 transcript，保证 transcript 与 `update_plan.code_review` 口径一致
    pub async fn dispatch_code_reviewer(
        &self,
        plan_id: &str,
        dispatch: &CodeReviewDispatchInfo,
    ) -> code_reviewer::CodeReviewSummary {
        let Some(dispatcher) = self.code_reviewer.lock().clone() else {
            return code_reviewer::CodeReviewSummary::placeholder_pending();
        };
        let path = match self.resolved_plan_path(plan_id) {
            Ok(p) => p,
            Err(e) => {
                return code_reviewer::CodeReviewSummary::aborted_with(e);
            }
        };
        let plan_text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                return code_reviewer::CodeReviewSummary::aborted_with(format!(
                    "read plan 失败: {e}"
                ));
            }
        };

        // 上一轮未清 finding 一并交给 reviewer，由它逐条按当前代码语义核销；
        // 参考号仅在本轮有效，不能把措辞变化伪装成稳定跨轮 id。
        let open_findings = self.unresolved_findings(plan_id);
        dispatcher
            .dispatch(plan_id, &plan_text, &open_findings, dispatch)
            .await
    }

    /// 同步派发 verifier。语义与 reviewer 类似，但无 round 概念：
    ///
    /// 1. **必须**在 `write_plan` 释放 advisory lock **之后**调用。
    /// 2. 读取 plan 文件 → 调 dispatcher → 解析 `<verify>` block → 返回 `VerifySummary`。
    /// 3. 失败 / parse 错 / max_turns / parent abort → `verdict=aborted`；
    ///    调用方（`update_plan`）**不**因此失败，而是按 `verify_gate` 决定是否收工。
    /// 4. transcript `plan.verify` 事件由调用方在 `normalize_for_gate()` 之后统一写入，
    ///    以保证 transcript 与 `update_plan.verify` 共用同一份最终语义。
    /// 5. 若 dispatcher 未注入 → 返回 `placeholder_pending`。
    pub async fn dispatch_verifier(&self, plan_id: &str) -> verify::VerifySummary {
        let Some(dispatcher) = self.verifier.lock().clone() else {
            return verify::VerifySummary::placeholder_pending();
        };
        let path = match self.resolved_plan_path(plan_id) {
            Ok(p) => p,
            Err(e) => return verify::VerifySummary::aborted_with(e),
        };
        let plan_text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => return verify::VerifySummary::aborted_with(format!("read plan 失败: {e}")),
        };

        dispatcher.dispatch(plan_id, &plan_text).await
    }

    pub(crate) fn resolved_plan_path(&self, plan_id: &str) -> Result<PathBuf, String> {
        if let Some(active) = self.active_plan().filter(|plan| plan.id == plan_id) {
            return Ok(active.path);
        }
        file_store::plan_path_for_id(plan_id).map_err(|e| format!("plan_id 非法: {e}"))
    }

    /// 把最终版 VerifySummary 写入 transcript `plan.verify` 事件。
    ///
    /// 调用方应先完成 `normalize_for_gate()`，再调用本方法，确保 transcript 与
    /// `update_plan` tool result 共享同一份 VerifySummary。
    #[allow(dead_code)] // verifier 暂时下线，保留待重启。
    pub(crate) fn write_verify_transcript(&self, plan_id: &str, summary: &verify::VerifySummary) {
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
    }

    pub(crate) fn write_plan_review_started_transcript(
        &self,
        plan_id: &str,
        child_session_id: Option<&str>,
        transcript_path: Option<&str>,
    ) {
        let mut payload = serde_json::json!({
            "event": crate::infra::wire::WIRE_PLAN_REVIEW_STARTED,
            "plan_id": plan_id,
        });
        if let Some(obj) = payload.as_object_mut() {
            if let Some(child_session_id) = child_session_id {
                obj.insert(
                    "child_session_id".to_string(),
                    serde_json::Value::String(child_session_id.to_string()),
                );
            }
            if let Some(transcript_path) = transcript_path {
                obj.insert(
                    "transcript_path".to_string(),
                    serde_json::Value::String(transcript_path.to_string()),
                );
            }
        }
        self.write_transcript_custom(payload);
    }

    pub(crate) fn write_code_review_started_transcript(
        &self,
        plan_id: &str,
        round: u32,
        review_attempt_id: &str,
        tool_call_id: &str,
        child_session_id: Option<&str>,
        transcript_path: Option<&str>,
    ) {
        let mut payload = serde_json::json!({
            "event": crate::infra::wire::WIRE_PLAN_CODE_REVIEW_STARTED,
            "plan_id": plan_id,
            "round": round,
            "review_attempt_id": review_attempt_id,
            "tool_call_id": tool_call_id,
        });
        if let (Some(obj), Some(child_session_id)) = (payload.as_object_mut(), child_session_id) {
            obj.insert(
                "child_session_id".to_string(),
                serde_json::Value::String(child_session_id.to_string()),
            );
        }
        if let (Some(obj), Some(transcript_path)) = (payload.as_object_mut(), transcript_path) {
            obj.insert(
                "transcript_path".to_string(),
                serde_json::Value::String(transcript_path.to_string()),
            );
        }
        self.write_transcript_custom(payload);
    }

    pub(crate) fn write_explorer_started_transcript(
        &self,
        task_id: &str,
        child_session_id: Option<&str>,
        transcript_path: Option<&str>,
    ) {
        let mut payload = serde_json::json!({
            "event": crate::infra::wire::WIRE_PLAN_EXPLORER_STARTED,
            "task_id": task_id,
        });
        if let Some(obj) = payload.as_object_mut() {
            if let Some(child_session_id) = child_session_id {
                obj.insert(
                    "child_session_id".to_string(),
                    serde_json::Value::String(child_session_id.to_string()),
                );
            }
            if let Some(transcript_path) = transcript_path {
                obj.insert(
                    "transcript_path".to_string(),
                    serde_json::Value::String(transcript_path.to_string()),
                );
            }
        }
        self.write_transcript_custom(payload);
    }

    pub(crate) fn write_code_review_transcript(
        &self,
        plan_id: &str,
        summary: &code_reviewer::CodeReviewSummary,
        round: u32,
        review_attempt_id: &str,
        tool_call_id: &str,
    ) {
        let mut payload = summary.to_json();
        if let Some(obj) = payload.as_object_mut() {
            obj.insert(
                "event".to_string(),
                serde_json::Value::String(crate::infra::wire::WIRE_PLAN_CODE_REVIEW.to_string()),
            );
            obj.insert(
                "plan_id".to_string(),
                serde_json::Value::String(plan_id.to_string()),
            );
            obj.insert(
                "rounds".to_string(),
                serde_json::Value::Number(serde_json::Number::from(round)),
            );
            obj.insert(
                "round".to_string(),
                serde_json::Value::Number(serde_json::Number::from(round)),
            );
            obj.insert(
                "review_attempt_id".to_string(),
                serde_json::Value::String(review_attempt_id.to_string()),
            );
            obj.insert(
                "tool_call_id".to_string(),
                serde_json::Value::String(tool_call_id.to_string()),
            );
            if !summary.child_session_id.is_empty() {
                obj.insert(
                    "child_session_id".to_string(),
                    serde_json::Value::String(summary.child_session_id.clone()),
                );
            }
        }
        self.write_transcript_custom(payload);
    }

    /// 代码评审预算用尽仍有未清 finding：记录残余，随后由 acceptance 的绿构建证据收口。
    pub(crate) fn write_code_review_exhausted_transcript(
        &self,
        plan_id: &str,
        rounds: u32,
        residual_findings: &[String],
    ) {
        self.write_transcript_custom(serde_json::json!({
            "event": crate::infra::wire::WIRE_PLAN_CODE_REVIEW_EXHAUSTED,
            "plan_id": plan_id,
            "rounds": rounds,
            "max_code_review_rounds": self.max_code_review_rounds(),
            "outcome": "passed_to_acceptance_with_residual_findings",
            "code_review_pass": true,
            "residual_findings": residual_findings,
        }));
    }

    /// 代码评审预算已用尽后，验收期间又修改了代码。
    ///
    /// 预算决定不再重开 reviewer；此事件保留那批未复审代码的可审计痕迹，避免把
    /// 「没有新的 review」伪装成「没有新的改动」。
    pub(crate) fn write_code_review_unreviewed_edit_transcript(
        &self,
        plan_id: &str,
        rounds: u32,
        changed_code_files: &[String],
        newest_edit_mtime_ms: u128,
    ) {
        self.write_transcript_custom(serde_json::json!({
            "event": crate::infra::wire::WIRE_PLAN_CODE_REVIEW_UNREVIEWED_EDIT,
            "plan_id": plan_id,
            "rounds": rounds,
            "max_code_review_rounds": self.max_code_review_rounds(),
            "changed_code_files": changed_code_files,
            "newest_edit_mtime_ms": newest_edit_mtime_ms,
        }));
    }

    /// Record provider usage for the active build only. Calls made in normal chat or planning
    /// mode are deliberately excluded, so plan completion reports are not polluted by earlier
    /// conversation cache traffic.
    pub(crate) fn record_plan_cache_usage(
        &self,
        prompt_tokens: u32,
        cache_read_tokens: Option<u32>,
        tail_changed: bool,
    ) {
        let Some(plan_id) = self.executing_plan_id() else {
            return;
        };
        self.plan_cache_observations
            .lock()
            .entry(plan_id)
            .or_default()
            .observe(prompt_tokens, cache_read_tokens, tail_changed);
    }

    /// Returns a JSON-ready snapshot so `plan.complete` can be an auditable build outcome even
    /// though it is emitted from the plan tool rather than the agent loop.
    pub(crate) fn plan_cache_observation_json(&self, plan_id: &str) -> Option<serde_json::Value> {
        self.plan_cache_observations
            .lock()
            .get(plan_id)
            .map(PlanCacheObservation::to_json)
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

    pub fn try_begin_code_review_round(&self, plan_id: &str) -> Option<u32> {
        let max_rounds = self.max_code_review_rounds();
        let mut rounds = self.code_review_rounds.lock();
        let current = rounds.get(plan_id).copied().unwrap_or(0);
        if current >= max_rounds {
            return None;
        }
        let next = current + 1;
        rounds.insert(plan_id.to_string(), next);
        Some(next)
    }

    pub fn reset_code_review_rounds(&self, plan_id: &str) {
        self.code_review_rounds.lock().remove(plan_id);
        self.last_code_review_dispatch_ms.lock().remove(plan_id);
        self.unresolved_findings.lock().remove(plan_id);
        self.disputed_findings.lock().remove(plan_id);
        self.review_infra_retries.lock().remove(plan_id);
    }

    /// 记录本轮 code review 之后仍未清掉的 finding（`pass` 时传空 Vec 即可清空）。
    pub fn set_unresolved_findings(&self, plan_id: &str, findings: Vec<review::Finding>) {
        let mut guard = self.unresolved_findings.lock();
        if findings.is_empty() {
            guard.remove(plan_id);
        } else {
            guard.insert(plan_id.to_string(), findings);
        }
    }

    pub fn unresolved_findings(&self, plan_id: &str) -> Vec<review::Finding> {
        self.unresolved_findings
            .lock()
            .get(plan_id)
            .cloned()
            .unwrap_or_default()
    }

    pub(crate) fn last_code_review_dispatch_ms(&self, plan_id: &str) -> Option<u128> {
        self.last_code_review_dispatch_ms
            .lock()
            .get(plan_id)
            .copied()
    }

    pub(crate) fn set_last_code_review_dispatch_ms(&self, plan_id: &str, timestamp_ms: u128) {
        self.last_code_review_dispatch_ms
            .lock()
            .insert(plan_id.to_string(), timestamp_ms);
    }

    pub fn unresolved_finding_references(&self, plan_id: &str) -> Vec<String> {
        self.unresolved_findings
            .lock()
            .get(plan_id)
            .map(|findings| {
                findings
                    .iter()
                    .map(|finding| finding.reference.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn code_review_rounds(&self, plan_id: &str) -> u32 {
        self.code_review_rounds
            .lock()
            .get(plan_id)
            .copied()
            .unwrap_or(0)
    }

    /// 返回计划的 code review 轮次是否已经用尽。
    ///
    /// 仅在至少派发过一次 review 时才视为「耗尽」；`max=0` 表示调用方关闭了
    /// review 门禁，而不是「review 失败后耗尽」。这样 completion guard 不会把
    /// 未启用门禁的计划误当作 handoff。
    pub fn code_review_budget_exhausted(&self, plan_id: &str) -> bool {
        let rounds = self.code_review_rounds(plan_id);
        rounds > 0 && rounds >= self.max_code_review_rounds()
    }

    /// 技术故障刚发生时退还本轮正常 review 配额。
    pub fn refund_code_review_round(&self, plan_id: &str) {
        if let Some(rounds) = self.code_review_rounds.lock().get_mut(plan_id) {
            *rounds = rounds.saturating_sub(1);
        }
    }

    /// 增加技术故障重试次数，返回增加后的次数。
    pub fn bump_review_infra_retry(&self, plan_id: &str) -> u32 {
        let mut retries = self.review_infra_retries.lock();
        let retry = retries.entry(plan_id.to_owned()).or_insert(0);
        *retry += 1;
        *retry
    }

    pub fn review_infra_retries(&self, plan_id: &str) -> u32 {
        self.review_infra_retries
            .lock()
            .get(plan_id)
            .copied()
            .unwrap_or(0)
    }

    pub fn code_review_infra_retry_exhausted(&self, plan_id: &str) -> bool {
        self.review_infra_retries(plan_id) > 2
    }

    pub fn add_disputed_finding(&self, plan_id: &str, finding: review::Finding, reason: String) {
        let reference = finding.reference.clone();
        let disputed = DisputedFinding {
            reference,
            severity: finding.severity,
            area: finding.area,
            note: finding.note,
            resolution: "wontfix".into(),
            reason,
        };
        self.write_transcript_custom(serde_json::json!({
            "event": "plan.code_review.dispute",
            "plan_id": plan_id,
            "finding": disputed,
        }));
        self.disputed_findings
            .lock()
            .entry(plan_id.to_owned())
            .or_default()
            .push(disputed);
        if let Some(open) = self.unresolved_findings.lock().get_mut(plan_id) {
            open.retain(|item| item.reference != finding.reference);
        }
    }

    pub fn disputed_findings(&self, plan_id: &str) -> Vec<DisputedFinding> {
        self.disputed_findings
            .lock()
            .get(plan_id)
            .cloned()
            .unwrap_or_default()
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
    ) -> Result<(PathBuf, Option<String>), PlanRuntimeError> {
        if !Self::looks_like_plan_path(plan_id_or_path) {
            safety::assert_plan_id_safe(plan_id_or_path)?;
            let path = file_store::plan_path_for_id(plan_id_or_path)
                .map_err(|e| PlanRuntimeError::Io(e.to_string()))?;
            if !path.is_file() {
                return Err(PlanRuntimeError::BuildPlanNotFound {
                    plan_id: plan_id_or_path.to_string(),
                    hint: format!(
                        "未找到 ~/.tomcat/plans/{plan_id_or_path}.plan.md；先通过 PLAN 模式 create_plan 生成"
                    ),
                });
            }
            return Ok((path, Some(plan_id_or_path.to_string())));
        }

        let path = crate::infra::platform::normalize_path(plan_id_or_path)
            .map_err(|e| PlanRuntimeError::Io(e.to_string()))?;
        if !path.is_file() {
            return Err(PlanRuntimeError::BuildPlanPathNotFound {
                path: crate::infra::platform::format_home_path(&path),
                hint: "检查 plan path 是否正确，或改用 /plan build <plan_id/path>".into(),
            });
        }
        Ok((path, None))
    }

    /// `/plan build` 不带参数时的默认目标：当前绑定计划（planning 或 pending）。
    pub fn default_build_target(&self) -> Result<String, PlanRuntimeError> {
        if let Some(plan) = self.active_plan().filter(|plan| {
            matches!(
                plan.state,
                file_store::PlanFileState::Planning | file_store::PlanFileState::Pending
            )
        }) {
            return Ok(plan.id);
        }
        Err(PlanRuntimeError::BuildBlocked(
            "`/plan build` 需要 plan_id 或 path".into(),
        ))
    }

    /// `/plan build <plan_id/path>` 入口；执行 plan-runtime §5.1 的 5 件事 + 原子回滚。
    ///
    /// **闸门**（任一不通过 → `BuildBlocked`）：
    /// - 当前内存 mode 不能是 `Executing`；`Chat` / `Planning` / `Pending` / `Completed`
    ///   允许继续检查目标盘
    /// - 当前 session 的 active scratchpad todos（`session_todos` 中 pending/in_progress）
    ///   仅 warning，不阻塞 build
    /// - `/plan build` 无参时仍由 `default_build_target()` 优先命中当前 `Pending { id }`；
    ///   显式 target 则可切到另一份 `planning/pending` plan
    /// - 目标 PlanFile 必须存在（不存在 → `BuildPlanNotFound` / `BuildPlanPathNotFound`，附友好提示）
    /// - PlanFile.frontmatter.state ∈ `{planning, pending}`（executing/completed 拒）
    ///
    /// **5 件事**：
    /// 1. 改 frontmatter.session_key = `self.session_key`；session_id = `session_id`
    ///    （pending 续跑时若 `prev_session_key != self.session_key` → push warning，仍执行）
    /// 2. 改 frontmatter.state = `executing`
    /// 3. `write_plan`（atomic + advisory lock）；**失败时内存不动**，返回 PlanFile error
    /// 4. 写盘成功后刷新 active plan 缓存，并在必要时从 Plan 会话模式回到 Chat
    /// 5. 更新 `active_plan_path`，供后续 `/plan build` 自动开跑时生成真实 user turn 文本
    ///
    /// **原子性**：盘 write 失败 → 内存不变；盘 write 成功后才动内存——
    /// 配合 advisory lock 保证 PlanFile 不会出现"executing 但内存仍 Chat"的半态。
    /// （注：写盘 OK 但内存切换前 panic 这条很窄的窗口由 D7 recover 兜底）。
    pub fn build_plan(
        &self,
        plan_id_or_path: &str,
        session_id: Option<String>,
    ) -> Result<BuildPlanOutcome, PlanRuntimeError> {
        let (path, requested_plan_id) = self.resolve_build_target(plan_id_or_path)?;
        // ─── 预检：active scratchpad todos（仅 warning，不阻塞 build） ───────
        let has_active_session_todos = {
            let session_todos = self.session_todos.lock();
            session_todos.iter().any(|t| {
                matches!(
                    t.status,
                    file_store::TodoStatus::Pending | file_store::TodoStatus::InProgress
                )
            })
        };

        struct BuildCommit {
            plan_id: String,
            prev_disk_state: file_store::PlanFileState,
            warnings: Vec<String>,
        }

        let build = match file_store::update_plan_locked(&path, self.lock_timeout_ms, |plan| {
            safety::assert_plan_id_safe(&plan.frontmatter.plan_id)
                .map_err(|e| PlanRuntimeError::Io(e.to_string()))?;
            let plan_id = plan.frontmatter.plan_id.clone();

            // ─── 闸门 1：同一 session 不得同时执行两份计划 ─────────────
            if let Some(cur) = self.executing_plan_id() {
                return Err(PlanRuntimeError::BuildBlocked(format!(
                    "当前 session 已在执行计划（plan_id={cur}）；先中断使其变为 pending"
                )));
            }

            // ─── 读 PlanFile + 闸门 4/5：存在 + 合法 state ────────────────
            let prev_disk_state = plan.frontmatter.state;
            match prev_disk_state {
                file_store::PlanFileState::Planning | file_store::PlanFileState::Pending => {}
                file_store::PlanFileState::Executing => {
                    return Err(PlanRuntimeError::BuildBlocked(format!(
                        "PlanFile {plan_id} state=executing；可能被其它进程占用，请稍后或手工修复"
                    )));
                }
                file_store::PlanFileState::Completed => {
                    return Err(PlanRuntimeError::BuildBlocked(format!(
                        "PlanFile {plan_id} state=completed；已完成的 plan 不可再 build"
                    )));
                }
            }

            // ─── 准备五件事 ────────────────────────────────────────────
            let mut warnings: Vec<String> = Vec::new();
            if matches!(prev_disk_state, file_store::PlanFileState::Pending) {
                if let Some(prev_key) = &plan.frontmatter.session_key {
                    if prev_key != self.session_key.as_str() {
                        warnings.push(format!(
                            "pending plan {plan_id} 原绑定 session_key={prev_key}；本次将覆盖为 {}",
                            self.session_key
                        ));
                    }
                }
            }
            // 1, 2: frontmatter 改 session_key/session_id/state
            plan.frontmatter.session_key = Some(self.session_key.clone());
            plan.frontmatter.session_id = session_id.clone();
            plan.frontmatter.state = file_store::PlanFileState::Executing;
            Ok(BuildCommit {
                plan_id,
                prev_disk_state,
                warnings,
            })
        }) {
            Ok(v) => v,
            Err(file_store::LockedPlanMutationError::Plan(file_store::PlanError::NotFound {
                ..
            })) => {
                return match requested_plan_id {
                    Some(plan_id) => Err(PlanRuntimeError::BuildPlanNotFound {
                        plan_id: plan_id.clone(),
                        hint: format!(
                            "未找到 ~/.tomcat/plans/{plan_id}.plan.md；先通过 PLAN 模式 create_plan 生成"
                        ),
                    }),
                    None => Err(PlanRuntimeError::BuildPlanPathNotFound {
                        path: crate::infra::platform::format_home_path(&path),
                        hint: "检查 plan path 是否正确，或改用 /plan build <plan_id/path>".into(),
                    }),
                };
            }
            Err(file_store::LockedPlanMutationError::Plan(e)) => {
                return Err(PlanRuntimeError::from_plan_io(e));
            }
            Err(file_store::LockedPlanMutationError::Callback(e)) => return Err(e),
        };

        let plan_id = build.plan_id.clone();
        let mut warnings = build.warnings;
        if has_active_session_todos {
            warnings.push(
                "当前 session 仍有未完成 scratchpad todos；本次继续 build，不影响目标 PlanFile，建议稍后收口"
                    .into(),
            );
        }
        let prev_disk_state = build.prev_disk_state;
        // 4: 写盘成功后刷新 active-plan 缓存；Build 离开 Plan 会话模式。
        *self.active_plan.write() = Some(ActivePlan {
            id: plan_id.clone(),
            path: path.clone(),
            state: file_store::PlanFileState::Executing,
        });
        // 一次 build 就是一次交付尝试，code review 轮数预算按次发放而不是按计划终身发放。
        // 少了这一步，同一进程里二次 build 同一个计划会因为计数器没清而直接跳过 review。
        self.reset_code_review_rounds(&plan_id);
        self.plan_cache_observations.lock().remove(&plan_id);

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

        let event_payload = crate::infra::events::PlanEventPayload {
            plan_id: plan_id.clone(),
            path: crate::infra::platform::format_home_path(&path),
            state: file_store::PlanFileState::Executing.as_str().to_string(),
        };
        self.write_transcript_custom(serde_json::json!({
            "event": crate::infra::wire::WIRE_PLAN_BUILD,
            "plan_id": event_payload.plan_id,
            "path": event_payload.path,
            "state": event_payload.state,
        }));
        self.leave_plan_for_build()?;

        Ok(BuildPlanOutcome {
            plan_id: plan_id.to_string(),
            plan_path: path,
            prev_disk_state,
            warnings,
        })
    }

    // ─── P7 PR-PLF cancel→pending + 释放锁（plan-runtime.md §5.6） ───────

    /// 当用户取消当前执行回合时调；只在 active plan 为 executing 时生效。
    ///
    /// **副作用**（事务序）：
    /// 1. 读当前 plan 文件
    /// 2. 写 frontmatter.state = pending（atomic + advisory lock；写完即释放，防 D1）
    /// 3. 刷新 active-plan 缓存为 pending
    /// 4. 返回 plan_id 给上层做 transcript `plan.cancel.demote_to_pending`
    ///
    /// **幂等**：没有 executing plan 时直接返回 Ok(None)。
    /// **错误**：磁盘读/写失败不修改缓存，返回 `Io`；上层应仅 warning（D8）。
    pub fn park_executing_plan(&self) -> Result<Option<String>, PlanRuntimeError> {
        let active = match self.active_plan().filter(ActivePlan::is_executing) {
            Some(active) => active,
            None => return Ok(None),
        };
        let plan_id = active.id;
        // ② 改写磁盘
        let path = active.path;
        file_store::update_plan_locked(&path, self.lock_timeout_ms, |plan| {
            plan.frontmatter.state = file_store::PlanFileState::Pending;
            Ok::<(), PlanRuntimeError>(())
        })
        .map_err(|e| match e {
            file_store::LockedPlanMutationError::Plan(err) => PlanRuntimeError::from_plan_io(err),
            file_store::LockedPlanMutationError::Callback(err) => err,
        })?;
        // ③ 刷新缓存并记录纯计划文件事件（会话模式从头到尾仍是 Chat）。
        *self.active_plan.write() = Some(ActivePlan {
            id: plan_id.clone(),
            path: path.clone(),
            state: file_store::PlanFileState::Pending,
        });
        self.emit_plan_state_event(
            crate::infra::wire::WIRE_PLAN_PENDING,
            file_store::PlanFileState::Pending.as_str(),
            Some(&plan_id),
            Some(path),
        );
        Ok(Some(plan_id))
    }

    /// 挂接当前回合的 cancel_token；chat_loop 每轮 readline 后必须调（D2 防御）。
    ///
    /// 该 API 仅保存 token；真正的 cancel→pending 由 chat_loop 在 `select! cancel_token.cancelled()`
    /// 分支显式调 `park_executing_plan()` 触发——避免后台 spawn task 持 Arc<Self>
    /// 导致 PlanRuntime 生命周期跨 turn 泄漏。
    pub fn attach_cancel_hook(&self, token: CancellationToken) {
        *self.cancel_token.lock() = Some(token);
    }

    /// 当前回合的 cancel_token（克隆）。chat_loop 可以从这里取出，与新建的 token 比对，
    /// 决定是否需要重挂（D2：每轮 readline 后必须重挂，否则上一轮 hook 失效）。
    pub fn current_cancel_token(&self) -> Option<CancellationToken> {
        self.cancel_token.lock().clone()
    }

    // ─── P7 PR-PLF raw edit 拦截（plan-runtime.md §5.6） ─────────────────

    /// Plan 会话模式或存在执行中计划时，`tool_exec::write`/`edit` 等 raw 写入路径调用此 helper
    /// 判断该路径是否允许写入。
    ///
    /// **规则**：
    /// - 不是 `~/.tomcat/plans/*.plan.md` → 允许（其他文件不归本 runtime 管）
    /// - 是 `~/.tomcat/plans/*.plan.md`：
    ///   - Chat 模式且没有 pending / executing active plan → 允许
    ///   - Plan 模式、或计划处于 pending / executing → 拒
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
        self.mode() == AgentMode::Chat
            && !matches!(
                self.active_plan().map(|plan| plan.state),
                Some(file_store::PlanFileState::Executing | file_store::PlanFileState::Pending)
            )
    }
}

/// `/plan build` 成功返回。
#[derive(Debug, Clone)]
pub struct BuildPlanOutcome {
    pub plan_id: String,
    pub plan_path: PathBuf,
    /// 目标 PlanFile 的写前 state（planning / pending）；命令层据此打印不同提示。
    pub prev_disk_state: file_store::PlanFileState,
    /// 非致命警告（如 pending 续跑 session_key 不一致）。
    pub warnings: Vec<String>,
}

/// plan reviewer 子 Agent 派发器 trait。
#[async_trait]
pub trait PlanReviewerDispatcher: Send + Sync {
    async fn dispatch(
        &self,
        plan_id: &str,
        plan_text: &str,
        allow_review_edit: bool,
    ) -> plan_reviewer::PlanReviewSummary;
}

/// code reviewer 子 Agent 派发器 trait。
#[async_trait]
pub trait ExplorerDispatcher: Send + Sync {
    /// 同步跑完一个只读勘察子 Agent 并回传结论。失败一律以 `aborted` 报告表达，
    /// 不返回 Err——一个勘察任务失败不该让整批调用失败。
    async fn dispatch(&self, task: &explorer::ExplorerTask) -> explorer::ExplorerReport;
}

#[derive(Debug, Clone)]
pub struct CodeReviewDispatchInfo {
    pub round: u32,
    pub review_attempt_id: String,
    pub tool_call_id: String,
}

#[async_trait::async_trait]
pub trait CodeReviewerDispatcher: Send + Sync {
    /// `open_findings` 是上一轮未清的 finding；实现应把它们渲染进 prompt，
    /// 让 reviewer 按语义核销而不是重新发明问题；参考号仅在本轮输出中有效。
    async fn dispatch(
        &self,
        plan_id: &str,
        plan_text: &str,
        open_findings: &[review::Finding],
        dispatch: &CodeReviewDispatchInfo,
    ) -> code_reviewer::CodeReviewSummary;
}

/// verifier 子 Agent 派发器 trait（解耦真实 LLM + AgentRegistry）。
///
/// **契约**：
/// - 调用方（`PlanRuntime::dispatch_verifier`）保证：调度时 plan 文件 advisory lock 已 release。
/// - dispatch 内部应通过 [`crate::core::agent_registry::AgentRegistry::spawn_subagent_internal`]
///   构造子 `AgentLoop`，并把 `SubagentSpawnContext.cancel_token` 直接透传给 `AgentLoop::new`。
/// - 返回 `VerifySummary`：成功 / aborted / parse_failed 都用同一形态承载。
/// - **不**写父 transcript（verifier 子 Agent 持独立 session_id；transcript 隔离）。
#[async_trait]
pub trait VerifierDispatcher: Send + Sync {
    async fn dispatch(&self, plan_id: &str, plan_text: &str) -> verify::VerifySummary;
}

/// `PlanRuntime` 操作错误。
#[derive(Debug, thiserror::Error)]
pub enum PlanRuntimeError {
    #[error("当前已经在 {0} 模式，无法重复进入")]
    AlreadyInMode(String),
    #[error("plan_id 非法或不安全：{0}")]
    UnsafePlanId(String),
    /// PlanFile 文件 IO / serde 错误（P2 起细化）。
    #[error("plan io: {0}")]
    Io(String),
    /// `/plan build` 闸门未通过（运行态冲突 / disk mode 不合规等）。
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
