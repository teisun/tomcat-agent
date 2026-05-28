use std::sync::Arc;

use parking_lot::Mutex;
use tracing::warn;

use crate::infra::event_bus::EventListenerId;
use crate::infra::EventBus;

#[derive(Default)]
struct ThinkingPersistState {
    text: String,
    signature: Option<String>,
}

pub(crate) struct ThinkingPersistListenerIds {
    msg_update: EventListenerId,
    msg_end: EventListenerId,
}

pub(crate) fn register_thinking_persist_listeners(
    bus: &dyn EventBus,
    transcript_path: std::path::PathBuf,
) -> ThinkingPersistListenerIds {
    let state = Arc::new(Mutex::new(ThinkingPersistState::default()));

    let state_for_update = Arc::clone(&state);
    let msg_update = bus.on(
        crate::infra::wire::WIRE_MESSAGE_UPDATE,
        Box::new(move |evt: crate::infra::event_bus::EventContext| {
            let event = match evt.payload.get("assistantMessageEvent") {
                Some(event) => event,
                None => return Ok(()),
            };
            if event.get("kind").and_then(|v| v.as_str()) != Some("thinking_delta") {
                return Ok(());
            }
            let delta = event.get("delta").and_then(|v| v.as_str()).unwrap_or("");
            if delta.is_empty() {
                return Ok(());
            }
            let mut state_guard = state_for_update.lock();
            state_guard.text.push_str(delta);
            if let Some(signature) = event.get("signature").and_then(|v| v.as_str()) {
                state_guard.signature = Some(signature.to_string());
            }
            Ok(())
        }),
    );

    let state_for_end = Arc::clone(&state);
    let msg_end = bus.on(
        crate::infra::wire::WIRE_MESSAGE_END,
        Box::new(move |_evt: crate::infra::event_bus::EventContext| {
            let (text, signature) = {
                let mut state_guard = state_for_end.lock();
                if state_guard.text.is_empty() {
                    return Ok(());
                }
                (
                    std::mem::take(&mut state_guard.text),
                    state_guard.signature.take(),
                )
            };
            let entry =
                crate::core::session::TranscriptEntry::ThinkingTrace(crate::core::session::ThinkingTraceEntry {
                    id: None,
                    parent_id: None,
                    timestamp: chrono::Utc::now()
                        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                    text,
                    signature,
                });
            if let Err(err) = crate::core::session::append_entry(&transcript_path, &entry) {
                warn!(error = %err, "append thinking_trace entry failed");
            }
            Ok(())
        }),
    );

    ThinkingPersistListenerIds {
        msg_update,
        msg_end,
    }
}

pub(crate) fn unregister_thinking_persist_listeners(
    bus: &dyn EventBus,
    ids: &ThinkingPersistListenerIds,
) {
    bus.off(ids.msg_update);
    bus.off(ids.msg_end);
}
