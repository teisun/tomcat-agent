//! EventBus -> `serve` writer 的桥接层。
//!
//! 每个会话各自订阅一组 `WIRE_*` 事件，并按 `sessionId` 过滤后转发到统一 writer。

use std::sync::Arc;

use crate::api::serve::registry::SessionSlot;
use crate::infra::events::wire;
use crate::EventListenerId;

use super::types::OutFrame;
use super::writer::WriterHandle;

const EVENT_NAMES: &[&str] = &[
    wire::WIRE_AGENT_START,
    wire::WIRE_AGENT_END,
    wire::WIRE_TURN_START,
    wire::WIRE_TURN_END,
    wire::WIRE_MESSAGE_START,
    wire::WIRE_MESSAGE_UPDATE,
    wire::WIRE_MESSAGE_END,
    wire::WIRE_LLM_ERROR,
    wire::WIRE_LLM_NOTICE,
    wire::WIRE_TOOL_EXECUTION_START,
    wire::WIRE_TOOL_CALL_STREAMING,
    wire::WIRE_TOOL_EXECUTION_UPDATE,
    wire::WIRE_TOOL_EXECUTION_END,
    wire::WIRE_AUTO_COMPACTION_START,
    wire::WIRE_AUTO_COMPACTION_END,
    wire::WIRE_COMPACTION_ERROR,
    wire::WIRE_TOOL_RESULT_TRUNCATED,
    wire::WIRE_AUTO_RETRY_START,
    wire::WIRE_AUTO_RETRY_END,
    wire::WIRE_CONTEXT_METRICS_UPDATE,
    wire::WIRE_TOOL_RESULT_PERSISTED,
    wire::WIRE_BOUNDARY_SWITCHED,
    wire::WIRE_CONTEXT_OVERFLOW_TRIM_START,
    wire::WIRE_CONTEXT_OVERFLOW_TRIM_END,
    wire::WIRE_LAYER0_CONTEXT_RELEASE,
    wire::WIRE_EXTENSION_ERROR,
    wire::WIRE_SEARCH_TOOLS_PREFLIGHT,
    wire::WIRE_GIT_PREFLIGHT,
    wire::WIRE_AGENT_INTERRUPTED,
    wire::WIRE_SUB_AGENT_START,
    wire::WIRE_SUB_AGENT_END,
    wire::WIRE_PLAN_CREATE,
    wire::WIRE_PLAN_BUILD,
    wire::WIRE_PLAN_UPDATE,
    wire::WIRE_PLAN_REVIEW,
    wire::WIRE_PLAN_CODE_REVIEW,
    wire::WIRE_PLAN_VERIFY,
    wire::WIRE_PLAN_REVIEW_WARNING,
    wire::WIRE_PLAN_CODE_REVIEW_WARNING,
    wire::WIRE_PLAN_COMPLETE,
    wire::WIRE_PLAN_TODOS,
    wire::WIRE_SESSION_TITLE_UPDATED,
    wire::WIRE_SESSION_TODOS,
];

pub fn register_session_event_pump(
    slot: &Arc<SessionSlot>,
    writer: WriterHandle,
) -> Vec<EventListenerId> {
    let mut ids = Vec::new();
    for event_name in EVENT_NAMES {
        let session_id = slot.session_id.clone();
        let slot_for_listener = Arc::clone(slot);
        let event_bus = Arc::clone(&slot.ctx.global_services.event_bus);
        let writer = writer.clone();
        let id = event_bus.on(
            event_name,
            Box::new(move |ctx| {
                if ctx.session_id.as_deref() != Some(session_id.as_str()) {
                    return Ok(());
                }
                if event_name == &wire::WIRE_AGENT_END {
                    slot_for_listener.mark_terminal_emitted();
                }
                let _ = writer.send(OutFrame::Event(ctx.payload.clone()));
                Ok(())
            }),
        );
        ids.push(id);
    }
    ids
}

pub fn unregister_session_event_pump(slot: &Arc<SessionSlot>) {
    let ids = std::mem::take(&mut *slot.listener_ids.lock());
    for id in ids {
        slot.ctx.global_services.event_bus.off(id);
    }
}
