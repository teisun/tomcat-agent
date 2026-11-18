//! Admission and ownership checks shared by native connections and the ACP endpoint.
use std::sync::Arc;

use super::registry::SessionSlot;
use super::types::{OutFrame, ResponseFrame, ServeCommand};
use super::ServeState;
use crate::AppError;

/// Reads that do not change slot membership or control ownership.
fn read_command(command: &ServeCommand) -> bool {
    matches!(
        command,
        ServeCommand::GetState { .. }
            | ServeCommand::GetMessages { .. }
            | ServeCommand::ListCheckpoints { .. }
            | ServeCommand::GetSessionFiles { .. }
            | ServeCommand::GetSessionFileBaseline { .. }
            | ServeCommand::PreviewRewind { .. }
            | ServeCommand::GetInstructionCatalog { .. }
    )
}

/// Commands that only read or change this connection's displayed session.
fn observer_command(command: &ServeCommand) -> bool {
    read_command(command)
        || matches!(
            command,
            ServeCommand::SwitchSession { .. } | ServeCommand::CloseSession { .. }
        )
}

/// Commands that never acquire connection-bound execution resources.
fn shared_input_command(command: &ServeCommand) -> bool {
    match command {
        ServeCommand::Steer { params, .. } => params.only_if_running == Some(true),
        ServeCommand::Interrupt { .. }
        | ServeCommand::IngestAttachment { .. }
        | ServeCommand::RetainAttachmentLeases { .. }
        | ServeCommand::CacheAttachmentThumbnail { .. } => true,
        _ => false,
    }
}

pub(super) enum Admission {
    Denied,
    Allowed,
    Acquire(Arc<SessionSlot>),
}

pub(super) fn has_other_live_controller(state: &ServeState, slot: &SessionSlot) -> bool {
    let controller = *slot.controller.lock();
    controller
        .filter(|id| *id != state.connection_id)
        .and_then(|id| {
            state
                .connections
                .read()
                .get(&id)
                .and_then(std::sync::Weak::upgrade)
        })
        .is_some_and(|connection| !connection.is_closed())
}

/// Display-only name of the live controller, never the session's original source.
pub(super) fn controller_name(state: &ServeState, slot: &SessionSlot) -> Option<String> {
    let id = (*slot.controller.lock())?;
    let connection = state.connections.read().get(&id)?.upgrade()?;
    if connection.is_closed() {
        return None;
    }
    let crate::core::session::SessionOrigin::Acp { client_name } = connection.source()?;
    client_name.and_then(|name| {
        let name = name.trim();
        (!name.is_empty()).then(|| name.to_owned())
    })
}

/// Called under the slot's replay lock, before any events from the new controller.
/// Observers need the new name even when their own access remains unchanged.
pub(super) fn notify_session_access(
    state: &ServeState,
    slot: &SessionSlot,
    previous: Option<uuid::Uuid>,
) -> Result<(), AppError> {
    let controller = *slot.controller.lock();
    let name = controller_name(state, slot);
    let connections: Vec<_> = state
        .connections
        .read()
        .values()
        .filter_map(std::sync::Weak::upgrade)
        .collect();
    for connection in connections {
        if connection.is_acp() || connection.is_closed() {
            continue;
        }
        let controls = controller == Some(connection.connection_id);
        if controls || previous == Some(connection.connection_id) || connection.observes(slot) {
            let result = connection.writer.send(OutFrame::Event(serde_json::to_value(
                super::types::ServePlanEvent::SessionAccess {
                    session_id: slot.session_id.clone(),
                    access: if controls { "control" } else { "observe" }.into(),
                    controller_name: name.clone(),
                },
            )?));
            if connection.connection_id == state.connection_id {
                result?;
            }
        }
    }
    Ok(())
}

pub(super) fn needs_admission(command: &ServeCommand) -> bool {
    !read_command(command)
        && !matches!(
            command,
            ServeCommand::ListSessions { .. }
                | ServeCommand::ControlResponse { .. }
                | ServeCommand::ControlCancel { .. }
                | ServeCommand::Interrupt { .. }
        )
}

fn connection_command(command: &ServeCommand) -> bool {
    matches!(
        command,
        ServeCommand::ControlRequest { .. }
            | ServeCommand::NewSession { .. }
            | ServeCommand::ListSessions { .. }
            | ServeCommand::SetUiLanguage { .. }
            | ServeCommand::ListModels { .. }
            | ServeCommand::UpsertModel { .. }
            | ServeCommand::UpsertModels { .. }
            | ServeCommand::ListRemoteModels { .. }
            | ServeCommand::RemoveProvider { .. }
            | ServeCommand::RemoveModel { .. }
            | ServeCommand::SetProviderKey { .. }
            | ServeCommand::SetDefaultModel { .. }
            | ServeCommand::SetUtilityModel { .. }
            | ServeCommand::ListProviderKeys { .. }
            | ServeCommand::WebSearchSettings { .. }
            | ServeCommand::SetWebSearchRouting { .. }
            | ServeCommand::ClearProviderKey { .. }
            | ServeCommand::GetProjectTrust { .. }
            | ServeCommand::TrustProject { .. }
            | ServeCommand::ListConnectors { .. }
            | ServeCommand::ListConnectorTools { .. }
            | ServeCommand::AddConnector { .. }
            | ServeCommand::RemoveConnector { .. }
            | ServeCommand::TestConnector { .. }
            | ServeCommand::LoginConnector { .. }
            | ServeCommand::CancelLoginConnector { .. }
            | ServeCommand::LogoutConnector { .. }
            | ServeCommand::ReloadConnector { .. }
            | ServeCommand::SetConnectorToolFilter { .. }
            | ServeCommand::SetConnectorToolEnabled { .. }
    )
}

pub(super) fn authorize(state: &ServeState, command: &ServeCommand) -> Result<Admission, AppError> {
    if !state.shared {
        return Ok(Admission::Allowed);
    }
    let permitted = if state.is_closed() {
        false
    } else if let ServeCommand::ControlResponse { request_id, .. }
    | ServeCommand::ControlCancel { request_id, .. } = command
    {
        state
            .confirmation
            .owns_pending(request_id, state.connection_id)
            || if matches!(command, ServeCommand::ControlResponse { .. }) {
                state
                    .ask_question
                    .may_respond(request_id, state.connection_id)
            } else {
                state
                    .ask_question
                    .owns_pending(request_id, state.connection_id)
            }
    } else if matches!(command, ServeCommand::LoadExternalSession { .. }) {
        // Typed internal operation only: native JSON cannot deserialize this variant.
        state.is_acp()
    } else if connection_command(command) {
        true
    } else {
        let id = command
            .session_id()
            .map(str::to_owned)
            .or_else(|| state.active_session_id());
        match id {
            Some(id) => match state.registry.get(&id) {
                Some(slot) => {
                    // Shared native connections retain the existing project boundary.
                    // Check before read-only admission as well as control acquisition:
                    // knowing a live ID must not expose another project's history.
                    if !state.is_acp()
                        && !state.current_scope().is_some_and(|scope| {
                            scope.key == slot.ctx.session_runtime.session.current_session_key()
                        })
                    {
                        state.writer.send(OutFrame::Response(ResponseFrame::error(
                            command.command_id().map(str::to_owned),
                            Some(id),
                            "unknown_session",
                        )))?;
                        return Ok(Admission::Denied);
                    }
                    if observer_command(command) || state.controls(&slot) {
                        true
                    } else if !state.is_acp() {
                        if shared_input_command(command)
                            || matches!(command, ServeCommand::DeleteSession { .. })
                        {
                            true
                        } else {
                            return Ok(Admission::Acquire(slot));
                        }
                    } else {
                        false
                    }
                }
                // Cold opens already validate the same key in open_existing_session_slot.
                None if observer_command(command) => true,
                None => {
                    // Existing disk-only native operations remain scoped to this connection's
                    // own project, never to a different project it happened to observe.
                    super::scoped_session_manager(state)
                        .map(|manager| -> Result<bool, AppError> {
                            let Some(entry) = manager.get_session_by_id(&id)? else {
                                // Deleting an already-gone session remains an idempotent no-op.
                                // The deletion boundary revalidates scope before any mutation.
                                return Ok(matches!(command, ServeCommand::DeleteSession { .. }));
                            };
                            if entry.session_key != manager.current_session_key() {
                                return Ok(false);
                            }
                            Ok(manager.read_session_header_for_session(&id)?.is_some())
                        })
                        .transpose()?
                        .unwrap_or(false)
                }
            },
            None => observer_command(command),
        }
    };
    if !permitted {
        state.writer.send(OutFrame::Response(ResponseFrame::error(
            command.command_id().map(str::to_owned),
            command
                .session_id()
                .map(str::to_owned)
                .or_else(|| state.active_session_id()),
            if state.is_closed() {
                "connection_closed"
            } else {
                "read_only_session"
            },
        )))?;
    }
    Ok(if permitted {
        Admission::Allowed
    } else {
        Admission::Denied
    })
}

/// Called under runtime admission. Selection is separate from acquisition of control.
pub(super) async fn open_session(
    state: &Arc<ServeState>,
    slot: &Arc<SessionSlot>,
    session_mcp: Option<&crate::core::connector::mcp::manager::SessionMcpConfig>,
) -> Result<&'static str, AppError> {
    if !state.shared {
        return Ok("control");
    }
    let controller = *slot.controller.lock();
    if controller == Some(state.connection_id) && session_mcp.is_none() {
        if !slot.is_busy() {
            slot.ctx.sync_resource_inventory().await?;
        }
        return Ok("control");
    }
    if has_other_live_controller(state, slot) {
        if !state.is_acp() {
            return Ok("observe");
        }
        if slot.is_busy() || super::session_delete::slot_has_live_work(state, slot) {
            return Err(AppError::Config("session_in_use".into()));
        }
    }
    if slot.is_busy() && controller == Some(state.connection_id) {
        return Err(AppError::Config("busy".into()));
    }
    acquire(state, slot, session_mcp).await
}

/// All control transfers run under admission and leave every display selection intact.
pub(super) async fn acquire(
    state: &Arc<ServeState>,
    slot: &Arc<SessionSlot>,
    session_mcp: Option<&crate::core::connector::mcp::manager::SessionMcpConfig>,
) -> Result<&'static str, AppError> {
    if has_other_live_controller(state, slot)
        && (slot.is_busy() || super::session_delete::slot_has_live_work(state, slot))
    {
        return Err(AppError::Config("busy".into()));
    }
    let controller = *slot.controller.lock();
    if let Some(directory) = state.skills_dir.as_deref() {
        crate::core::skill::discovery::validate_connection_root(directory)?;
    }
    // Validate the replacement before touching control or retiring the old manager.
    let prepared = match session_mcp {
        Some(config) => {
            slot.ctx
                .prepare_session_mcp(config, &slot.session_id, state.child_environment())?
        }
        None => None,
    };
    // No controller is not a promise that cancelled work has already finished.
    if controller != Some(state.connection_id) {
        state.stop_slot_work(slot).await?;
    }
    slot.ctx.replace_session_mcp(prepared).await?;
    slot.ctx.bind_connection_skill_dir(state.skills_dir.clone());
    slot.ctx.sync_resource_inventory().await?;
    if state.is_closed() {
        return Err(AppError::Config("connection_closed".into()));
    }
    slot.ctx
        .session_runtime
        .bash_task_registry
        .set_environment(state.child_environment());
    super::ask_question::bind_panel_for_controller(state, &slot.ctx, &slot.session_id);
    {
        let replay = slot.message_replay.lock();
        let already_receiving = state.controls(slot) || state.observes(slot);
        *slot.controller.lock() = Some(state.connection_id);
        if controller != Some(state.connection_id) {
            notify_session_access(state, slot, controller)?;
        }
        if !already_receiving && !state.is_acp() {
            replay.send(&slot.session_id, &state.writer)?;
        }
    }
    slot.ctx.start_session_mcp();
    Ok("control")
}
