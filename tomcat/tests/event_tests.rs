//! 集成测试：事件总线（DefaultEventBus）on/emit_sync/off 与 remove_plugin_listeners 行为。
//! 验证插件卸载后事件监听彻底释放，符合 INTEGRATION_TEST_PRACTICE 场景 B。

mod common;

use tomcat::{DefaultEventBus, EventBus, EventContext};
use std::sync::atomic::{AtomicU32, Ordering};

/// [on + emit_sync] 注册回调后 emit_sync 触发一次
///
/// 验证：emit_sync 后计数器为 1
/// 意义：事件总线基本契约——on 注册的回调可被 emit_sync 同步触发
#[test]
fn test_event_bus_on_emit_sync_invokes_callback() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = tracing::info_span!("test_event_bus_on_emit_sync_invokes_callback").entered();

    let bus = DefaultEventBus::new();
    let count = std::sync::Arc::new(AtomicU32::new(0));

    let c = std::sync::Arc::clone(&count);
    bus.on(
        "test.event",
        Box::new(move |_ctx| {
            c.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }),
    );
    tracing::info!("Arrange: 创建 DefaultEventBus，注册 test.event 回调");
    let ctx = EventContext::new("test.event", serde_json::json!({}));
    tracing::info!("Act: 调用 emit_sync(test.event, ctx)");
    bus.emit_sync("test.event", ctx)?;
    tracing::info!("Assert: 验证回调被触发一次");
    assert_eq!(count.load(Ordering::SeqCst), 1, "emit_sync 应触发回调一次");

    Ok(())
}

/// [off 取消监听] off 后再 emit_sync 不再触发回调
///
/// 验证：off 后 emit_sync 不增加计数器
/// 意义：事件总线资源释放——off 可移除指定监听，防止内存泄漏
#[test]
fn test_event_bus_off_removes_listener() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = tracing::info_span!("test_event_bus_off_removes_listener").entered();

    let bus = DefaultEventBus::new();
    let count = std::sync::Arc::new(AtomicU32::new(0));

    let c = std::sync::Arc::clone(&count);
    let id = bus.on(
        "off.test",
        Box::new(move |_ctx| {
            c.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }),
    );
    tracing::info!("Arrange: 创建 DefaultEventBus，注册 off.test 回调");
    bus.emit_sync(
        "off.test",
        EventContext::new("off.test", serde_json::json!({})),
    )?;
    assert_eq!(count.load(Ordering::SeqCst), 1);
    tracing::info!("Act: 调用 off(id)，再次 emit_sync");
    bus.off(id);
    bus.emit_sync(
        "off.test",
        EventContext::new("off.test", serde_json::json!({})),
    )?;
    tracing::info!("Assert: 验证 off 后不再触发");
    assert_eq!(count.load(Ordering::SeqCst), 1, "off 后不应再触发");

    Ok(())
}

/// [remove_plugin_listeners] 移除指定插件的所有监听
///
/// 验证：remove_plugin_listeners 后该插件回调不再触发
/// 意义：插件卸载安全——卸载时一次性清除该插件的全部监听（INTEGRATION_TEST_PRACTICE 场景 B）
#[test]
fn test_event_bus_remove_plugin_listeners_cleans_up_after_unload(
) -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span =
        tracing::info_span!("test_event_bus_remove_plugin_listeners_cleans_up_after_unload")
            .entered();

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
    bus.emit_sync(
        "session.start",
        EventContext::new("session.start", serde_json::json!({})),
    )?;
    assert_eq!(count.load(Ordering::SeqCst), 1, "移除前应触发一次");
    tracing::info!("Act: 调用 remove_plugin_listeners(plugin-a)，再次 emit_sync");
    bus.remove_plugin_listeners("plugin-a");
    bus.emit_sync(
        "session.start",
        EventContext::new("session.start", serde_json::json!({})),
    )?;
    tracing::info!("Assert: 验证移除后该插件回调不再触发");
    assert_eq!(
        count.load(Ordering::SeqCst),
        1,
        "remove_plugin_listeners 后不应再触发该插件回调"
    );

    Ok(())
}

/// [once 语义] add_listener(once=true) 触发一次后自动移除
///
/// 验证：第一次 emit_sync 后计数器为 1，第二次 emit_sync 后仍为 1
/// 意义：事件总线 once 契约——一次性监听仅触发一次，防止重复处理
#[test]
fn test_event_bus_once_fires_then_auto_removes() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = tracing::info_span!("test_event_bus_once_fires_then_auto_removes").entered();

    let bus = DefaultEventBus::new();
    let count = std::sync::Arc::new(AtomicU32::new(0));

    let c = std::sync::Arc::clone(&count);
    bus.add_listener(
        "once.event",
        true,
        None,
        0,
        Box::new(move |_ctx| {
            c.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }),
    );
    tracing::info!("Arrange: add_listener(once=true)");

    bus.emit_sync(
        "once.event",
        EventContext::new("once.event", serde_json::json!({})),
    )?;
    assert_eq!(count.load(Ordering::SeqCst), 1, "首次 emit 应触发回调");

    tracing::info!("Act: 第二次 emit_sync");
    bus.emit_sync(
        "once.event",
        EventContext::new("once.event", serde_json::json!({})),
    )?;
    tracing::info!("Assert: once 回调不再触发，计数仍为 1");
    assert_eq!(
        count.load(Ordering::SeqCst),
        1,
        "once 回调第二次 emit 不应再触发"
    );

    Ok(())
}
