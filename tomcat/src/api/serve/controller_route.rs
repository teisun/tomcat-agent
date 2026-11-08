//! Resolve approval destinations at request time, not when the session was first opened.
use std::sync::Weak;

use uuid::Uuid;

use super::runtime::ServeRuntime;
use super::writer::WriterHandle;

#[derive(Clone)]
pub(super) struct ControlRecipient {
    pub connection_id: Uuid,
    pub is_acp: bool,
    pub writer: WriterHandle,
}

#[derive(Clone)]
pub(super) enum ControllerRoute {
    Direct(WriterHandle),
    Runtime(Weak<ServeRuntime>),
}

impl ControllerRoute {
    pub fn observers(&self, session_id: &str) -> Vec<ControlRecipient> {
        let Self::Runtime(runtime) = self else {
            return Vec::new();
        };
        let Some(runtime) = runtime.upgrade() else {
            return Vec::new();
        };
        let Some(slot) = runtime.registry.get(session_id) else {
            return Vec::new();
        };
        let connections: Vec<_> = runtime
            .connections
            .read()
            .values()
            .filter_map(Weak::upgrade)
            .collect();
        connections
            .into_iter()
            .filter(|connection| {
                !connection.is_closed() && !connection.is_acp() && connection.observes(&slot)
            })
            .map(|connection| ControlRecipient {
                connection_id: connection.connection_id,
                writer: connection.writer.clone(),
                is_acp: false,
            })
            .collect()
    }

    pub fn is_observer(&self, session_id: &str, connection_id: Uuid) -> bool {
        let Self::Runtime(runtime) = self else {
            return false;
        };
        let Some(runtime) = runtime.upgrade() else {
            return false;
        };
        let Some(slot) = runtime.registry.get(session_id) else {
            return false;
        };
        let connection = runtime
            .connections
            .read()
            .get(&connection_id)
            .and_then(std::sync::Weak::upgrade);
        connection.is_some_and(|connection| {
            !connection.is_closed() && !connection.is_acp() && connection.observes(&slot)
        })
    }

    pub fn recipient(&self, session_id: &str) -> Option<ControlRecipient> {
        match self {
            Self::Direct(writer) => Some(ControlRecipient {
                connection_id: Uuid::nil(),
                is_acp: false,
                writer: writer.clone(),
            }),
            Self::Runtime(runtime) => {
                let runtime = runtime.upgrade()?;
                let slot = runtime.registry.get(session_id)?;
                let id = (*slot.controller.lock())?;
                let connection = runtime
                    .connections
                    .read()
                    .get(&id)
                    .and_then(Weak::upgrade)?;
                if connection.is_closed() {
                    return None;
                }
                Some(ControlRecipient {
                    connection_id: id,
                    is_acp: connection.is_acp(),
                    writer: connection.writer.clone(),
                })
            }
        }
    }
}
