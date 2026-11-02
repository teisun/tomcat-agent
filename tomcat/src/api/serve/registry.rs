//! `serve` 层的会话注册表。
//!
//! 负责把 `sessionId` 映射到运行时会话壳 `SessionSlot`，与
//! `core::agent_registry::AgentRegistry` 的“Agent 实例登记”分工正交。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;
use std::collections::HashMap;
use tokio::task::JoinHandle;

use crate::core::llm::SystemPromptSnapshot;
use crate::infra::event_bus::EventListenerId;
use crate::{api::chat::ChatContext, AppError, ContextState, SessionMode};

/// 单会话 turn 之间需要延续的上下文快照。
pub struct SessionTurnState {
    pub context_state: ContextState,
    pub prompt_snapshot: SystemPromptSnapshot,
    pub context_budget_chars: usize,
}

/// `serve` 层维护的单个会话槽位。
pub struct SessionSlot {
    pub session_id: String,
    pub ctx: Arc<ChatContext>,
    pub mode: SessionMode,
    pub cwd: Option<String>,
    /// Immutable header metadata. The transcript remains the durable source of truth.
    pub origin: Option<crate::core::session::SessionOrigin>,
    pub busy: AtomicBool,
    /// The sole writer identity; the runtime's admission lock serializes transfers.
    pub(super) controller: Mutex<Option<uuid::Uuid>>,
    /// Serializes current-message replay, fanout and changes of subscribers.
    pub(super) message_replay: Mutex<super::event_pump::MessageReplay>,
    pub(super) last_used: Mutex<std::time::Instant>,
    /// Admission is busy during maintenance, but no agent turn is running.
    pub(super) command_job: AtomicBool,
    /// A restart recovered an unanswered ask_question before the frontend
    /// completed the initialize handshake. It must be re-armed only after the
    /// client is ready to receive the new response route.
    pub resume_pending_ask_question: AtomicBool,
    pub terminal_emitted: AtomicBool,
    pub turn_state: Mutex<Option<SessionTurnState>>,
    /// 最近一次向该会话前端广播的可信水位（实测定基后的后续估算也会刷新它）。
    ///
    /// 它刻意不放进 `turn_state`：运行中的 turn 会临时取走后者，重连时仍须回放
    /// 已广播的水位。`None` 表示本进程加载该会话后尚未收到 provider 的 usage，
    /// 以避免把冷启动的 fallback 估算显示成实测值。
    pub last_context_ratio: Mutex<Option<f64>>,
    pub run_task: Mutex<Option<JoinHandle<()>>>,
    pub background_task_listener: Mutex<Option<JoinHandle<()>>>,
    pub listener_ids: Mutex<Vec<EventListenerId>>,
}

impl SessionSlot {
    pub fn new(
        session_id: String,
        ctx: Arc<ChatContext>,
        mode: SessionMode,
        cwd: Option<String>,
        turn_state: SessionTurnState,
    ) -> Self {
        Self {
            session_id,
            ctx,
            mode,
            cwd,
            origin: None,
            busy: AtomicBool::new(false),
            controller: Mutex::new(None),
            message_replay: Mutex::new(Default::default()),
            last_used: Mutex::new(std::time::Instant::now()),
            command_job: AtomicBool::new(false),
            resume_pending_ask_question: AtomicBool::new(false),
            terminal_emitted: AtomicBool::new(false),
            turn_state: Mutex::new(Some(turn_state)),
            last_context_ratio: Mutex::new(None),
            run_task: Mutex::new(None),
            background_task_listener: Mutex::new(None),
            listener_ids: Mutex::new(Vec::new()),
        }
    }

    pub fn is_command_job_running(&self) -> bool {
        self.command_job.load(Ordering::SeqCst)
    }

    pub fn is_turn_running(&self) -> bool {
        self.is_busy() && !self.is_command_job_running()
    }

    pub fn is_busy(&self) -> bool {
        self.busy.load(Ordering::SeqCst)
    }

    pub fn mark_busy(&self) -> bool {
        self.busy
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    pub fn mark_idle(&self) {
        self.touch();
        self.busy.store(false, Ordering::SeqCst);
    }

    pub(super) fn touch(&self) {
        *self.last_used.lock() = std::time::Instant::now();
    }

    pub fn is_interrupted(&self) -> bool {
        self.ctx.session_runtime.cancel_token.lock().is_cancelled()
    }

    pub fn reset_terminal_emitted(&self) {
        self.terminal_emitted.store(false, Ordering::SeqCst);
    }

    pub fn mark_terminal_emitted(&self) {
        self.terminal_emitted.store(true, Ordering::SeqCst);
    }

    pub fn mark_terminal_emitted_if_absent(&self) -> bool {
        self.terminal_emitted
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }
}

/// `list_sessions` 返回的最小会话摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSummary {
    pub session_id: String,
    pub busy: bool,
    pub interrupted: bool,
}

/// Trusted routing context outlives the final live runtime; it never retains a runtime or lock.
#[derive(Clone)]
pub(super) struct SessionScope {
    pub key: String,
    pub mode: SessionMode,
    pub cwd: Option<String>,
}

impl SessionScope {
    pub(super) fn from_slot(slot: &SessionSlot) -> Self {
        Self {
            key: slot
                .ctx
                .session_runtime
                .session
                .current_session_key()
                .to_owned(),
            mode: slot.mode,
            cwd: slot.cwd.clone(),
        }
    }
}

#[derive(Default)]
struct Slots {
    by_id: HashMap<String, Arc<SessionSlot>>,
    order: Vec<String>,
}

/// Service-wide identity and capacity; a connection's selected session lives on the connection.
pub struct ChatContextRegistry {
    slots: Mutex<Slots>,
    max_sessions: usize,
}

impl ChatContextRegistry {
    pub fn new(max_sessions: usize) -> Self {
        Self {
            slots: Mutex::new(Slots::default()),
            max_sessions,
        }
    }

    pub fn len(&self) -> usize {
        self.slots.lock().by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn max_sessions(&self) -> usize {
        self.max_sessions
    }

    pub fn insert(&self, slot: Arc<SessionSlot>) -> Result<(), AppError> {
        let mut slots = self.slots.lock();
        if slots.by_id.contains_key(&slot.session_id) {
            return Err(AppError::Config("session_in_use".into()));
        }
        if slots.by_id.len() >= self.max_sessions {
            return Err(AppError::Config("too_many_sessions".into()));
        }
        slots.order.push(slot.session_id.clone());
        slots.by_id.insert(slot.session_id.clone(), slot);
        Ok(())
    }

    pub fn get(&self, session_id: &str) -> Option<Arc<SessionSlot>> {
        self.slots.lock().by_id.get(session_id).cloned()
    }

    pub fn remove(&self, session_id: &str) -> Option<Arc<SessionSlot>> {
        let mut slots = self.slots.lock();
        let removed = slots.by_id.remove(session_id)?;
        slots.order.retain(|id| id != session_id);
        Some(removed)
    }

    pub(super) fn slots(&self) -> Vec<Arc<SessionSlot>> {
        self.slots.lock().by_id.values().cloned().collect()
    }

    pub fn list(&self) -> Vec<SessionSummary> {
        let slots = self.slots.lock();
        slots
            .order
            .iter()
            .filter_map(|id| {
                slots.by_id.get(id).map(|slot| SessionSummary {
                    session_id: id.clone(),
                    busy: slot.is_turn_running(),
                    interrupted: slot.is_interrupted(),
                })
            })
            .collect()
    }
}
