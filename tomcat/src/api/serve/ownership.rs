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

/// Explicit allowlist: a new session command is not writable by observers by default.
fn observer_command(command: &ServeCommand) -> bool {
    read_command(command)
        || matches!(
            command,
            ServeCommand::SwitchSession { .. } | ServeCommand::CloseSession { .. }
        )
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

pub(super) fn authorize(state: &ServeState, command: &ServeCommand) -> Result<bool, AppError> {
    if !state.shared {
        return Ok(true);
    }
    let permitted = if state.is_closed() {
        false
    } else if let ServeCommand::ControlResponse { request_id, .. }
    | ServeCommand::ControlCancel { request_id, .. } = command
    {
        state
            .confirmation
            .owns_pending(request_id, state.connection_id)
            || state
                .ask_question
                .owns_pending(request_id, state.connection_id)
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
                        return Ok(false);
                    }
                    observer_command(command) || state.controls(&slot)
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
                            Ok(manager
                                .read_session_header_for_session(&id)?
                                .is_some_and(|header| header.origin.is_none()))
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
    Ok(permitted)
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
    if !state.is_acp() && slot.origin.is_some() {
        return Ok("observe");
    }
    let controller = *slot.controller.lock();
    if controller == Some(state.connection_id) && session_mcp.is_none() {
        if !slot.is_busy() {
            slot.ctx.sync_resource_inventory().await?;
        }
        return Ok("control");
    }
    if controller.is_some() && controller != Some(state.connection_id) {
        if state.is_acp() {
            return Err(AppError::Config("session_in_use".into()));
        }
        return Ok("observe");
    }
    if slot.is_busy() && controller == Some(state.connection_id) {
        return Err(AppError::Config("busy".into()));
    }
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
        if !already_receiving && !state.is_acp() {
            replay.send(&slot.session_id, &state.writer)?;
        }
    }
    slot.ctx.start_session_mcp();
    Ok("control")
}
