use super::super::manager::PluginManager;
use super::super::types::{parse_manifest, PluginInstance, PluginManifest, PluginStatus};
use crate::infra::error::AppError;
use crate::infra::DefaultEventBus;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[test]
fn parse_manifest_valid() {
    let json = r#"{
        "id": "test-plugin",
        "name": "Test",
        "version": "0.1.0",
        "description": "d",
        "author": "a",
        "main": "index.js",
        "requiredPermissions": ["read"],
        "requiredApiVersion": "1.0",
        "tags": []
    }"#;
    let m = parse_manifest(json).unwrap();
    assert_eq!(m.id, "test-plugin");
    assert_eq!(m.required_api_version, "1.0");
}

#[test]
fn parse_manifest_missing_id_fails() {
    let json = r#"{
        "id": "",
        "name": "x",
        "version": "0.1.0",
        "description": "d",
        "author": "a",
        "main": "index.js",
        "requiredPermissions": [],
        "requiredApiVersion": "1.0",
        "tags": []
    }"#;
    let r = parse_manifest(json);
    assert!(r.is_err());
    assert!(r.unwrap_err().to_string().contains("id"));
}

#[test]
fn manager_register_and_unload() {
    let bus = Arc::new(DefaultEventBus::new());
    let manager = PluginManager::new(bus);
    let inst = PluginInstance {
        id: "p1".to_string(),
        manifest: PluginManifest {
            id: "p1".to_string(),
            name: "P1".to_string(),
            version: "0.1.0".to_string(),
            description: String::new(),
            author: String::new(),
            main: "index.js".to_string(),
            required_permissions: vec![],
            required_api_version: "1.0".to_string(),
            tags: vec![],
        },
        wasm_instance: None,
        status: PluginStatus::Loaded,
        registered_tools: vec![],
        event_listener_ids: vec![],
        config: serde_json::Value::Null,
        created_at: 0,
        loaded_at: 0,
        plugin_root: PathBuf::from("."),
    };
    manager.register_plugin(inst).unwrap();
    assert_eq!(manager.list_loaded(), vec!["p1"]);
    manager.unload_plugin("p1").unwrap();
    assert!(manager.list_loaded().is_empty());
}

#[test]
fn get_plugin_returns_some_after_register_none_for_unknown() {
    let bus = Arc::new(DefaultEventBus::new());
    let manager = PluginManager::new(bus);
    let inst = PluginInstance {
        id: "p-get".to_string(),
        manifest: PluginManifest {
            id: "p-get".to_string(),
            name: "PGet".to_string(),
            version: "0.1.0".to_string(),
            description: String::new(),
            author: String::new(),
            main: "index.js".to_string(),
            required_permissions: vec![],
            required_api_version: "1.0".to_string(),
            tags: vec![],
        },
        wasm_instance: None,
        status: PluginStatus::Loaded,
        registered_tools: vec![],
        event_listener_ids: vec![],
        config: serde_json::Value::Null,
        created_at: 0,
        loaded_at: 0,
        plugin_root: PathBuf::from("."),
    };
    assert!(manager.get_plugin("p-get").is_none());
    manager.register_plugin(inst).unwrap();
    let info = manager.get_plugin("p-get").unwrap();
    assert_eq!(info.id, "p-get");
    assert_eq!(info.manifest.name, "PGet");
    assert!(manager.get_plugin("unknown").is_none());
}

#[test]
fn register_plugin_duplicate_returns_err() {
    let bus = Arc::new(DefaultEventBus::new());
    let manager = PluginManager::new(bus);
    let inst = PluginInstance {
        id: "dup".to_string(),
        manifest: PluginManifest {
            id: "dup".to_string(),
            name: "Dup".to_string(),
            version: "0.1.0".to_string(),
            description: String::new(),
            author: String::new(),
            main: "index.js".to_string(),
            required_permissions: vec![],
            required_api_version: "1.0".to_string(),
            tags: vec![],
        },
        wasm_instance: None,
        status: PluginStatus::Loaded,
        registered_tools: vec![],
        event_listener_ids: vec![],
        config: serde_json::Value::Null,
        created_at: 0,
        loaded_at: 0,
        plugin_root: PathBuf::from("."),
    };
    manager.register_plugin(inst).unwrap();
    let inst2 = PluginInstance {
        id: "dup".to_string(),
        manifest: PluginManifest {
            id: "dup".to_string(),
            name: "Dup".to_string(),
            version: "0.1.0".to_string(),
            description: String::new(),
            author: String::new(),
            main: "index.js".to_string(),
            required_permissions: vec![],
            required_api_version: "1.0".to_string(),
            tags: vec![],
        },
        wasm_instance: None,
        status: PluginStatus::Loaded,
        registered_tools: vec![],
        event_listener_ids: vec![],
        config: serde_json::Value::Null,
        created_at: 0,
        loaded_at: 0,
        plugin_root: PathBuf::from("."),
    };
    let r = manager.register_plugin(inst2);
    assert!(r.is_err());
    assert!(r.unwrap_err().to_string().contains("already loaded"));
}

#[test]
fn enable_disable_changes_status() {
    let bus = Arc::new(DefaultEventBus::new());
    let manager = PluginManager::new(bus);
    let inst = PluginInstance {
        id: "p2".to_string(),
        manifest: PluginManifest {
            id: "p2".to_string(),
            name: "P2".to_string(),
            version: "0.1.0".to_string(),
            description: String::new(),
            author: String::new(),
            main: "index.js".to_string(),
            required_permissions: vec![],
            required_api_version: "1.0".to_string(),
            tags: vec![],
        },
        wasm_instance: None,
        status: PluginStatus::Loaded,
        registered_tools: vec![],
        event_listener_ids: vec![],
        config: serde_json::Value::Null,
        created_at: 0,
        loaded_at: 0,
        plugin_root: PathBuf::from("."),
    };
    manager.register_plugin(inst).unwrap();
    manager.enable_plugin("p2").unwrap();
    assert_eq!(
        manager.get_plugin("p2").map(|i| i.status).unwrap(),
        PluginStatus::Enabled
    );
    manager.disable_plugin("p2").unwrap();
    assert_eq!(
        manager.get_plugin("p2").map(|i| i.status).unwrap(),
        PluginStatus::Disabled
    );
}

#[test]
fn unload_nonexistent_returns_err() {
    let bus = Arc::new(DefaultEventBus::new());
    let manager = PluginManager::new(bus);
    let r = manager.unload_plugin("nonexistent");
    assert!(r.is_err());
}

#[test]
fn enable_plugin_not_found_returns_err() {
    let bus = Arc::new(DefaultEventBus::new());
    let manager = PluginManager::new(bus);
    let r = manager.enable_plugin("nonexistent");
    assert!(r.is_err());
    assert!(r.unwrap_err().to_string().contains("not found"));
}

#[test]
fn disable_plugin_not_found_returns_err() {
    let bus = Arc::new(DefaultEventBus::new());
    let manager = PluginManager::new(bus);
    let r = manager.disable_plugin("nonexistent");
    assert!(r.is_err());
    assert!(r.unwrap_err().to_string().contains("not found"));
}

#[test]
fn load_plugin_without_wasm_engine_returns_err() {
    let bus = Arc::new(DefaultEventBus::new());
    let manager = PluginManager::new(bus);
    let tmp = tempfile::tempdir().unwrap();
    let plugin_json = r#"{"id":"x","name":"X","version":"0.1.0","description":"","author":"","main":"index.js","requiredPermissions":[],"requiredApiVersion":"1.0","tags":[]}"#;
    std::fs::write(tmp.path().join("plugin.json"), plugin_json).unwrap();
    std::fs::write(tmp.path().join("index.js"), "// empty").unwrap();
    let r = manager.load_plugin(tmp.path());
    assert!(r.is_err());
    assert!(r.unwrap_err().to_string().contains("set_wasm_engine"));
}

#[test]
fn load_plugin_nonexistent_path_returns_err() {
    let bus = Arc::new(DefaultEventBus::new());
    let manager = PluginManager::new(bus);
    let r = manager.load_plugin(Path::new("/nonexistent/dir/12345"));
    assert!(r.is_err());
    assert!(matches!(r.unwrap_err(), AppError::Plugin(_)));
}

#[test]
fn load_plugin_dir_without_manifest_returns_err() {
    let bus = Arc::new(DefaultEventBus::new());
    let mut manager = PluginManager::new(bus);
    let _ = crate::ext::WasmEngine::global(None).map(|e| manager.set_wasm_engine(e));
    let tmp = tempfile::tempdir().unwrap();
    let r = manager.load_plugin(tmp.path());
    assert!(r.is_err());
    let err = r.unwrap_err();
    assert!(
        err.to_string().contains("plugin.json")
            || err.to_string().contains("pi-plugin")
            || err.to_string().contains("未找到")
    );
}

#[test]
fn load_plugin_user_deny_returns_permission_err() {
    let bus = Arc::new(DefaultEventBus::new());
    let mut manager = PluginManager::new(bus);
    let engine = match crate::ext::WasmEngine::global(None) {
        Ok(e) => e,
        Err(_) => return,
    };
    manager.set_wasm_engine(engine);
    manager.set_confirm_permissions(Arc::new(|_| Ok(false)));

    let tmp = tempfile::tempdir().unwrap();
    let plugin_json = r#"{
        "id": "deny-test",
        "name": "DenyTest",
        "version": "0.1.0",
        "description": "",
        "author": "",
        "main": "index.js",
        "requiredPermissions": [],
        "requiredApiVersion": "1.0",
        "tags": []
    }"#;
    std::fs::write(tmp.path().join("plugin.json"), plugin_json).unwrap();
    std::fs::write(tmp.path().join("index.js"), "// empty").unwrap();

    let r = manager.load_plugin(tmp.path());
    assert!(r.is_err());
    let err = r.unwrap_err();
    assert!(matches!(err, AppError::Permission(_)));
    assert!(err.to_string().contains("拒绝"));
}
