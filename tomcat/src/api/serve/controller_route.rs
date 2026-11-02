//! Resolve approval destinations at request time, not when the session was first opened.
use std::sync::Weak;

use uuid::Uuid;

use super::runtime::ServeRuntime;
use super::writer::WriterHandle;

#[derive(Clone)]
pub(super) struct ControlRecipient {
    pub connection_id: Uuid,
    pub writer: WriterHandle,
}

#[derive(Clone)]
pub(super) enum ControllerRoute {
    Direct(WriterHandle),
    Runtime(Weak<ServeRuntime>),
}

impl ControllerRoute {
    pub fn recipient(&self, session_id: &str) -> Option<ControlRecipient> {
        match self {
            Self::Direct(writer) => Some(ControlRecipient {
                connection_id: Uuid::nil(),
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
                    writer: connection.writer.clone(),
                })
            }
        }
    }
}
