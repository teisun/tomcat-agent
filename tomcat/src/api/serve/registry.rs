//! `serve` 层的会话注册表。
//!
//! 负责把 `sessionId` 映射到运行时会话壳 `SessionSlot`，与
//! `core::agent_registry::AgentRegistry` 的“Agent 实例登记”分工正交。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use dashmap::DashMap;
use parking_lot::{Mutex, RwLock};
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
    pub busy: AtomicBool,
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
            busy: AtomicBool::new(false),
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
        self.busy.store(false, Ordering::SeqCst);
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
    fn from_slot(slot: &SessionSlot) -> Self {
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

/// 进程内 `sessionId -> SessionSlot` 的注册表。
pub struct ChatContextRegistry {
    slots: DashMap<String, Arc<SessionSlot>>,
    order: Mutex<Vec<String>>,
    active_session_id: RwLock<Option<String>>,
    last_closed_scope: RwLock<Option<SessionScope>>,
    max_sessions: usize,
}

impl ChatContextRegistry {
    pub fn new(max_sessions: usize) -> Self {
        Self {
            slots: DashMap::new(),
            order: Mutex::new(Vec::new()),
            active_session_id: RwLock::new(None),
            last_closed_scope: RwLock::new(None),
            max_sessions,
        }
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    pub fn max_sessions(&self) -> usize {
        self.max_sessions
    }

    pub fn insert(&self, slot: Arc<SessionSlot>) -> Result<(), AppError> {
        if self.len() >= self.max_sessions {
            return Err(AppError::Config("too_many_sessions".to_string()));
        }
        let session_id = slot.session_id.clone();
        self.slots.insert(session_id.clone(), slot);
        self.order.lock().push(session_id.clone());
        if self.active_session_id.read().is_none() {
            *self.active_session_id.write() = Some(session_id);
        }
        Ok(())
    }

    pub fn get(&self, session_id: &str) -> Option<Arc<SessionSlot>> {
        self.slots
            .get(session_id)
            .map(|slot| Arc::clone(slot.value()))
    }

    pub fn resolve_session_id(&self, requested: Option<&str>) -> Result<String, AppError> {
        if let Some(requested) = requested {
            if self.slots.contains_key(requested) {
                return Ok(requested.to_string());
            }
            return Err(AppError::Config("unknown_session".to_string()));
        }
        self.active_session_id
            .read()
            .clone()
            .ok_or_else(|| AppError::Config("unknown_session".to_string()))
    }

    pub fn active_session_id(&self) -> Option<String> {
        self.active_session_id.read().clone()
    }

    pub(super) fn current_scope(&self) -> Option<SessionScope> {
        let active = self.active_session_id.read();
        active
            .as_deref()
            .and_then(|id| self.get(id))
            .map(|slot| SessionScope::from_slot(&slot))
            .or_else(|| self.last_closed_scope.read().clone())
    }

    pub fn set_active_session(&self, session_id: &str) -> Result<(), AppError> {
        if !self.slots.contains_key(session_id) {
            return Err(AppError::Config("unknown_session".to_string()));
        }
        *self.active_session_id.write() = Some(session_id.to_string());
        Ok(())
    }

    pub fn remove(&self, session_id: &str) -> Option<Arc<SessionSlot>> {
        let mut active = self.active_session_id.write();
        let removed = self.slots.remove(session_id).map(|(_, slot)| slot);
        if removed.is_some() {
            self.order.lock().retain(|existing| existing != session_id);
            if active.as_deref() == Some(session_id) {
                *active = self.order.lock().first().cloned();
                if active.is_none() {
                    *self.last_closed_scope.write() =
                        removed.as_ref().map(|slot| SessionScope::from_slot(slot));
                }
            }
        }
        removed
    }

    pub fn list(&self) -> Vec<SessionSummary> {
        let order = self.order.lock().clone();
        order
            .into_iter()
            .filter_map(|session_id| {
                self.get(&session_id).map(|slot| SessionSummary {
                    session_id,
                    busy: slot.is_turn_running(),
                    interrupted: slot.is_interrupted(),
                })
            })
            .collect()
    }
}
