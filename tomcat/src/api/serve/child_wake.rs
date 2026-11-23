//! Reports are durable within the follow-up queue, not the notification. Recheck
//! after batch close and every busy→idle transition under prompt admission.
use super::{
    commands::{start_turn, TurnAck},
    ServeState, SessionSlot,
};
use std::sync::{Arc, Weak};

pub(super) fn listen(state: &Arc<ServeState>, slot: &Arc<SessionSlot>) {
    let mut handle = slot.child_wake_listener.lock();
    if handle.is_some() {
        return;
    }
    let runtime = Arc::downgrade(&state.runtime);
    let reports = slot.ctx.session_runtime.plan_runtime.child_reports.clone();
    let weak_slot = Arc::downgrade(slot);
    *handle = Some(tokio::spawn(async move {
        loop {
            reports.changed.notified().await;
            let (Some(runtime), Some(slot)) = (runtime.upgrade(), weak_slot.upgrade()) else {
                break;
            };
            if slot.is_busy() {
                continue;
            }
            let _admission = runtime.admission.lock().await;
            let controller = (*slot.controller.lock())
                .and_then(|id| runtime.connections.read().get(&id).and_then(Weak::upgrade));
            let Some(controller) = controller.filter(|c| c.controls(&slot)) else {
                continue;
            };
            if slot.is_busy() {
                continue;
            }
            let can_wake = !slot.is_interrupted()
                && slot.auto_turns.load(std::sync::atomic::Ordering::SeqCst)
                    < crate::api::chat::AUTO_TURN_BUDGET;
            if can_wake && reports.ready() {
                if let Err(error) =
                    start_turn(controller, slot, None, None, TurnAck::ChildReports).await
                {
                    tracing::warn!(%error, "could not start child report turn");
                }
                continue;
            }
            if reports.has_open_batch() {
                continue;
            }
            if slot.ctx.agent_registry.has_children(&slot.session_id) {
                // Never keep the admission lock while a child may need the controller.
                drop(_admission);
                slot.ctx
                    .agent_registry
                    .wait_for_children(&slot.session_id)
                    .await;
                reports.changed.notify_one();
                continue;
            }
            // A batch could have closed between checking readiness and checking
            // live children. Recheck before consuming the terminal outcome.
            if can_wake && reports.ready() {
                reports.changed.notify_one();
                continue;
            }
            let Some((owner, mut outcome, failure)) = slot.pending_settled.lock().take() else {
                continue;
            };
            if owner != controller.connection_id {
                continue;
            }
            if slot.is_interrupted() {
                outcome = crate::infra::events::AgentIdleOutcome::Interrupted;
            }
            let event = super::types::ServePlanEvent::SessionSettled {
                session_id: slot.session_id.clone(),
                outcome,
                failure,
            };
            controller.emit_event(
                &slot.session_id,
                serde_json::to_value(event).expect("session_settled serializes"),
            );
        }
    }));
}
