//! 集成测试：插件清单解析与插件管理器（parse_manifest、PluginManager）与 EventBus 协作。
//! 黑盒测试，仅通过 tomcat 公共 API。

mod common;

use std::sync::Arc;
use tomcat::{
    parse_manifest, AppError, DefaultEventBus, PluginInstance, PluginManager, PluginStatus,
};

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

#[test]
fn test_parse_manifest_functions_default_empty() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();

    let json = r#"{
        "id": "function-default-empty",
        "name": "Function Default Empty",
        "version": "0.1.0",
        "description": "desc",
        "author": "author",
        "main": "index.js",
        "requiredPermissions": [],
        "requiredApiVersion": "1.0",
        "tags": [],
        "tools": []
    }"#;

    let manifest = parse_manifest(json)?;
    assert!(
        manifest.functions.is_empty(),
        "未声明 functions 时应回落为 []"
    );
    Ok(())
}

#[test]
fn test_parse_manifest_function_only_plugin_allowed() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();

    let json = r#"{
        "id": "function-only",
        "name": "Function Only",
        "version": "0.1.0",
        "description": "desc",
        "author": "author",
        "main": "index.js",
        "requiredPermissions": [],
        "requiredApiVersion": "1.0",
        "tags": [],
        "tools": [],
        "functions": [
            { "point": "test.echo", "function": "echoHost" }
        ]
    }"#;

    let manifest = parse_manifest(json)?;
    assert_eq!(manifest.functions.len(), 1);
    assert_eq!(manifest.functions[0].point, "test.echo");
    assert_eq!(manifest.functions[0].function, "echoHost");
    Ok(())
}

#[test]
fn test_parse_manifest_event_only_plugin_allowed() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();

    let json = r#"{
        "id": "event-only",
        "name": "Event Only",
        "version": "0.1.0",
        "description": "desc",
        "author": "author",
        "main": "index.js",
        "requiredPermissions": [],
        "requiredApiVersion": "1.0",
        "tags": [],
        "tools": [],
        "functions": [],
        "events": ["session_start"]
    }"#;

    let manifest = parse_manifest(json)?;
    assert!(manifest.tools.is_empty());
    assert!(manifest.functions.is_empty());
    assert_eq!(manifest.events, vec!["session_start"]);
    Ok(())
}

#[test]
fn test_parse_manifest_function_entry_requires_non_empty_point() {
    common::setup_logging();

    let json = r#"{
        "id": "bad-point",
        "name": "Bad Point",
        "version": "0.1.0",
        "description": "desc",
        "author": "author",
        "main": "index.js",
        "requiredPermissions": [],
        "requiredApiVersion": "1.0",
        "tags": [],
        "tools": [],
        "functions": [
            { "point": "", "function": "echoHost" }
        ]
    }"#;

    let err = parse_manifest(json).expect_err("empty point should be rejected");
    assert!(err.to_string().contains("manifest.functions[].point"));
}

#[test]
fn test_parse_manifest_function_entry_requires_non_empty_function() {
    common::setup_logging();

    let json = r#"{
        "id": "bad-function",
        "name": "Bad Function",
        "version": "0.1.0",
        "description": "desc",
        "author": "author",
        "main": "index.js",
        "requiredPermissions": [],
        "requiredApiVersion": "1.0",
        "tags": [],
        "tools": [],
        "functions": [
            { "point": "test.echo", "function": "" }
        ]
    }"#;

    let err = parse_manifest(json).expect_err("empty function should be rejected");
    assert!(err.to_string().contains("manifest.functions[].function"));
}

#[test]
fn test_parse_manifest_unknown_point_retained() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();

    let json = r#"{
        "id": "unknown-point",
        "name": "Unknown Point",
        "version": "0.1.0",
        "description": "desc",
        "author": "author",
        "main": "index.js",
        "requiredPermissions": [],
        "requiredApiVersion": "1.0",
        "tags": [],
        "tools": [],
        "functions": [
            { "point": "reranker.default", "function": "rankDocuments" }
        ]
    }"#;

    let manifest = parse_manifest(json)?;
    assert_eq!(manifest.functions[0].point, "reranker.default");
    assert_eq!(manifest.functions[0].function, "rankDocuments");
    Ok(())
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
        plugin_vm_instance: None,
        status: PluginStatus::Loaded,
        registered_tools: vec![],
        registered_functions: vec![],
        registered_commands: vec![],
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
