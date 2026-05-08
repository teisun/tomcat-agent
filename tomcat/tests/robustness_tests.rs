//! 集成测试：鲁棒性保障（INTEGRATION_TEST_ROBUSTNESS）。
//! 覆盖契约边界（脏数据/非法输入）、错误分类断言、重复加载卸载（资源/状态一致性）。
//! 超时控制见 llm_tests；路径白名单需在具备 PrimitiveExecutor 实现后补充。

mod common;

use tomcat::{
    parse_manifest, AppError, DefaultEventBus, PluginInstance, PluginManager, PluginStatus,
};
use std::sync::Arc;

/// [非法 JSON] parse_manifest 遇到非法 JSON 返回 Err 不 panic
///
/// 验证：Err(AppError::Plugin(_))，不 panic
/// 意义：鲁棒性——契约边界，脏数据不能让系统崩溃
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

/// [requiredApiVersion 为空] 缺少 requiredApiVersion 时返回 Plugin 错误
///
/// 验证：Err(AppError::Plugin(_))
/// 意义：鲁棒性——必填字段校验，防止加载不兼容插件
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

/// [main 为空] manifest.main 为空时返回 Plugin 错误
///
/// 验证：Err(AppError::Plugin(_))
/// 意义：鲁棒性——无入口脚本的插件不得加载
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

/// [重复注册卸载 50 次] 多次 register+unload 不 panic、状态一致
///
/// 验证：50 轮 register+unload 后 list_loaded 始终在 unload 后为空
/// 意义：鲁棒性——资源/状态一致性，防止内存泄漏或状态残留
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
            plugin_root: std::path::PathBuf::from("."),
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

/// [卸载不存在插件] unload_plugin 对不存在 ID 返回 Plugin 错误
///
/// 验证：Err(AppError::Plugin(_)) 且信息含"not found"或 plugin_id
/// 意义：鲁棒性——错误分类断言，确保错误类型准确
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
