use super::super::{register_thinking_persist_listeners, unregister_thinking_persist_listeners};
use crate::infra::event_bus::EventContext;
use crate::infra::events::wire;
use crate::infra::{DefaultEventBus, EventBus};
use crate::SessionManager;

fn temp_sessions_dir() -> std::path::PathBuf {
    let mut p = std::env::temp_dir();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    p.push(format!("pi_chat_thinking_persist_{}", nanos));
    p
}

#[test]
fn thinking_deltas_are_persisted_as_single_transcript_entry() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let session = SessionManager::new(dir.clone());
    session
        .create_session(session.current_session_key(), None)
        .unwrap();
    let transcript_path = session.current_transcript_path().unwrap().unwrap();

    let bus = DefaultEventBus::new();
    let ids = register_thinking_persist_listeners(&bus, transcript_path);

    bus.emit_sync(
        wire::WIRE_MESSAGE_UPDATE,
        EventContext::new(
            wire::WIRE_MESSAGE_UPDATE,
            serde_json::json!({
                "assistantMessageEvent": {
                    "kind": "thinking_delta",
                    "delta": "step-1 ",
                }
            }),
        ),
    )
    .unwrap();
    bus.emit_sync(
        wire::WIRE_MESSAGE_UPDATE,
        EventContext::new(
            wire::WIRE_MESSAGE_UPDATE,
            serde_json::json!({
                "assistantMessageEvent": {
                    "kind": "thinking_delta",
                    "delta": "step-2",
                    "signature": "sig-xyz",
                }
            }),
        ),
    )
    .unwrap();
    bus.emit_sync(
        wire::WIRE_MESSAGE_END,
        EventContext::new(wire::WIRE_MESSAGE_END, serde_json::json!({})),
    )
    .unwrap();
    unregister_thinking_persist_listeners(&bus, &ids);

    let entries = session.get_entries(10).unwrap();
    assert_eq!(entries.len(), 1);
    match &entries[0] {
        crate::core::session::TranscriptEntry::ThinkingTrace(e) => {
            assert_eq!(e.text, "step-1 step-2");
            assert_eq!(e.signature.as_deref(), Some("sig-xyz"));
        }
        other => panic!("expected thinking_trace, got {:?}", other),
    }

    let _ = std::fs::remove_dir_all(&dir);
}
