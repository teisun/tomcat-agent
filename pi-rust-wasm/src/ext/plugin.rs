//! # 插件生命周期管理（与 design CODE_BLOCK_P1_008 / P1_009 一致）
//!
//! PluginManifest、PluginInstance、PluginStatus、加载/启用/禁用/卸载及与 EventBus、ToolRegistry 的清理对接。

use crate::core::ToolRegistry;
use crate::infra::error::AppError;
use crate::infra::event_bus::{EventBus, EventListenerId};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use super::WasmInstance;

/// 插件清单（与 design CODE_BLOCK_P1_008 一致）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: String,
    pub main: String,
    pub required_permissions: Vec<String>,
    pub required_api_version: String,
    pub tags: Vec<String>,
}

/// 插件运行状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PluginStatus {
    Unloaded,
    Loading,
    Loaded,
    Enabled,
    Disabled,
    Error,
}

/// 单插件实例：持有 Wasm 实例、状态、注册的工具与事件监听 ID，卸载时清理。
#[derive(Debug)]
pub struct PluginInstance {
    pub id: String,
    pub manifest: PluginManifest,
    pub wasm_instance: Option<WasmInstance>,
    pub status: PluginStatus,
    pub registered_tools: Vec<String>,
    pub event_listener_ids: Vec<EventListenerId>,
    pub config: serde_json::Value,
    pub created_at: i64,
    pub loaded_at: i64,
}

/// 只读插件信息（不含 Wasm 实例），用于 get_plugin 等查询。
#[derive(Debug, Clone)]
pub struct PluginInfo {
    pub id: String,
    pub manifest: PluginManifest,
    pub status: PluginStatus,
    pub registered_tools: Vec<String>,
    pub config: serde_json::Value,
    pub created_at: i64,
    pub loaded_at: i64,
}

impl PluginInstance {
    pub fn plugin_id(&self) -> &str {
        &self.id
    }

    pub fn to_info(&self) -> PluginInfo {
        PluginInfo {
            id: self.id.clone(),
            manifest: self.manifest.clone(),
            status: self.status,
            registered_tools: self.registered_tools.clone(),
            config: self.config.clone(),
            created_at: self.created_at,
            loaded_at: self.loaded_at,
        }
    }
}

/// 清单解析与校验：必填字段、required_api_version、required_permissions。
pub fn parse_manifest(json: &str) -> Result<PluginManifest, AppError> {
    let m: PluginManifest = serde_json::from_str(json)
        .map_err(|e| AppError::Plugin(format!("manifest parse error: {}", e)))?;
    validate_manifest(&m)?;
    Ok(m)
}

/// 校验必填字段与权限格式。
fn validate_manifest(m: &PluginManifest) -> Result<(), AppError> {
    if m.id.is_empty() {
        return Err(AppError::Plugin("manifest.id is required".to_string()));
    }
    if m.name.is_empty() {
        return Err(AppError::Plugin("manifest.name is required".to_string()));
    }
    if m.main.is_empty() {
        return Err(AppError::Plugin("manifest.main is required".to_string()));
    }
    if m.required_api_version.is_empty() {
        return Err(AppError::Plugin(
            "manifest.required_api_version is required".to_string(),
        ));
    }
    Ok(())
}

/// 插件管理器：加载/卸载/启用/禁用，卸载时调用 EventBus.remove_plugin_listeners、ToolRegistry.unregister_plugin_tools。
pub struct PluginManager {
    event_bus: Arc<dyn EventBus>,
    tools: Option<Arc<dyn ToolRegistry>>,
    plugins: std::sync::RwLock<HashMap<String, PluginInstance>>,
}

impl PluginManager {
    pub fn new(event_bus: Arc<dyn EventBus>) -> Self {
        Self {
            event_bus,
            tools: None,
            plugins: std::sync::RwLock::new(HashMap::new()),
        }
    }

    /// 注入 ToolRegistry（006 就绪后调用）；卸载时用于 unregister_plugin_tools。
    pub fn set_tool_registry(&mut self, t: Arc<dyn ToolRegistry>) {
        self.tools = Some(t);
    }

    /// 注册已构造的插件实例（内部使用；加载流程完成后调用）。
    pub fn register_plugin(&self, instance: PluginInstance) -> Result<(), AppError> {
        let id = instance.id.clone();
        let mut map = self
            .plugins
            .write()
            .map_err(|e| AppError::Plugin(format!("plugins lock poisoned: {}", e)))?;
        if map.contains_key(&id) {
            return Err(AppError::Plugin(format!("plugin already loaded: {}", id)));
        }
        map.insert(id, instance);
        Ok(())
    }

    /// 按 ID 获取插件信息（只读，不含 Wasm 实例）。
    pub fn get_plugin(&self, plugin_id: &str) -> Option<PluginInfo> {
        let map = self.plugins.read().ok()?;
        map.get(plugin_id).map(|i| i.to_info())
    }

    /// 列出已加载插件 ID。
    pub fn list_loaded(&self) -> Vec<String> {
        let map = self.plugins.read().unwrap_or_else(|e| e.into_inner());
        map.keys().cloned().collect()
    }

    /// 启用插件：仅改状态。
    pub fn enable_plugin(&self, plugin_id: &str) -> Result<(), AppError> {
        let mut map = self
            .plugins
            .write()
            .map_err(|e| AppError::Plugin(format!("plugins lock poisoned: {}", e)))?;
        let inst = map
            .get_mut(plugin_id)
            .ok_or_else(|| AppError::Plugin(format!("plugin not found: {}", plugin_id)))?;
        inst.status = PluginStatus::Enabled;
        Ok(())
    }

    /// 禁用插件：仅改状态。
    pub fn disable_plugin(&self, plugin_id: &str) -> Result<(), AppError> {
        let mut map = self
            .plugins
            .write()
            .map_err(|e| AppError::Plugin(format!("plugins lock poisoned: {}", e)))?;
        let inst = map
            .get_mut(plugin_id)
            .ok_or_else(|| AppError::Plugin(format!("plugin not found: {}", plugin_id)))?;
        inst.status = PluginStatus::Disabled;
        Ok(())
    }

    /// 卸载：移除事件监听、注销工具、销毁 Wasm 实例、从 map 移除。
    pub fn unload_plugin(&self, plugin_id: &str) -> Result<(), AppError> {
        let instance = {
            let mut map = self
                .plugins
                .write()
                .map_err(|e| AppError::Plugin(format!("plugins lock poisoned: {}", e)))?;
            map.remove(plugin_id)
                .ok_or_else(|| AppError::Plugin(format!("plugin not found: {}", plugin_id)))?
        };

        self.event_bus.remove_plugin_listeners(plugin_id);
        if let Some(ref t) = self.tools {
            t.unregister_plugin_tools(plugin_id);
        }
        if let Some(wasm) = instance.wasm_instance {
            wasm.destroy();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::DefaultEventBus;

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
}
