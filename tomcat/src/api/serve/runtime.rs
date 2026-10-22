//! Service-owned execution resources. A transport connection never owns this registry.
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Weak};

use parking_lot::RwLock;
use uuid::Uuid;

use crate::core::agent_registry::AgentRegistry;
use crate::{AppConfig, AppError, ModelPrefsStore};

use super::ask_question::ServeAskQuestionBridge;
use super::confirmation::ServeConfirmationBridge;
use super::connection::ServeConnection;
use super::controller_route::ControllerRoute;
use super::fanout_event_bus::FanoutEventBus;
use super::registry::{ChatContextRegistry, SessionSlot};

pub(crate) struct ServeRuntime {
    pub cfg: AppConfig,
    /// Explicit source: test runtimes never consult the developer's HOME.
    pub config_path: PathBuf,
    pub registry: Arc<ChatContextRegistry>,
    pub ask_question: ServeAskQuestionBridge,
    pub confirmation: ServeConfirmationBridge,
    /// Serialize slot membership changes and control transfer, including cleanup.
    /// Reads, approval replies and immediate cancellation do not wait on this lock.
    pub admission: tokio::sync::Mutex<()>,
    pub shared_model_catalog: crate::core::llm::SharedModelCatalog,
    pub shared_model_prefs: Arc<ModelPrefsStore>,
    pub shared_agent_registry: Arc<AgentRegistry>,
    pub shared_event_bus: Arc<FanoutEventBus>,
    pub shared: bool,
    pub shutdown: tokio_util::sync::CancellationToken,
    pub connections: RwLock<HashMap<Uuid, Weak<ServeConnection>>>,
}

impl ServeRuntime {
    pub fn new(
        cfg: AppConfig,
        config_path: PathBuf,
        shared_model_prefs: Arc<ModelPrefsStore>,
        shared: bool,
    ) -> Result<Arc<Self>, AppError> {
        let registry = Arc::new(ChatContextRegistry::new(cfg.serve.max_sessions));
        let shared_event_bus = Arc::new(FanoutEventBus::new());
        let shared_agent_registry = AgentRegistry::new().attach_event_bus(shared_event_bus.clone());
        let shared_model_catalog = crate::core::llm::SharedModelCatalog::load(&cfg)?;
        Ok(Arc::new_cyclic(|runtime| {
            let route = ControllerRoute::Runtime(runtime.clone());
            Self {
                ask_question: ServeAskQuestionBridge::with_route(route.clone()),
                confirmation: ServeConfirmationBridge::with_route(route, config_path.clone()),
                admission: tokio::sync::Mutex::new(()),
                cfg,
                config_path,
                registry,
                shared_model_catalog,
                shared_model_prefs,
                shared_agent_registry,
                shared_event_bus,
                shared,
                shutdown: tokio_util::sync::CancellationToken::new(),
                connections: RwLock::new(HashMap::new()),
            }
        }))
    }

    pub fn emit_event(&self, session_id: &str, payload: serde_json::Value) {
        let Some(slot) = self.registry.get(session_id) else {
            return;
        };
        let mut replay = slot.message_replay.lock();
        if self.shared {
            replay.record(&payload);
        }
        let controller = *slot.controller.lock();
        let connections: Vec<_> = self
            .connections
            .read()
            .values()
            .filter_map(Weak::upgrade)
            .collect();
        for connection in connections {
            if connection.is_closed() {
                continue;
            }
            if !self.shared
                || controller == Some(connection.connection_id)
                || connection.observes(&slot)
            {
                let _ = connection
                    .writer
                    .send(super::types::OutFrame::Event(payload.clone()));
            }
        }
    }

    pub fn remove_slot(&self, session_id: &str) -> Option<Arc<SessionSlot>> {
        let removed = self.registry.remove(session_id)?;
        let connections: Vec<_> = self
            .connections
            .read()
            .values()
            .filter_map(Weak::upgrade)
            .collect();
        for connection in connections {
            connection.forget_session(&removed);
        }
        Some(removed)
    }
}

impl ServeConnection {
    /// ACP cancellation is terminal only after tools have stopped too. Keep the
    /// controller and the slot, and serialize cleanup with admission/takeover.
    pub(crate) async fn finish_cancelled_turn(&self, session_id: &str) -> Result<(), AppError> {
        let _admission = self.admission.lock().await;
        let slot = self
            .registry
            .get(session_id)
            .ok_or_else(|| AppError::Config("unknown_session".into()))?;
        if !self.controls(&slot) {
            return Err(AppError::Config("read_only_session".into()));
        }
        self.stop_slot_work(&slot).await
    }

    /// EOF cancels first, before queued commands or a slow bootstrap can delay teardown.
    pub(super) fn begin_disconnect(&self) {
        self.lifetime.cancel();
        for slot in self.registry.slots() {
            if *slot.controller.lock() == Some(self.connection_id) {
                self.ask_question
                    .cancel_live_session(&slot.session_id, "connection_closed");
                self.confirmation
                    .cancel_live_session(&slot.session_id, "connection_closed");
                slot.ctx.session_runtime.cancel_token.lock().cancel();
                slot.ctx.suspend_session_mcp();
                slot.ctx.agent_registry.cascade_abort(&slot.session_id);
            }
        }
    }

    pub(super) async fn disconnect(&self) -> Result<(), AppError> {
        self.begin_disconnect();
        let _admission = self.admission.lock().await;
        // Re-scan after admission: a bootstrap already in flight may have installed a slot.
        let slots: Vec<_> = self
            .registry
            .slots()
            .into_iter()
            .filter(|slot| *slot.controller.lock() == Some(self.connection_id))
            .collect();
        for slot in &slots {
            *slot.controller.lock() = None;
        }
        let results =
            futures_util::future::join_all(slots.iter().map(|slot| self.stop_slot_work(slot)))
                .await;
        let failures: Vec<_> = results
            .into_iter()
            .filter_map(Result::err)
            .map(|e| e.to_string())
            .collect();
        if failures.is_empty() {
            Ok(())
        } else {
            Err(AppError::Config(failures.join("; ")))
        }
    }
}

impl ServeRuntime {
    pub(super) async fn stop_all(&self) -> Result<(), AppError> {
        self.shutdown.cancel();
        let connections: Vec<_> = self
            .connections
            .read()
            .values()
            .filter_map(Weak::upgrade)
            .collect();
        for connection in connections {
            connection.begin_disconnect();
        }
        let _admission = self.admission.lock().await;
        let results = futures_util::future::join_all(self.registry.slots().into_iter().map(
            |slot| async move {
                let stopped = self.stop_slot_work(&slot).await;
                let cleaned =
                    super::cleanup_session_slot(self, &slot, super::SlotCleanup::Shutdown).await;
                stopped.and(cleaned)
            },
        ))
        .await;
        let failures: Vec<_> = results
            .into_iter()
            .filter_map(Result::err)
            .map(|e| e.to_string())
            .collect();
        if failures.is_empty() {
            Ok(())
        } else {
            Err(AppError::Config(failures.join("; ")))
        }
    }

    /// Retain the slot and durable history; a replacement controller must wait for real quiescence.
    pub(super) async fn stop_slot_work(&self, slot: &Arc<SessionSlot>) -> Result<(), AppError> {
        use crate::core::tools::primitive::BashTaskStatus;
        let mut failures = Vec::new();
        self.ask_question
            .cancel_live_session(&slot.session_id, "connection_closed");
        self.confirmation
            .cancel_live_session(&slot.session_id, "connection_closed");
        slot.ctx.session_runtime.cancel_token.lock().cancel();
        slot.ctx.agent_registry.cascade_abort(&slot.session_id);
        slot.ctx.suspend_session_mcp();
        let bash = &slot.ctx.session_runtime.bash_task_registry;
        let running: Vec<_> = bash
            .list()
            .into_iter()
            .filter(|task| {
                matches!(
                    task.status,
                    BashTaskStatus::Running | BashTaskStatus::DrainingOutput
                )
            })
            .collect();
        for task in &running {
            if let Err(error) = bash.stop(&task.task_id).await {
                failures.push(error.to_string());
            }
        }
        // Maintenance can be writing a checkpoint/transcript. Never abort it and claim success.
        if slot.is_command_job_running() {
            let handle = slot.run_task.lock().take();
            if let Some(mut handle) = handle {
                if tokio::time::timeout(super::SESSION_SHUTDOWN_TIMEOUT, &mut handle)
                    .await
                    .is_err()
                {
                    if slot.is_busy() {
                        *slot.run_task.lock() = Some(handle);
                    }
                    failures.push("stop_timeout".into());
                }
            }
        }
        if let Err(error) = super::cancel_and_wait(self, slot).await {
            failures.push(error.into());
        }
        if let Err(error) = slot.ctx.stop_session_mcp().await {
            failures.push(error.to_string());
        }
        let drained = tokio::time::timeout(super::SESSION_SHUTDOWN_TIMEOUT, async {
            for task in &running {
                bash.wait_for_finish(&task.task_id).await?;
            }
            Ok::<_, AppError>(())
        })
        .await;
        match drained {
            Ok(Ok(())) => {}
            Ok(Err(error)) => failures.push(error.to_string()),
            Err(_) => failures.push("stop_timeout".into()),
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(AppError::Config(failures.join("; ")))
        }
    }

    pub(super) async fn ensure_capacity(&self) -> Result<(), AppError> {
        if self.registry.len() < self.registry.max_sessions() {
            return Ok(());
        }
        if self.shared {
            let connections: Vec<_> = self
                .connections
                .read()
                .values()
                .filter_map(Weak::upgrade)
                .collect();
            let mut candidates = self.registry.slots();
            candidates.sort_by_key(|slot| *slot.last_used.lock());
            for slot in candidates {
                if slot.is_busy()
                    || self.ask_question.has_pending_session(&slot.session_id)
                    || self.confirmation.has_pending_session(&slot.session_id)
                {
                    continue;
                }
                if slot
                    .ctx
                    .session_runtime
                    .bash_task_registry
                    .list()
                    .iter()
                    .any(|task| {
                        matches!(
                            task.status,
                            crate::core::tools::primitive::BashTaskStatus::Running
                                | crate::core::tools::primitive::BashTaskStatus::DrainingOutput
                        )
                    })
                {
                    continue;
                }
                // Current views keep their slots; ACP also retains every controlled session.
                let controller = *slot.controller.lock();
                if connections.iter().any(|connection| {
                    !connection.is_closed()
                        && (connection.active_session_id().as_deref()
                            == Some(slot.session_id.as_str())
                            || connection.is_acp() && controller == Some(connection.connection_id))
                }) {
                    continue;
                }
                self.stop_slot_work(&slot).await?;
                super::cleanup_session_slot(self, &slot, super::SlotCleanup::Close).await?;
                return Ok(());
            }
        }
        Err(AppError::Config("too_many_sessions".into()))
    }
}
