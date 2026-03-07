//! 集成测试：鲁棒性保障（INTEGRATION_TEST_ROBUSTNESS）。
//! 覆盖契约边界（脏数据/非法输入）、错误分类断言、重复加载卸载（资源/状态一致性）。
//! 超时控制见 llm_tests；路径白名单需在具备 PrimitiveExecutor 实现后补充。

mod common;

use pi_awsm::{
    parse_manifest, AppError, DefaultEventBus, PluginInstance, PluginManager, PluginStatus,
};
use std::sync::Arc;

/// 契约边界：非法 JSON 必须返回 Err，不得 panic；错误类型为 Plugin 或 Serialize。
#[test]
fn test_parse_manifest_malformed_json_returns_err_no_panic() {
    common::setup_logging();
    let _span =
        tracing::info_span!("test_parse_manifest_malformed_json_returns_err_no_panic").entered();

    tracing::info!("Arrange: 准备非法 JSON 字符串");
    let invalid = r#"{"id": "x", "name": NOPE}"#; // 无效 JSON
    let res = parse_manifest(invalid);
    tracing::info!("Act: 调用 parse_manifest(invalid)");
    tracing::info!("Assert: 返回 Err，且为 Plugin 或序列化相关错误，不 panic");
    assert!(res.is_err());
    let err = res.unwrap_err();
    assert!(
        matches!(err, AppError::Plugin(_)),
        "malformed JSON 应返回 Plugin 错误，got: {:?}",
        err
    );
}

/// 契约边界：缺少必填字段 required_api_version 时返回 Plugin 错误。
#[test]
fn test_parse_manifest_missing_required_api_version_returns_plugin_error() {
    common::setup_logging();
    let _span = tracing::info_span!(
        "test_parse_manifest_missing_required_api_version_returns_plugin_error"
    )
    .entered();

    let json = r#"{
        "id": "p",
        "name": "P",
        "version": "0.1.0",
        "description": "",
        "author": "",
        "main": "index.js",
        "requiredPermissions": [],
        "requiredApiVersion": "",
        "tags": []
    }"#;
    tracing::info!("Arrange: manifest 中 requiredApiVersion 为空");
    let res = parse_manifest(json);
    tracing::info!("Assert: 错误类型为 AppError::Plugin");
    assert!(matches!(res, Err(AppError::Plugin(_))));
}

/// 契约边界：缺少 main 时返回 Plugin 错误。
#[test]
fn test_parse_manifest_missing_main_returns_plugin_error() {
    common::setup_logging();
    let _span =
        tracing::info_span!("test_parse_manifest_missing_main_returns_plugin_error").entered();

    let json = r#"{
        "id": "p",
        "name": "P",
        "version": "0.1.0",
        "description": "",
        "author": "",
        "main": "",
        "requiredPermissions": [],
        "requiredApiVersion": "1.0",
        "tags": []
    }"#;
    let res = parse_manifest(json);
    assert!(matches!(res, Err(AppError::Plugin(_))));
}

/// 资源/状态边界：多次注册与卸载同一插件，无 panic，最终状态一致且 list 为空。
#[test]
fn test_plugin_manager_repeated_register_unload_state_consistent(
) -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span =
        tracing::info_span!("test_plugin_manager_repeated_register_unload_state_consistent")
            .entered();

    let bus = Arc::new(DefaultEventBus::new());
    let manager = PluginManager::new(bus);
    let manifest = parse_manifest(
        r#"{
        "id": "stress",
        "name": "Stress",
        "version": "0.1.0",
        "description": "",
        "author": "",
        "main": "index.js",
        "requiredPermissions": [],
        "requiredApiVersion": "1.0",
        "tags": []
    }"#,
    )?;

    tracing::info!("Arrange: PluginManager + 合法 manifest");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis() as i64;

    for i in 0..50 {
        let instance = PluginInstance {
            id: "stress".to_string(),
            manifest: manifest.clone(),
            wasm_instance: None,
            status: PluginStatus::Loaded,
            registered_tools: vec![],
            event_listener_ids: vec![],
            config: serde_json::json!({}),
            created_at: now + i as i64,
            loaded_at: now + i as i64,
        };
        manager.register_plugin(instance)?;
        assert_eq!(manager.list_loaded(), vec!["stress".to_string()]);
        manager.unload_plugin("stress")?;
        assert!(
            manager.list_loaded().is_empty(),
            "第 {} 次 unload 后 list_loaded 应为空",
            i + 1
        );
    }
    tracing::info!("Assert: 50 次 register+unload 无 panic，list_loaded 始终在 unload 后为空");
    Ok(())
}

/// 错误分类断言：卸载不存在的插件必须返回 Plugin 错误且消息包含 not found。
#[test]
fn test_unload_nonexistent_plugin_returns_plugin_error() {
    common::setup_logging();
    let _span =
        tracing::info_span!("test_unload_nonexistent_plugin_returns_plugin_error").entered();

    let bus = Arc::new(DefaultEventBus::new());
    let manager = PluginManager::new(bus);
    let res = manager.unload_plugin("nonexistent");
    assert!(matches!(res, Err(AppError::Plugin(_))));
    let err = res.unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("not found")
            || err.to_string().contains("nonexistent"),
        "错误信息应包含 not found 或 plugin_id，got: {}",
        err
    );
}
