//! 集成测试：TASK-15 长生命周期 VM — RuntimeManager、VmActorHandle、PluginManager session API。
//! 黑盒测试，仅通过 pi_wasm pub API。覆盖 5 个验收场景：
//! 1. RuntimeManager 双键管理与 session 批量清理
//! 2. VmActorHandle 命令发送与状态转换
//! 3. 多会话隔离（RuntimeManager 层面）
//! 4. session 级清理无残留
//! 5. PluginManager session VM 需依赖注入校验（无 engine 时返回明确错误）

mod common;

use pi_wasm::{
    parse_manifest, DefaultEventBus, EventEnvelope, HostApiDispatcher, PluginInstance,
    PluginManager, PluginStatus, RuntimeManager, SharedRuntimeManager, VmActorHandle, VmActorState,
    VmCommand, VmRuntimeKey,
};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

fn stub_handle() -> VmActorHandle {
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    VmActorHandle {
        cmd_tx: tx,
        state: Arc::new(AtomicU8::new(VmActorState::Created as u8)),
    }
}

fn make_manifest_json(id: &str) -> String {
    format!(
        r#"{{
        "id": "{id}",
        "name": "Test Plugin {id}",
        "version": "0.1.0",
        "description": "test",
        "author": "test",
        "main": "index.js",
        "requiredPermissions": [],
        "requiredApiVersion": "1.0",
        "tags": []
    }}"#
    )
}

fn make_plugin_instance(id: &str) -> PluginInstance {
    let manifest = parse_manifest(&make_manifest_json(id)).unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    PluginInstance {
        id: id.to_string(),
        manifest,
        wasm_instance: None,
        status: PluginStatus::Loaded,
        registered_tools: vec![],
        event_listener_ids: vec![],
        config: serde_json::json!({}),
        created_at: now,
        loaded_at: now,
        plugin_root: std::path::PathBuf::from("/tmp/fake-plugin"),
    }
}

// ---------------------------------------------------------------------------
// 场景 1：RuntimeManager 双键管理与 lookup
// ---------------------------------------------------------------------------

/// [RuntimeManager 双键 insert/get] session_id + plugin_id 精确查找
///
/// 验收标准：插件全局变量可跨事件保持 — 基础设施层面验证双键管理正确。
#[test]
fn test_runtime_manager_insert_get_by_composite_key() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = tracing::info_span!("test_runtime_manager_insert_get_by_composite_key").entered();

    let mgr = RuntimeManager::new();
    let key = VmRuntimeKey::new("session-1", "plugin-a");

    tracing::info!("Arrange: 创建 RuntimeManager 与 VmRuntimeKey(session-1/plugin-a)");
    mgr.insert(key.clone(), stub_handle());

    tracing::info!("Act: get 同 key 和不同 key");
    let found = mgr.get(&key);
    let not_found = mgr.get(&VmRuntimeKey::new("session-1", "plugin-b"));

    tracing::info!("Assert: 同 key 返回 Some，不同 key 返回 None");
    assert!(found.is_some(), "同 key 应找到 handle");
    assert!(not_found.is_none(), "不同 plugin_id 应返回 None");

    Ok(())
}

// ---------------------------------------------------------------------------
// 场景 2：VmActorHandle 状态转换
// ---------------------------------------------------------------------------

/// [VmActorHandle 状态查询] 外部可通过 handle.current_state() 感知 actor 状态
///
/// 验收标准：已注册 handler 在多次事件中持续有效 — handle 状态可正确反映 actor 生命周期。
#[test]
fn test_vm_actor_handle_state_transitions() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = tracing::info_span!("test_vm_actor_handle_state_transitions").entered();

    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let state = Arc::new(AtomicU8::new(VmActorState::Created as u8));
    let handle = VmActorHandle {
        cmd_tx: tx,
        state: state.clone(),
    };

    tracing::info!("Arrange: 创建 VmActorHandle，初始 state = Created");
    assert_eq!(handle.current_state(), VmActorState::Created);

    tracing::info!("Act: 模拟状态变更 Created → Running → Idle → Stopped");
    state.store(VmActorState::Running as u8, Ordering::Relaxed);
    assert_eq!(handle.current_state(), VmActorState::Running);

    state.store(VmActorState::Idle as u8, Ordering::Relaxed);
    assert_eq!(handle.current_state(), VmActorState::Idle);

    state.store(VmActorState::Stopped as u8, Ordering::Relaxed);
    tracing::info!("Assert: 最终 state = Stopped");
    assert_eq!(handle.current_state(), VmActorState::Stopped);

    Ok(())
}

/// [VmActorHandle dispatch] 向 handle 发送 VmCommand::DispatchEvent
///
/// 验收标准：验证 channel 通信正常，命令可到达 actor 端。
#[tokio::test]
async fn test_vm_actor_handle_dispatch_event_command() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = tracing::info_span!("test_vm_actor_handle_dispatch_event_command").entered();

    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    let handle = VmActorHandle {
        cmd_tx: tx,
        state: Arc::new(AtomicU8::new(VmActorState::Running as u8)),
    };

    tracing::info!("Arrange: 创建 VmActorHandle 并持有 rx");
    tracing::info!("Act: dispatch Init + DispatchEvent + Shutdown");

    handle.dispatch(VmCommand::Init).await?;
    handle
        .dispatch(VmCommand::DispatchEvent {
            event_type: "test_event".into(),
            data: serde_json::json!({"seq": 1}),
            context: serde_json::json!({}),
        })
        .await?;
    handle.shutdown().await?;

    tracing::info!("Assert: rx 收到 3 条命令");
    let cmd1 = rx.recv().await.unwrap();
    assert!(matches!(cmd1, VmCommand::Init));

    let cmd2 = rx.recv().await.unwrap();
    assert!(matches!(cmd2, VmCommand::DispatchEvent { .. }));

    let cmd3 = rx.recv().await.unwrap();
    assert!(matches!(cmd3, VmCommand::Shutdown));

    Ok(())
}

// ---------------------------------------------------------------------------
// 场景 3：多会话隔离
// ---------------------------------------------------------------------------

/// [多会话隔离] 不同 session_id 下同一 plugin_id 的 handle 相互独立
///
/// 验收标准：多会话上下文隔离（状态不串会话）
#[test]
fn test_multi_session_isolation_in_runtime_manager() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = tracing::info_span!("test_multi_session_isolation_in_runtime_manager").entered();

    let mgr = RuntimeManager::new();

    let key_s1 = VmRuntimeKey::new("session-A", "plugin-x");
    let key_s2 = VmRuntimeKey::new("session-B", "plugin-x");

    let (tx1, _rx1) = tokio::sync::mpsc::channel(1);
    let state1 = Arc::new(AtomicU8::new(VmActorState::Running as u8));
    let handle1 = VmActorHandle {
        cmd_tx: tx1,
        state: state1.clone(),
    };

    let (tx2, _rx2) = tokio::sync::mpsc::channel(1);
    let state2 = Arc::new(AtomicU8::new(VmActorState::Created as u8));
    let handle2 = VmActorHandle {
        cmd_tx: tx2,
        state: state2.clone(),
    };

    tracing::info!(
        "Arrange: 为 session-A/plugin-x(Running) 和 session-B/plugin-x(Created) 各创建 handle"
    );
    mgr.insert(key_s1.clone(), handle1);
    mgr.insert(key_s2.clone(), handle2);

    tracing::info!("Act: 查询两个 session 的 handle state");
    let h1 = mgr.get(&key_s1).unwrap();
    let h2 = mgr.get(&key_s2).unwrap();

    tracing::info!("Assert: s1=Running, s2=Created，互不干扰");
    assert_eq!(h1.current_state(), VmActorState::Running);
    assert_eq!(h2.current_state(), VmActorState::Created);

    state1.store(VmActorState::Stopped as u8, Ordering::Relaxed);
    assert_eq!(
        mgr.get(&key_s1).unwrap().current_state(),
        VmActorState::Stopped,
        "修改 s1 状态后 s1 反映 Stopped"
    );
    assert_eq!(
        mgr.get(&key_s2).unwrap().current_state(),
        VmActorState::Created,
        "s2 不受 s1 影响"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// 场景 4：session 级清理无残留
// ---------------------------------------------------------------------------

/// [session 清理] remove_session 后该 session 下所有 handle 被移除，其他 session 不受影响
///
/// 验收标准：关闭流程无悬挂线程、无 pending 泄漏
#[test]
fn test_session_cleanup_removes_all_handles_for_session() -> Result<(), Box<dyn std::error::Error>>
{
    common::setup_logging();
    let _span =
        tracing::info_span!("test_session_cleanup_removes_all_handles_for_session").entered();

    let mgr = RuntimeManager::new();
    mgr.insert(VmRuntimeKey::new("sess-1", "p1"), stub_handle());
    mgr.insert(VmRuntimeKey::new("sess-1", "p2"), stub_handle());
    mgr.insert(VmRuntimeKey::new("sess-1", "p3"), stub_handle());
    mgr.insert(VmRuntimeKey::new("sess-2", "p1"), stub_handle());

    tracing::info!("Arrange: sess-1 下 3 个 handle，sess-2 下 1 个 handle，共 4 个");
    assert_eq!(mgr.len(), 4);

    tracing::info!("Act: remove_session(sess-1)");
    let removed = mgr.remove_session("sess-1");

    tracing::info!("Assert: 移除 3 个，剩余 1 个（sess-2/p1）");
    assert_eq!(removed.len(), 3);
    assert_eq!(mgr.len(), 1);
    assert!(mgr.get(&VmRuntimeKey::new("sess-2", "p1")).is_some());
    assert!(mgr.get(&VmRuntimeKey::new("sess-1", "p1")).is_none());

    Ok(())
}

/// [PluginManager end_session] 结束会话后 RuntimeManager 中该 session handle 为空
///
/// 验收标准：关闭流程无悬挂线程、无 pending 泄漏 — end_session 正确清理。
#[tokio::test]
async fn test_plugin_manager_end_session_cleans_runtime_manager(
) -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span =
        tracing::info_span!("test_plugin_manager_end_session_cleans_runtime_manager").entered();

    let bus = Arc::new(DefaultEventBus::new());
    let dispatcher = Arc::new(HostApiDispatcher::new(bus.clone()));
    let rm: SharedRuntimeManager = Arc::new(RuntimeManager::new());

    let mut mgr = PluginManager::new(bus);
    mgr.set_host_dispatcher(dispatcher);
    mgr.set_runtime_manager(rm.clone());

    rm.insert(VmRuntimeKey::new("sess-x", "plugin-a"), stub_handle());
    rm.insert(VmRuntimeKey::new("sess-x", "plugin-b"), stub_handle());

    tracing::info!("Arrange: PluginManager 注入 RuntimeManager(2 个 handle) + dispatcher");
    assert_eq!(rm.len(), 2);

    tracing::info!("Act: end_session(sess-x)");
    mgr.end_session("sess-x").await?;

    tracing::info!("Assert: rm 为空，所有 handle 已清理");
    assert!(rm.is_empty(), "end_session 后 RuntimeManager 应为空");

    Ok(())
}

// ---------------------------------------------------------------------------
// 场景 5：PluginManager start_session_vm 依赖校验
// ---------------------------------------------------------------------------

/// [start_session_vm 无 engine] 未注入 WasmEngine 时调用 start_session_vm 返回明确错误
///
/// 验收标准：错误隔离——缺失依赖时不 panic，返回 AppError::Plugin
#[tokio::test]
async fn test_start_session_vm_without_engine_returns_err() -> Result<(), Box<dyn std::error::Error>>
{
    common::setup_logging();
    let _span = tracing::info_span!("test_start_session_vm_without_engine_returns_err").entered();

    let bus = Arc::new(DefaultEventBus::new());
    let rm: SharedRuntimeManager = Arc::new(RuntimeManager::new());
    let mut mgr = PluginManager::new(bus.clone());
    mgr.set_runtime_manager(rm);

    mgr.register_plugin(make_plugin_instance("test-plugin"))?;

    tracing::info!("Arrange: PluginManager 有 RuntimeManager 和已注册插件，但无 WasmEngine");
    tracing::info!("Act: start_session_vm(sess, test-plugin)");
    let result = mgr.start_session_vm("sess-1", "test-plugin").await;

    tracing::info!("Assert: 返回 Err(Plugin(...))，msg 含 wasm_engine");
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("wasm_engine"),
        "错误应提示 wasm_engine 未设置，实际: {err_msg}"
    );

    Ok(())
}

/// [start_session_vm 无 RuntimeManager] 未注入 RuntimeManager 时返回明确错误
#[tokio::test]
async fn test_start_session_vm_without_runtime_manager_returns_err(
) -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span =
        tracing::info_span!("test_start_session_vm_without_runtime_manager_returns_err").entered();

    let bus = Arc::new(DefaultEventBus::new());
    let mgr = PluginManager::new(bus);
    mgr.register_plugin(make_plugin_instance("test-plugin"))?;

    tracing::info!("Arrange: PluginManager 无 RuntimeManager");
    tracing::info!("Act: start_session_vm(sess, test-plugin)");
    let result = mgr.start_session_vm("sess-1", "test-plugin").await;

    tracing::info!("Assert: 返回 Err，msg 含 runtime_manager");
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("runtime_manager"),
        "错误应提示 runtime_manager 未设置，实际: {err_msg}"
    );

    Ok(())
}

/// [dispatch_session_event 无 dispatcher] 未注入 HostApiDispatcher 时返回错误
#[test]
fn test_dispatch_session_event_without_dispatcher_returns_err(
) -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span =
        tracing::info_span!("test_dispatch_session_event_without_dispatcher_returns_err").entered();

    let bus = Arc::new(DefaultEventBus::new());
    let mgr = PluginManager::new(bus);

    tracing::info!("Arrange: PluginManager 无 host_dispatcher");
    tracing::info!("Act: dispatch_session_event");
    let result = mgr.dispatch_session_event(
        "sess-1",
        "plugin-a",
        "test_event",
        serde_json::json!({}),
        serde_json::json!({}),
    );

    tracing::info!("Assert: 返回 Err，含 host_dispatcher 提示");
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("host_dispatcher"),
        "错误应提示 host_dispatcher 未设置，实际: {err_msg}"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// 场景补充：EventEnvelope 序列化（跨事件数据传递基础验证）
// ---------------------------------------------------------------------------

/// [EventEnvelope 序列化] 事件信封正确序列化/反序列化
#[test]
fn test_event_envelope_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = tracing::info_span!("test_event_envelope_roundtrip").entered();

    let env = EventEnvelope {
        event_type: "tool_call".into(),
        data: serde_json::json!({"tool": "read", "path": "/tmp/x"}),
        context: serde_json::json!({"session_id": "s1"}),
    };

    tracing::info!("Arrange: 构造 EventEnvelope(tool_call)");
    let json = serde_json::to_string(&env)?;
    tracing::info!("Act: 序列化 → 反序列化");
    let decoded: EventEnvelope = serde_json::from_str(&json)?;

    tracing::info!("Assert: 字段值一致");
    assert_eq!(decoded.event_type, "tool_call");
    assert_eq!(decoded.data["tool"], "read");
    assert_eq!(decoded.context["session_id"], "s1");

    Ok(())
}

// ---------------------------------------------------------------------------
// 场景补充：HostApiDispatcher 事件 channel 注册与投递
// ---------------------------------------------------------------------------

/// [deliver_event 正常投递] 注册 event channel 后 deliver_event 可投递事件
#[test]
fn test_dispatcher_event_channel_register_and_deliver() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = tracing::info_span!("test_dispatcher_event_channel_register_and_deliver").entered();

    let bus = Arc::new(DefaultEventBus::new());
    let dispatcher = HostApiDispatcher::new(bus);

    tracing::info!("Arrange: 注册 instance s1/p1 的 event channel");
    let _tx = dispatcher.register_event_channel("s1/p1", 8);

    tracing::info!("Act: deliver_event 投递一个事件");
    dispatcher.deliver_event(
        "s1/p1",
        EventEnvelope {
            event_type: "session_start".into(),
            data: serde_json::json!({}),
            context: serde_json::json!({}),
        },
    )?;

    tracing::info!("Assert: 通过 tx 对应的 rx 可接收到事件（由 register_event_channel 返回的 tx 侧验证 channel 联通）");
    // deliver_event 写入的是 dispatcher 内部维护的 tx，这里验证不返回错误即通过
    // 更深入的验证：channel 满时返回回压错误
    Ok(())
}

/// [deliver_event 回压] event channel 满时 deliver_event 返回错误
#[test]
fn test_dispatcher_deliver_event_backpressure() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = tracing::info_span!("test_dispatcher_deliver_event_backpressure").entered();

    let bus = Arc::new(DefaultEventBus::new());
    let dispatcher = HostApiDispatcher::new(bus);
    let _tx = dispatcher.register_event_channel("s1/p1", 2);

    tracing::info!("Arrange: 注册容量为 2 的 event channel");
    let envelope = || EventEnvelope {
        event_type: "tick".into(),
        data: serde_json::json!({}),
        context: serde_json::json!({}),
    };

    tracing::info!("Act: 投递 3 条事件，第 3 条应触发回压");
    dispatcher.deliver_event("s1/p1", envelope())?;
    dispatcher.deliver_event("s1/p1", envelope())?;
    let result = dispatcher.deliver_event("s1/p1", envelope());

    tracing::info!("Assert: 第 3 条返回 Err（channel 已满）");
    assert!(result.is_err(), "channel 满后 deliver_event 应返回 Err");

    Ok(())
}

/// [cleanup_instance 清理事件 channel] cleanup 后 deliver_event 返回错误
#[test]
fn test_dispatcher_cleanup_removes_event_channel() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = tracing::info_span!("test_dispatcher_cleanup_removes_event_channel").entered();

    let bus = Arc::new(DefaultEventBus::new());
    let dispatcher = HostApiDispatcher::new(bus);
    let _tx = dispatcher.register_event_channel("s1/p1", 4);

    tracing::info!("Arrange: 注册 event channel");
    dispatcher.deliver_event(
        "s1/p1",
        EventEnvelope {
            event_type: "evt".into(),
            data: serde_json::json!({}),
            context: serde_json::json!({}),
        },
    )?;

    tracing::info!("Act: cleanup_instance(s1/p1)");
    dispatcher.cleanup_instance("s1/p1");

    tracing::info!("Assert: deliver_event 返回 Err（channel 已移除）");
    let result = dispatcher.deliver_event(
        "s1/p1",
        EventEnvelope {
            event_type: "evt".into(),
            data: serde_json::json!({}),
            context: serde_json::json!({}),
        },
    );
    assert!(
        result.is_err(),
        "cleanup 后 deliver_event 应返回 Err（无 channel）"
    );

    Ok(())
}
