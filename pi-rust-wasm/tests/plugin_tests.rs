//! 集成测试：插件清单解析与插件管理器（parse_manifest、PluginManager）与 EventBus 协作。
//! 黑盒测试，仅通过 pi_wasm 公共 API。

mod common;

use pi_wasm::{
    parse_manifest, AppError, DefaultEventBus, PluginInstance, PluginManager, PluginStatus,
};
use std::sync::Arc;

/// [parse_manifest 合法] 合法 JSON 解析出完整 PluginManifest
///
/// 验证：id/name/required_api_version 字段值正确
/// 意义：TASK-06 插件清单解析——正向路径
#[test]
fn test_parse_manifest_valid_json_returns_manifest() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = tracing::info_span!("test_parse_manifest_valid_json_returns_manifest").entered();

    let json = r#"{
        "id": "test-plugin",
        "name": "Test Plugin",
        "version": "0.1.0",
        "description": "desc",
        "author": "author",
        "main": "index.js",
        "requiredPermissions": ["read"],
        "requiredApiVersion": "1.0",
        "tags": []
    }"#;
    tracing::info!("Arrange: 准备合法 JSON manifest 字符串");
    let manifest = parse_manifest(json)?;
    tracing::info!("Act: 调用 parse_manifest(json)");
    tracing::info!("Assert: 验证 manifest 字段 id、name、required_api_version");
    assert_eq!(manifest.id, "test-plugin");
    assert_eq!(manifest.name, "Test Plugin");
    assert_eq!(manifest.required_api_version, "1.0");

    Ok(())
}

/// [parse_manifest id 为空] manifest.id 为空时返回 Plugin 错误
///
/// 验证：Err(AppError::Plugin(_))
/// 意义：TASK-06 插件清单校验——必填字段缺失的边界防护
#[test]
fn test_parse_manifest_missing_id_returns_err() {
    common::setup_logging();
    let _span = tracing::info_span!("test_parse_manifest_missing_id_returns_err").entered();

    let json = r#"{
        "id": "",
        "name": "N",
        "version": "0.1.0",
        "description": "d",
        "author": "a",
        "main": "index.js",
        "requiredPermissions": [],
        "requiredApiVersion": "1.0",
        "tags": []
    }"#;
    tracing::info!("Arrange: 准备 id 为空的 JSON manifest");
    let res = parse_manifest(json);
    tracing::info!("Act: 调用 parse_manifest(json)");
    tracing::info!("Assert: 验证返回 Err 且错误类型为 Plugin（鲁棒性：错误分类断言）");
    assert!(res.is_err(), "manifest.id 为空时应返回 Err");
    assert!(
        matches!(res, Err(AppError::Plugin(_))),
        "id 为空应返回 AppError::Plugin"
    );
}

/// [PluginManager register + list] 注册插件后 list_loaded 含该插件
///
/// 验证：list_loaded 含 "p1"、get_plugin 返回 Some 且 id 正确
/// 意义：TASK-06 插件管理——注册与查询端到端
#[test]
fn test_plugin_manager_register_and_list_loaded() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = tracing::info_span!("test_plugin_manager_register_and_list_loaded").entered();

    let bus = Arc::new(DefaultEventBus::new());
    let mgr = PluginManager::new(bus);

    let manifest = parse_manifest(
        r#"{
        "id": "p1",
        "name": "P1",
        "version": "0.1.0",
        "description": "d",
        "author": "a",
        "main": "main.js",
        "requiredPermissions": [],
        "requiredApiVersion": "1.0",
        "tags": []
    }"#,
    )?;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis() as i64;
    let instance = PluginInstance {
        id: "p1".to_string(),
        manifest,
        wasm_instance: None,
        status: PluginStatus::Loaded,
        registered_tools: vec![],
        event_listener_ids: vec![],
        config: serde_json::json!({}),
        created_at: now,
        loaded_at: now,
        plugin_root: std::path::PathBuf::from("."),
    };
    tracing::info!("Arrange: 创建 DefaultEventBus、PluginManager 与 PluginInstance p1");
    mgr.register_plugin(instance)?;
    tracing::info!("Act: 调用 register_plugin、list_loaded、get_plugin");
    let list = mgr.list_loaded();
    tracing::info!("Assert: 验证 list_loaded 含 p1，get_plugin 返回 Some 且 id 为 p1");
    assert_eq!(list, vec!["p1".to_string()]);

    let info = mgr.get_plugin("p1");
    assert!(info.is_some());
    assert_eq!(info.unwrap().id, "p1");

    Ok(())
}
