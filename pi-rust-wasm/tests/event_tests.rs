//! 集成测试：事件总线（DefaultEventBus）on/emit_sync/off 与 remove_plugin_listeners 行为。
//! 验证插件卸载后事件监听彻底释放，符合 INTEGRATION_TEST_PRACTICE 场景 B。

mod common;

use pi_awsm::{DefaultEventBus, EventBus, EventContext};
use std::sync::atomic::{AtomicU32, Ordering};

#[test]
fn test_event_bus_on_emit_sync_invokes_callback() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = tracing::info_span!("test_event_bus_on_emit_sync_invokes_callback").entered();

    let bus = DefaultEventBus::new();
    let count = std::sync::Arc::new(AtomicU32::new(0));

    let c = std::sync::Arc::clone(&count);
    bus.on("test.event", Box::new(move |_ctx| {
        c.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }));
    tracing::info!("Arrange: 创建 DefaultEventBus，注册 test.event 回调");
    let ctx = EventContext::new("test.event", serde_json::json!({}));
    tracing::info!("Act: 调用 emit_sync(test.event, ctx)");
    bus.emit_sync("test.event", ctx)?;
    tracing::info!("Assert: 验证回调被触发一次");
    assert_eq!(count.load(Ordering::SeqCst), 1, "emit_sync 应触发回调一次");

    Ok(())
}

#[test]
fn test_event_bus_off_removes_listener() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = tracing::info_span!("test_event_bus_off_removes_listener").entered();

    let bus = DefaultEventBus::new();
    let count = std::sync::Arc::new(AtomicU32::new(0));

    let c = std::sync::Arc::clone(&count);
    let id = bus.on("off.test", Box::new(move |_ctx| {
        c.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }));
    tracing::info!("Arrange: 创建 DefaultEventBus，注册 off.test 回调");
    bus.emit_sync("off.test", EventContext::new("off.test", serde_json::json!({})))?;
    assert_eq!(count.load(Ordering::SeqCst), 1);
    tracing::info!("Act: 调用 off(id)，再次 emit_sync");
    bus.off(id);
    bus.emit_sync("off.test", EventContext::new("off.test", serde_json::json!({})))?;
    tracing::info!("Assert: 验证 off 后不再触发");
    assert_eq!(count.load(Ordering::SeqCst), 1, "off 后不应再触发");

    Ok(())
}

#[test]
fn test_event_bus_remove_plugin_listeners_cleans_up_after_unload() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = tracing::info_span!("test_event_bus_remove_plugin_listeners_cleans_up_after_unload").entered();

    let bus = DefaultEventBus::new();
    let count = std::sync::Arc::new(AtomicU32::new(0));

    let c = std::sync::Arc::clone(&count);
    bus.add_listener(
        "session.start",
        false,
        Some("plugin-a".to_string()),
        0,
        Box::new(move |_ctx| {
            c.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }),
    );
    tracing::info!("Arrange: 创建 DefaultEventBus，为 plugin-a 注册 session.start 监听");
    bus.emit_sync("session.start", EventContext::new("session.start", serde_json::json!({})))?;
    assert_eq!(count.load(Ordering::SeqCst), 1, "移除前应触发一次");
    tracing::info!("Act: 调用 remove_plugin_listeners(plugin-a)，再次 emit_sync");
    bus.remove_plugin_listeners("plugin-a");
    bus.emit_sync("session.start", EventContext::new("session.start", serde_json::json!({})))?;
    tracing::info!("Assert: 验证移除后该插件回调不再触发");
    assert_eq!(
        count.load(Ordering::SeqCst),
        1,
        "remove_plugin_listeners 后不应再触发该插件回调"
    );

    Ok(())
}
