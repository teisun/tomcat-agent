//! # 事件通道：注册 / 投递 / 等待 / 清理
//!
//! 覆盖 `register_event_channel`、`deliver_event`、`do_wait_for_event`、
//! `cleanup_instance` 之间的契约：
//!
//! - 正常注册后投递事件不报错；
//! - 反压：缓冲打满后 `deliver_event` 返回错误；
//! - `do_wait_for_event` 能从 channel 中收到消息；
//! - sender 主动关闭后，`do_wait_for_event` 返回 `__shutdown` 占位；
//! - `cleanup_instance` 同时清理 event channel；
//! - 未注册的 instance 投递事件直接返回 Err。

use std::sync::Arc;

use super::super::HostApiDispatcher;
use crate::ext::vm_actor::EventEnvelope;
use crate::infra::wire;

#[test]
fn register_event_channel_and_deliver() {
    let bus = Arc::new(crate::infra::DefaultEventBus::new());
    let d = HostApiDispatcher::new(bus);
    d.register_event_channel("s1/p1", 4);

    let envelope = EventEnvelope {
        event_type: "test_event".into(),
        data: serde_json::json!({"key": "val"}),
        context: serde_json::json!({}),
    };
    d.deliver_event("s1/p1", envelope).unwrap();
}

#[test]
fn deliver_event_backpressure() {
    let bus = Arc::new(crate::infra::DefaultEventBus::new());
    let d = HostApiDispatcher::new(bus);
    d.register_event_channel("s1/p1", 2);

    for _ in 0..2 {
        d.deliver_event(
            "s1/p1",
            EventEnvelope {
                event_type: "x".into(),
                data: serde_json::json!(null),
                context: serde_json::json!(null),
            },
        )
        .unwrap();
    }

    let r = d.deliver_event(
        "s1/p1",
        EventEnvelope {
            event_type: "overflow".into(),
            data: serde_json::json!(null),
            context: serde_json::json!(null),
        },
    );
    assert!(r.is_err());
    assert!(r.unwrap_err().to_string().contains("backpressure"));
}

#[test]
fn wait_for_event_receives_delivered_event() {
    let bus = Arc::new(crate::infra::DefaultEventBus::new());
    let d = Arc::new(HostApiDispatcher::new(bus));
    d.register_event_channel("s1/p1", 4);

    d.deliver_event(
        "s1/p1",
        EventEnvelope {
            event_type: wire::vm::WIRE_SESSION_START.into(),
            data: serde_json::json!({"sid": "s1"}),
            context: serde_json::json!({}),
        },
    )
    .unwrap();

    let resp = d
        .do_wait_for_event("s1/p1", &serde_json::json!({}))
        .unwrap();
    assert!(resp.ok);
    assert_eq!(
        resp.data.as_ref().unwrap()["type"].as_str().unwrap(),
        wire::vm::WIRE_SESSION_START
    );
}

#[test]
fn wait_for_event_channel_closed_returns_shutdown() {
    let bus = Arc::new(crate::infra::DefaultEventBus::new());
    let d = Arc::new(HostApiDispatcher::new(bus));
    d.register_event_channel("s1/p1", 4);

    d.event_senders.remove("s1/p1");

    let resp = d
        .do_wait_for_event("s1/p1", &serde_json::json!({}))
        .unwrap();
    assert!(resp.ok);
    assert_eq!(resp.data.as_ref().unwrap()["type"], "__shutdown");
}

#[test]
fn cleanup_instance_removes_event_channels() {
    let bus = Arc::new(crate::infra::DefaultEventBus::new());
    let d = HostApiDispatcher::new(bus);
    d.register_event_channel("s1/p1", 4);
    assert!(d.get_event_sender("s1/p1").is_some());

    d.cleanup_instance("s1/p1");
    assert!(d.get_event_sender("s1/p1").is_none());
}

#[test]
fn deliver_event_no_channel_returns_err() {
    let bus = Arc::new(crate::infra::DefaultEventBus::new());
    let d = HostApiDispatcher::new(bus);
    let r = d.deliver_event(
        "nonexistent",
        EventEnvelope {
            event_type: "x".into(),
            data: serde_json::json!(null),
            context: serde_json::json!(null),
        },
    );
    assert!(r.is_err());
}
