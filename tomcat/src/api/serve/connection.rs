//! Connection-local selection, handshake and response destination.
use std::ops::Deref;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use parking_lot::RwLock;
use uuid::Uuid;

use crate::{AppConfig, AppError, ModelPrefsStore};

use super::registry::{SessionScope, SessionSlot};
use super::runtime::ServeRuntime;
use super::writer::WriterHandle;

#[derive(Default)]
struct Selection {
    active_session_id: Option<String>,
    scope: Option<SessionScope>,
}

pub(crate) struct ServeConnection {
    pub runtime: Arc<ServeRuntime>,
    pub connection_id: Uuid,
    pub writer: WriterHandle,
    pub initialized: AtomicBool,
    pub lifetime: tokio_util::sync::CancellationToken,
    environment: Arc<crate::infra::platform::EnvironmentSnapshot>,
    pub skills_dir: Option<PathBuf>,
    source: RwLock<Option<crate::core::session::SessionOrigin>>,
    selection: RwLock<Selection>,
}

// A command uses its connection's output/selection and the shared runtime's resources.
impl Deref for ServeConnection {
    type Target = ServeRuntime;

    fn deref(&self) -> &Self::Target {
        &self.runtime
    }
}

impl ServeConnection {
    pub(super) fn new(
        cfg: AppConfig,
        config_path: PathBuf,
        writer: WriterHandle,
        shared_model_prefs: Arc<ModelPrefsStore>,
    ) -> Result<Arc<Self>, AppError> {
        Ok(Self::connect(
            ServeRuntime::new(cfg, config_path, shared_model_prefs, false)?,
            writer,
        ))
    }

    pub fn connect(runtime: Arc<ServeRuntime>, writer: WriterHandle) -> Arc<Self> {
        Self::connect_with_source(runtime, writer, None, Default::default(), None)
    }

    #[cfg(test)]
    pub fn connect_acp(runtime: Arc<ServeRuntime>, writer: WriterHandle) -> Arc<Self> {
        Self::connect_with_source(
            runtime,
            writer,
            Some(crate::core::session::SessionOrigin::Acp { client_name: None }),
            Default::default(),
            None,
        )
    }

    pub(crate) fn connect_with_source(
        runtime: Arc<ServeRuntime>,
        writer: WriterHandle,
        source: Option<crate::core::session::SessionOrigin>,
        environment: crate::infra::platform::EnvironmentSnapshot,
        skills_dir: Option<PathBuf>,
    ) -> Arc<Self> {
        let lifetime = runtime.shutdown.child_token();
        let connection = Arc::new(Self {
            runtime,
            connection_id: Uuid::new_v4(),
            writer,
            initialized: AtomicBool::new(false),
            lifetime,
            environment: Arc::new(environment),
            skills_dir,
            source: RwLock::new(source),
            selection: RwLock::new(Selection::default()),
        });
        connection
            .connections
            .write()
            .insert(connection.connection_id, Arc::downgrade(&connection));
        connection
    }

    pub(crate) fn child_environment(
        &self,
    ) -> Option<Arc<crate::infra::platform::EnvironmentSnapshot>> {
        self.shared.then(|| self.environment.clone())
    }

    pub fn source(&self) -> Option<crate::core::session::SessionOrigin> {
        self.source.read().clone()
    }

    pub fn is_acp(&self) -> bool {
        self.source.read().is_some()
    }

    pub fn set_acp_client_name(&self, name: Option<String>) {
        if let Some(crate::core::session::SessionOrigin::Acp { client_name }) =
            self.source.write().as_mut()
        {
            *client_name = name;
        }
    }

    pub fn active_session_id(&self) -> Option<String> {
        self.selection.read().active_session_id.clone()
    }

    pub(super) fn current_scope(&self) -> Option<SessionScope> {
        self.selection.read().scope.clone()
    }

    /// Existing session initialization supplies the directory, not the transport handshake.
    pub(super) fn set_session_scope(&self, scope: SessionScope) {
        self.selection.write().scope = Some(scope);
    }

    pub fn resolve_session_id(&self, requested: Option<&str>) -> Result<String, AppError> {
        let selected = requested
            .map(str::to_owned)
            .or_else(|| self.active_session_id());
        selected
            .filter(|id| self.registry.get(id).is_some())
            .ok_or_else(|| AppError::Config("unknown_session".into()))
    }

    pub fn set_active_session(&self, id: &str) -> Result<(), AppError> {
        let slot = self
            .registry
            .get(id)
            .ok_or_else(|| AppError::Config("unknown_session".into()))?;
        // Same lock as capture + fanout: no live delta can slip between the
        // prefix and activation. Never hold it while registering bus listeners.
        let replay = slot.message_replay.lock();
        let control = self.controls(&slot);
        let mut selection = self.selection.write();
        if self.shared
            && !control
            && !selection.scope.as_ref().is_some_and(|scope| {
                scope.key == slot.ctx.session_runtime.session.current_session_key()
            })
        {
            return Err(AppError::Config("unknown_session".into()));
        }
        let already_receiving = control || selection.active_session_id.as_deref() == Some(id);
        selection.active_session_id = Some(id.to_owned());
        if !self.shared || control {
            selection.scope = Some(SessionScope::from_slot(&slot));
        }
        if self.shared && !already_receiving {
            replay.send(id, &self.writer)?;
        }
        slot.touch();
        Ok(())
    }

    pub fn is_closed(&self) -> bool {
        self.lifetime.is_cancelled()
    }

    pub fn controls(&self, slot: &SessionSlot) -> bool {
        !self.is_closed() && *slot.controller.lock() == Some(self.connection_id)
    }

    pub(super) fn observes(&self, slot: &SessionSlot) -> bool {
        let selection = self.selection.read();
        selection.active_session_id.as_deref() == Some(slot.session_id.as_str())
            && selection.scope.as_ref().is_some_and(|scope| {
                scope.key == slot.ctx.session_runtime.session.current_session_key()
            })
    }

    pub fn clear_displayed_session(&self, id: &str) {
        let slot = self.registry.get(id);
        let _replay = slot.as_ref().map(|slot| slot.message_replay.lock());
        let mut selection = self.selection.write();
        if selection.active_session_id.as_deref() == Some(id) {
            selection.active_session_id = None;
        }
    }

    pub(super) fn forget_session(&self, removed: &SessionSlot) {
        let mut selection = self.selection.write();
        if selection.active_session_id.as_deref() != Some(removed.session_id.as_str()) {
            return;
        }
        selection.active_session_id = None;
        if !self.shared {
            // Preserve private stdio's selection of the first remaining session after close.
            if let Some(slot) = self
                .registry
                .list()
                .first()
                .and_then(|s| self.registry.get(&s.session_id))
            {
                selection.active_session_id = Some(slot.session_id.clone());
                selection.scope = Some(SessionScope::from_slot(&slot));
            }
        }
        // Retain the trusted disk scope even when its last runtime is closed.
    }
}

impl Drop for ServeConnection {
    fn drop(&mut self) {
        self.connections.write().remove(&self.connection_id);
    }
}
