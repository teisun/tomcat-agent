//! `serve` 层的会话注册表。
//!
//! 负责把 `sessionId` 映射到运行时会话壳 `SessionSlot`，与
//! `core::agent_registry::AgentRegistry` 的“Agent 实例登记”分工正交。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use dashmap::DashMap;
use parking_lot::{Mutex, RwLock};
use tokio::task::JoinHandle;

use crate::infra::event_bus::EventListenerId;
use crate::{api::chat::ChatContext, AppError, ContextState, SessionMode};

/// 单会话 turn 之间需要延续的上下文快照。
pub struct SessionTurnState {
    pub context_state: ContextState,
    pub system_text: String,
    pub context_budget_chars: usize,
}

/// `serve` 层维护的单个会话槽位。
pub struct SessionSlot {
    pub session_id: String,
    pub ctx: Arc<ChatContext>,
    pub mode: SessionMode,
    pub cwd: Option<String>,
    pub busy: AtomicBool,
    pub turn_state: Mutex<Option<SessionTurnState>>,
    pub run_task: Mutex<Option<JoinHandle<()>>>,
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
            turn_state: Mutex::new(Some(turn_state)),
            run_task: Mutex::new(None),
            listener_ids: Mutex::new(Vec::new()),
        }
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
}

/// `list_sessions` 返回的最小会话摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSummary {
    pub session_id: String,
    pub busy: bool,
}

/// 进程内 `sessionId -> SessionSlot` 的注册表。
pub struct ChatContextRegistry {
    slots: DashMap<String, Arc<SessionSlot>>,
    order: Mutex<Vec<String>>,
    active_session_id: RwLock<Option<String>>,
    max_sessions: usize,
}

impl ChatContextRegistry {
    pub fn new(max_sessions: usize) -> Self {
        Self {
            slots: DashMap::new(),
            order: Mutex::new(Vec::new()),
            active_session_id: RwLock::new(None),
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

    pub fn set_active_session(&self, session_id: &str) -> Result<(), AppError> {
        if !self.slots.contains_key(session_id) {
            return Err(AppError::Config("unknown_session".to_string()));
        }
        *self.active_session_id.write() = Some(session_id.to_string());
        Ok(())
    }

    pub fn remove(&self, session_id: &str) -> Option<Arc<SessionSlot>> {
        let removed = self.slots.remove(session_id).map(|(_, slot)| slot);
        if removed.is_some() {
            self.order.lock().retain(|existing| existing != session_id);
            let mut active = self.active_session_id.write();
            if active.as_deref() == Some(session_id) {
                *active = self.order.lock().first().cloned();
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
                    busy: slot.is_busy(),
                })
            })
            .collect()
    }
}
