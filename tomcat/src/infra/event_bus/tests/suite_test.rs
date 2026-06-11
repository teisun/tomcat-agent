use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use super::super::*;
use crate::infra::events::{AgentEvent, ExtensionEvent, ToolOutput};
use crate::infra::wire;

/// CLI 曾每轮 `run` 结束后 `off` 掉 `auto_compaction_end`；Layer1 在 `readline` 空闲时才 emit 会无人消费。
/// 会话级监听应跨轮保留——本用例模拟「只摘掉占位监听、保留 compaction_end」后延迟 emit 仍能送达。
#[test]
fn auto_compaction_end_delivered_when_session_listener_not_off() {
    let bus = DefaultEventBus::new();
    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    let session_id = bus.on(
        wire::WIRE_AUTO_COMPACTION_END,
        Box::new(move |_| {
            h.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }),
    );
    let per_turn_id = bus.on("per_turn_placeholder", Box::new(|_| Ok(())));
    bus.off(per_turn_id);
    bus.emit_sync(
        wire::WIRE_AUTO_COMPACTION_END,
        EventContext::new(wire::WIRE_AUTO_COMPACTION_END, serde_json::json!({})),
    )
    .unwrap();
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    bus.off(session_id);
}

#[test]
fn on_returns_id_and_emit_sync_calls() {
    let bus = DefaultEventBus::new();
    let called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let c = called.clone();
    let id = bus.on(
        "test",
        Box::new(move |_ctx| {
            c.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }),
    );
    let ctx = EventContext::new("test", serde_json::Value::Null);
    bus.emit_sync("test", ctx).unwrap();
    assert!(called.load(std::sync::atomic::Ordering::SeqCst));
    bus.off(id);
}

#[test]
fn once_removes_after_emit() {
    let bus = DefaultEventBus::new();
    let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let c = count.clone();
    bus.once(
        "once",
        Box::new(move |_| {
            c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }),
    );
    bus.emit_sync("once", EventContext::new("once", serde_json::Value::Null))
        .unwrap();
    bus.emit_sync("once", EventContext::new("once", serde_json::Value::Null))
        .unwrap();
    assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[test]
fn single_listener_error_does_not_abort_others() {
    let bus = DefaultEventBus::new();
    let ok_called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let ok_c = ok_called.clone();
    bus.on(
        "err",
        Box::new(move |_| Err(AppError::Event("fail".to_string()))),
    );
    bus.on(
        "err",
        Box::new(move |_| {
            ok_c.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }),
    );
    let ctx = EventContext::new("err", serde_json::Value::Null);
    let _ = bus.emit_sync("err", ctx);
    assert!(ok_called.load(std::sync::atomic::Ordering::SeqCst));
}

#[test]
fn remove_plugin_listeners_removes_by_plugin_id() {
    let bus = DefaultEventBus::new();
    let called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let c = called.clone();
    bus.add_listener(
        "ev",
        false,
        Some("plugin_a".to_string()),
        0,
        Box::new(move |_| {
            c.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }),
    );
    bus.remove_plugin_listeners("plugin_a");
    bus.emit_sync("ev", EventContext::new("ev", serde_json::Value::Null))
        .unwrap();
    assert!(!called.load(std::sync::atomic::Ordering::SeqCst));
}

#[test]
fn off_removes_listener() {
    let bus = DefaultEventBus::new();
    let called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let c = called.clone();
    let id = bus.on(
        "off_test",
        Box::new(move |_| {
            c.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }),
    );
    bus.off(id);
    bus.emit_sync(
        "off_test",
        EventContext::new("off_test", serde_json::Value::Null),
    )
    .unwrap();
    assert!(!called.load(std::sync::atomic::Ordering::SeqCst));
}

#[test]
fn priority_order_higher_first() {
    let bus = DefaultEventBus::new();
    let order = std::sync::Arc::new(std::sync::Mutex::new(Vec::<i32>::new()));
    let o1 = order.clone();
    let o2 = order.clone();
    bus.add_listener(
        "pri",
        false,
        None,
        10,
        Box::new(move |_| {
            o1.lock().unwrap().push(10);
            Ok(())
        }),
    );
    bus.add_listener(
        "pri",
        false,
        None,
        5,
        Box::new(move |_| {
            o2.lock().unwrap().push(5);
            Ok(())
        }),
    );
    bus.emit_sync("pri", EventContext::new("pri", serde_json::Value::Null))
        .unwrap();
    let v = order.lock().unwrap().clone();
    assert_eq!(v, [10, 5]);
}

#[test]
fn event_context_with_plugin_id_and_priority() {
    let ctx = EventContext::new("ev", serde_json::json!({}))
        .with_plugin_id("plugin-1")
        .with_priority(42);
    assert_eq!(ctx.plugin_id.as_deref(), Some("plugin-1"));
    assert_eq!(ctx.session_id, None);
    assert_eq!(ctx.priority, 42);
}

#[test]
fn event_context_with_session_id_sets_non_empty_value() {
    let ctx = EventContext::new("ev", serde_json::json!({})).with_session_id("s1");
    assert_eq!(ctx.session_id.as_deref(), Some("s1"));
}

#[test]
fn event_context_with_session_id_trims_whitespace_and_rejects_blank() {
    let ctx = EventContext::new("ev", serde_json::json!({})).with_session_id("  s1  ");
    assert_eq!(ctx.session_id.as_deref(), Some("s1"));

    let blank = EventContext::new("ev", serde_json::json!({})).with_session_id("   ");
    assert_eq!(blank.session_id, None);
}

#[test]
fn event_context_new_defaults_session_id_to_none() {
    let ctx = EventContext::new("ev", serde_json::json!({}));
    assert_eq!(ctx.session_id, None);
}

#[test]
fn scoped_event_emitter_writes_session_id_to_payload_and_context() {
    let bus: Arc<dyn EventBus> = Arc::new(DefaultEventBus::new());
    let captured = Arc::new(std::sync::Mutex::new(None::<EventContext>));
    let captured_cb = Arc::clone(&captured);
    bus.on(
        wire::WIRE_TOOL_EXECUTION_START,
        Box::new(move |ctx| {
            *captured_cb.lock().unwrap() = Some(ctx);
            Ok(())
        }),
    );
    let emitter = ScopedEventEmitter::new(Arc::clone(&bus), "s1");
    emitter
        .emit(AgentEvent::ToolExecutionStart {
            tool_call_id: "c1".into(),
            tool_name: "read".into(),
            args: serde_json::json!({"path": "src/main.rs"}),
        })
        .unwrap();
    let ctx = captured.lock().unwrap().clone().expect("captured ctx");
    assert_eq!(ctx.event_name, wire::WIRE_TOOL_EXECUTION_START);
    assert_eq!(ctx.session_id.as_deref(), Some("s1"));
    assert_eq!(ctx.payload["sessionId"].as_str(), Some("s1"));
    assert_eq!(ctx.payload["toolCallId"].as_str(), Some("c1"));
}

#[test]
fn scoped_event_emitter_normalizes_empty_session_id_to_none() {
    let bus: Arc<dyn EventBus> = Arc::new(DefaultEventBus::new());
    let captured = Arc::new(std::sync::Mutex::new(None::<EventContext>));
    let captured_cb = Arc::clone(&captured);
    bus.on(
        wire::WIRE_TOOL_CALL_STREAMING,
        Box::new(move |ctx| {
            *captured_cb.lock().unwrap() = Some(ctx);
            Ok(())
        }),
    );
    let emitter = ScopedEventEmitter::new(Arc::clone(&bus), "");
    emitter
        .emit(AgentEvent::ToolCallStreaming {
            tool_call_id: "c1".into(),
            tool_name: "write".into(),
            args_preview: serde_json::json!({"path": "~/demo.txt"}),
        })
        .unwrap();
    let ctx = captured.lock().unwrap().clone().expect("captured ctx");
    assert_eq!(ctx.session_id, None);
    assert!(ctx.payload.get("sessionId").is_none());
}

#[test]
fn scoped_event_emitter_trims_whitespace_session_id_before_emitting() {
    let bus: Arc<dyn EventBus> = Arc::new(DefaultEventBus::new());
    let captured = Arc::new(std::sync::Mutex::new(None::<EventContext>));
    let captured_cb = Arc::clone(&captured);
    bus.on(
        wire::WIRE_AGENT_START,
        Box::new(move |ctx| {
            *captured_cb.lock().unwrap() = Some(ctx);
            Ok(())
        }),
    );
    let emitter = ScopedEventEmitter::new(Arc::clone(&bus), "  s1  ");
    emitter.emit(AgentEvent::AgentStart).unwrap();
    let ctx = captured.lock().unwrap().clone().expect("captured ctx");
    assert_eq!(ctx.session_id.as_deref(), Some("s1"));
    assert_eq!(ctx.payload["sessionId"].as_str(), Some("s1"));
}

#[test]
fn scoped_event_emitter_emits_extension_events_with_session_id() {
    let bus: Arc<dyn EventBus> = Arc::new(DefaultEventBus::new());
    let captured = Arc::new(std::sync::Mutex::new(None::<EventContext>));
    let captured_cb = Arc::clone(&captured);
    bus.on(
        wire::WIRE_TOOL_RESULT,
        Box::new(move |ctx| {
            *captured_cb.lock().unwrap() = Some(ctx);
            Ok(())
        }),
    );
    let emitter = ScopedEventEmitter::new(Arc::clone(&bus), "s1");
    emitter
        .emit_extension(ExtensionEvent::ToolResult {
            tool_name: "read".into(),
            tool_call_id: "c1".into(),
            input: serde_json::json!({"path": "src/main.rs"}),
            content: vec![crate::infra::events::ContentBlock(
                serde_json::json!({"text": "ok"}),
            )],
            details: None,
            is_error: false,
        })
        .unwrap();
    let ctx = captured.lock().unwrap().clone().expect("captured ctx");
    assert_eq!(ctx.event_name, wire::WIRE_TOOL_RESULT);
    assert_eq!(ctx.session_id.as_deref(), Some("s1"));
    assert_eq!(ctx.payload["sessionId"].as_str(), Some("s1"));
}

#[test]
fn scoped_event_emitter_emits_raw_payload_with_session_id_and_plugin_id() {
    let bus: Arc<dyn EventBus> = Arc::new(DefaultEventBus::new());
    let captured = Arc::new(std::sync::Mutex::new(None::<EventContext>));
    let captured_cb = Arc::clone(&captured);
    bus.on(
        "search_tools_preflight",
        Box::new(move |ctx| {
            *captured_cb.lock().unwrap() = Some(ctx);
            Ok(())
        }),
    );
    let emitter = ScopedEventEmitter::new(Arc::clone(&bus), "s1");
    emitter
        .emit_payload_with_plugin_id(
            "search_tools_preflight",
            serde_json::json!({"status": "ready"}),
            "plugin-1",
        )
        .unwrap();
    let ctx = captured.lock().unwrap().clone().expect("captured ctx");
    assert_eq!(ctx.session_id.as_deref(), Some("s1"));
    assert_eq!(ctx.plugin_id.as_deref(), Some("plugin-1"));
    assert_eq!(ctx.payload["sessionId"].as_str(), Some("s1"));
    assert_eq!(ctx.payload["status"].as_str(), Some("ready"));
}

#[tokio::test]
async fn scoped_event_emitter_can_be_cloned_into_spawn() {
    let bus: Arc<dyn EventBus> = Arc::new(DefaultEventBus::new());
    let captured = Arc::new(std::sync::Mutex::new(None::<EventContext>));
    let captured_cb = Arc::clone(&captured);
    bus.on(
        wire::WIRE_TOOL_EXECUTION_END,
        Box::new(move |ctx| {
            *captured_cb.lock().unwrap() = Some(ctx);
            Ok(())
        }),
    );
    let emitter = ScopedEventEmitter::new(Arc::clone(&bus), "s1");
    let task_emitter = emitter.clone();
    tokio::spawn(async move {
        task_emitter
            .emit(AgentEvent::ToolExecutionEnd {
                tool_call_id: "c1".into(),
                tool_name: "bash".into(),
                result: ToolOutput(serde_json::json!({"stdout": "ok"})),
                display: None,
                is_error: false,
            })
            .unwrap();
    })
    .await
    .unwrap();
    let ctx = captured.lock().unwrap().clone().expect("captured ctx");
    assert_eq!(ctx.session_id.as_deref(), Some("s1"));
    assert_eq!(ctx.payload["sessionId"].as_str(), Some("s1"));
}

#[test]
fn emit_sync_empty_event_name_no_listeners() {
    let bus = DefaultEventBus::new();
    let ctx = EventContext::new("no_listeners", serde_json::Value::Null);
    bus.emit_sync("no_listeners", ctx).unwrap();
}

#[tokio::test]
async fn emit_async_calls_listeners() {
    let bus = DefaultEventBus::new();
    let called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let c = called.clone();
    bus.on(
        "async_ev",
        Box::new(move |_| {
            c.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }),
    );
    let ctx = EventContext::new("async_ev", serde_json::Value::Null);
    bus.emit_async("async_ev", ctx).await.unwrap();
    assert!(called.load(std::sync::atomic::Ordering::SeqCst));
}

#[test]
fn listener_panic_is_caught_others_still_run() {
    let bus = DefaultEventBus::new();
    let ok_called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let ok_c = ok_called.clone();
    bus.on("panic_ev", Box::new(move |_| panic!("listener panic")));
    bus.on(
        "panic_ev",
        Box::new(move |_| {
            ok_c.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }),
    );
    let ctx = EventContext::new("panic_ev", serde_json::Value::Null);
    bus.emit_sync("panic_ev", ctx).unwrap();
    assert!(ok_called.load(std::sync::atomic::Ordering::SeqCst));
}
