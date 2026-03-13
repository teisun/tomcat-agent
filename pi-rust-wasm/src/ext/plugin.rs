//! # 插件生命周期管理（与 design CODE_BLOCK_P1_008 / P1_009 一致）
//!
//! PluginManifest、PluginInstance、PluginStatus、加载/启用/禁用/卸载及与 EventBus、ToolRegistry 的清理对接。

use crate::core::ToolRegistry;
use crate::infra::audit::{AuditRecorder, PluginLifecycleAuditEntry};
use crate::infra::error::AppError;
use crate::infra::event_bus::{EventBus, EventListenerId};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::{invoke_host_func_with, HostApiDispatcher, WasmEngine, WasmInstance};

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
    /// 插件根目录路径，用于解析 main 入口与 dispatch_event 时定位脚本。
    pub plugin_root: PathBuf,
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

    /// 返回插件 main 入口脚本的绝对路径，供 dispatch_event 等使用。
    pub fn main_script_path(&self) -> PathBuf {
        self.plugin_root.join(&self.manifest.main)
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

/// 用户确认插件权限的回调：传入清单，返回 Ok(true) 同意、Ok(false) 拒绝、Err 表示确认过程出错。
pub type ConfirmPermissionsFn = dyn Fn(&PluginManifest) -> Result<bool, AppError> + Send + Sync;

/// 插件管理器：加载/卸载/启用/禁用，卸载时调用 EventBus.remove_plugin_listeners、ToolRegistry.unregister_plugin_tools。
pub struct PluginManager {
    event_bus: Arc<dyn EventBus>,
    tools: Option<Arc<dyn ToolRegistry>>,
    plugins: RwLock<HashMap<String, PluginInstance>>,
    /// 用于 load_plugin 时创建 Wasm 实例；未设置时 load_plugin 返回错误。
    wasm_engine: Option<Arc<WasmEngine>>,
    /// 用于 load_plugin 时注册 host binding；未设置时仍可加载，host 调用走桩响应。
    host_dispatcher: Option<Arc<HostApiDispatcher>>,
    /// 加载前用户确认权限；未设置时视为自动同意（或由调用方在 load 前自行确认）。
    confirm_permissions: Option<Arc<ConfirmPermissionsFn>>,
    /// 审计记录器；未设置时不写插件生命周期审计。
    audit: Option<Arc<dyn AuditRecorder>>,
}

impl PluginManager {
    pub fn new(event_bus: Arc<dyn EventBus>) -> Self {
        Self {
            event_bus,
            tools: None,
            plugins: RwLock::new(HashMap::new()),
            wasm_engine: None,
            host_dispatcher: None,
            confirm_permissions: None,
            audit: None,
        }
    }

    /// 注入 ToolRegistry（006 就绪后调用）；卸载时用于 unregister_plugin_tools。
    pub fn set_tool_registry(&mut self, t: Arc<dyn ToolRegistry>) {
        self.tools = Some(t);
    }

    /// 注入审计记录器；未设置时 load/enable/disable/unload 不写审计。
    pub fn set_audit_recorder(&mut self, a: Arc<dyn AuditRecorder>) {
        self.audit = Some(a);
    }

    /// 注入 WasmEngine；load_plugin 前必须设置，否则加载返回错误。
    pub fn set_wasm_engine(&mut self, engine: Arc<WasmEngine>) {
        self.wasm_engine = Some(engine);
    }

    /// 注入 HostApiDispatcher；未设置时 load_plugin 仍可执行，插件内 host 调用走桩响应。
    pub fn set_host_dispatcher(&mut self, dispatcher: Arc<HostApiDispatcher>) {
        self.host_dispatcher = Some(dispatcher);
    }

    /// 注入权限确认回调；未设置时 load_plugin 不调用确认、视为同意。
    pub fn set_confirm_permissions(&mut self, f: Arc<ConfirmPermissionsFn>) {
        self.confirm_permissions = Some(f);
    }

    /// 从磁盘路径完整加载插件：读清单与 main → 权限校验与用户确认 → 创建 Wasm 实例 → 注册宿主 API → 执行初始化代码 → 注册到管理器。
    ///
    /// `path` 可为插件根目录（其下需有 plugin.json 或 pi-plugin.json）或清单文件路径。
    /// 调用前须已通过 [`set_wasm_engine`] 注入引擎；[`set_host_dispatcher`] 与 [`set_confirm_permissions`] 可选。
    ///
    /// # Errors
    /// * 未设置 wasm_engine、路径无效、清单解析失败、main 读取失败、用户拒绝权限、Wasm/QuickJS 执行失败时返回对应错误。
    pub fn load_plugin(&self, path: impl AsRef<Path>) -> Result<(), AppError> {
        let path = path.as_ref();
        let (plugin_root, manifest) = match self.resolve_manifest_and_root(path) {
            Ok(t) => t,
            Err(e) => {
                if let Some(ref a) = self.audit {
                    a.record_plugin_lifecycle(PluginLifecycleAuditEntry {
                        plugin_id: path.to_string_lossy().to_string(),
                        action: "load".to_string(),
                        success: false,
                        detail: Some(e.to_string()),
                    });
                }
                return Err(e);
            }
        };
        let plugin_code = match self.read_main_script(&plugin_root, &manifest) {
            Ok(c) => c,
            Err(e) => {
                if let Some(ref a) = self.audit {
                    a.record_plugin_lifecycle(PluginLifecycleAuditEntry {
                        plugin_id: manifest.id.clone(),
                        action: "load".to_string(),
                        success: false,
                        detail: Some(e.to_string()),
                    });
                }
                return Err(e);
            }
        };

        if let Some(ref confirm) = self.confirm_permissions {
            let ok = confirm(&manifest)
                .map_err(|e| AppError::Permission(format!("权限确认失败: {}", e)))?;
            if !ok {
                if let Some(ref a) = self.audit {
                    a.record_plugin_lifecycle(PluginLifecycleAuditEntry {
                        plugin_id: manifest.id.clone(),
                        action: "load".to_string(),
                        success: false,
                        detail: Some("用户拒绝插件授权".to_string()),
                    });
                }
                return Err(AppError::Permission("用户拒绝插件授权".to_string()));
            }
        }

        let engine = self.wasm_engine.as_ref().ok_or_else(|| {
            AppError::Plugin("load_plugin 需要先调用 set_wasm_engine 注入引擎".to_string())
        })?;

        let mut instance = match engine.create_instance(&manifest.id) {
            Ok(i) => i,
            Err(e) => {
                let err = AppError::Plugin(format!("创建 Wasm 实例失败: {}", e));
                if let Some(ref a) = self.audit {
                    a.record_plugin_lifecycle(PluginLifecycleAuditEntry {
                        plugin_id: manifest.id.clone(),
                        action: "load".to_string(),
                        success: false,
                        detail: Some(err.to_string()),
                    });
                }
                return Err(err);
            }
        };

        let instance_id = manifest.id.clone();
        let dispatcher_opt = self.host_dispatcher.clone();
        let invoke_fn = move |request_json: &str| {
            let resp =
                invoke_host_func_with(dispatcher_opt.as_deref(), &instance_id, request_json)?;
            serde_json::to_string(&resp).map_err(AppError::from)
        };
        if let Err(e) = instance.register_host_binding(invoke_fn) {
            instance.destroy();
            let err = AppError::Plugin(format!("注册 host binding 失败: {}", e));
            if let Some(ref a) = self.audit {
                a.record_plugin_lifecycle(PluginLifecycleAuditEntry {
                    plugin_id: manifest.id.clone(),
                    action: "load".to_string(),
                    success: false,
                    detail: Some(err.to_string()),
                });
            }
            return Err(err);
        }

        if let Err(e) = instance.run_script(&plugin_code) {
            instance.destroy();
            let err = AppError::Plugin(format!("插件初始化脚本执行失败: {}", e));
            if let Some(ref a) = self.audit {
                a.record_plugin_lifecycle(PluginLifecycleAuditEntry {
                    plugin_id: manifest.id.clone(),
                    action: "load".to_string(),
                    success: false,
                    detail: Some(err.to_string()),
                });
            }
            return Err(err);
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let plugin_instance = PluginInstance {
            id: manifest.id.clone(),
            manifest: manifest.clone(),
            wasm_instance: Some(instance),
            status: PluginStatus::Loaded,
            registered_tools: vec![],
            event_listener_ids: vec![],
            config: serde_json::Value::Null,
            created_at: now,
            loaded_at: now,
            plugin_root: plugin_root.clone(),
        };
        if let Err(e) = self.register_plugin(plugin_instance) {
            if let Some(ref a) = self.audit {
                a.record_plugin_lifecycle(PluginLifecycleAuditEntry {
                    plugin_id: manifest.id.clone(),
                    action: "load".to_string(),
                    success: false,
                    detail: Some(e.to_string()),
                });
            }
            return Err(e);
        }
        if let Err(e) = self.enable_plugin(&manifest.id) {
            if let Some(ref a) = self.audit {
                a.record_plugin_lifecycle(PluginLifecycleAuditEntry {
                    plugin_id: manifest.id.clone(),
                    action: "load".to_string(),
                    success: false,
                    detail: Some(e.to_string()),
                });
            }
            return Err(e);
        }
        if let Some(ref a) = self.audit {
            a.record_plugin_lifecycle(PluginLifecycleAuditEntry {
                plugin_id: manifest.id.clone(),
                action: "load".to_string(),
                success: true,
                detail: None,
            });
        }
        Ok(())
    }

    /// 解析路径得到插件根目录与清单；path 为目录时在其下查找 plugin.json / pi-plugin.json，为文件时视为清单路径。
    fn resolve_manifest_and_root(
        &self,
        path: &Path,
    ) -> Result<(PathBuf, PluginManifest), AppError> {
        let (root, manifest_path) = if path.is_dir() {
            let root = path
                .canonicalize()
                .map_err(|e| AppError::Plugin(format!("插件目录无效: {}", e)))?;
            let manifest_path = root
                .join("plugin.json")
                .canonicalize()
                .or_else(|_| root.join("pi-plugin.json").canonicalize())
                .map_err(|_| {
                    AppError::Plugin("插件目录下未找到 plugin.json 或 pi-plugin.json".to_string())
                })?;
            (root, manifest_path)
        } else {
            let manifest_path = path
                .canonicalize()
                .map_err(|e| AppError::Plugin(format!("清单文件无效: {}", e)))?;
            let root = manifest_path
                .parent()
                .ok_or_else(|| AppError::Plugin("清单路径无父目录".to_string()))?
                .canonicalize()
                .map_err(|e| AppError::Plugin(format!("插件根目录无效: {}", e)))?;
            (root, manifest_path)
        };
        let json = std::fs::read_to_string(&manifest_path)
            .map_err(|e| AppError::Plugin(format!("读取清单失败: {}", e)))?;
        let manifest = parse_manifest(&json)?;
        Ok((root, manifest))
    }

    /// 读取 main 入口脚本内容；校验 main 路径不逃逸出插件根目录。
    fn read_main_script(
        &self,
        plugin_root: &Path,
        manifest: &PluginManifest,
    ) -> Result<String, AppError> {
        let main_path = plugin_root.join(&manifest.main);
        let main_path = main_path.canonicalize().map_err(|e| {
            AppError::Plugin(format!(
                "main 入口文件无效或不存在: {} ({}): {}",
                manifest.main,
                main_path.display(),
                e
            ))
        })?;
        let root_canon = plugin_root.canonicalize().map_err(AppError::Io)?;
        if !main_path.starts_with(&root_canon) {
            return Err(AppError::Permission(format!(
                "main 路径不得超出插件根目录: {}",
                main_path.display()
            )));
        }
        std::fs::read_to_string(&main_path)
            .map_err(|e| AppError::Plugin(format!("读取 main 脚本失败: {}", e)))
    }

    /// 注册已构造的插件实例（内部使用；加载流程完成后调用）。
    pub fn register_plugin(&self, instance: PluginInstance) -> Result<(), AppError> {
        let id = instance.id.clone();
        let mut map = self.plugins.write();
        if map.contains_key(&id) {
            return Err(AppError::Plugin(format!("plugin already loaded: {}", id)));
        }
        map.insert(id, instance);
        Ok(())
    }

    /// 按 ID 获取插件信息（只读，不含 Wasm 实例）。
    pub fn get_plugin(&self, plugin_id: &str) -> Option<PluginInfo> {
        let map = self.plugins.read();
        map.get(plugin_id).map(|i| i.to_info())
    }

    /// 列出已加载插件 ID。
    pub fn list_loaded(&self) -> Vec<String> {
        let map = self.plugins.read();
        map.keys().cloned().collect()
    }

    /// 启用插件：仅改状态。
    pub fn enable_plugin(&self, plugin_id: &str) -> Result<(), AppError> {
        let mut map = self.plugins.write();
        let inst = match map.get_mut(plugin_id) {
            Some(i) => i,
            None => {
                let e = AppError::Plugin(format!("plugin not found: {}", plugin_id));
                if let Some(ref a) = self.audit {
                    a.record_plugin_lifecycle(PluginLifecycleAuditEntry {
                        plugin_id: plugin_id.to_string(),
                        action: "enable".to_string(),
                        success: false,
                        detail: Some(e.to_string()),
                    });
                }
                return Err(e);
            }
        };
        inst.status = PluginStatus::Enabled;
        if let Some(ref a) = self.audit {
            a.record_plugin_lifecycle(PluginLifecycleAuditEntry {
                plugin_id: plugin_id.to_string(),
                action: "enable".to_string(),
                success: true,
                detail: None,
            });
        }
        Ok(())
    }

    /// 禁用插件：仅改状态。
    pub fn disable_plugin(&self, plugin_id: &str) -> Result<(), AppError> {
        let mut map = self.plugins.write();
        let inst = match map.get_mut(plugin_id) {
            Some(i) => i,
            None => {
                let e = AppError::Plugin(format!("plugin not found: {}", plugin_id));
                if let Some(ref a) = self.audit {
                    a.record_plugin_lifecycle(PluginLifecycleAuditEntry {
                        plugin_id: plugin_id.to_string(),
                        action: "disable".to_string(),
                        success: false,
                        detail: Some(e.to_string()),
                    });
                }
                return Err(e);
            }
        };
        inst.status = PluginStatus::Disabled;
        if let Some(ref a) = self.audit {
            a.record_plugin_lifecycle(PluginLifecycleAuditEntry {
                plugin_id: plugin_id.to_string(),
                action: "disable".to_string(),
                success: true,
                detail: None,
            });
        }
        Ok(())
    }

    /// 卸载：移除事件监听、注销工具、销毁 Wasm 实例、从 map 移除。
    pub fn unload_plugin(&self, plugin_id: &str) -> Result<(), AppError> {
        let instance = {
            let mut map = self.plugins.write();
            match map.remove(plugin_id) {
                Some(inst) => inst,
                None => {
                    let e = AppError::Plugin(format!("plugin not found: {}", plugin_id));
                    if let Some(ref a) = self.audit {
                        a.record_plugin_lifecycle(PluginLifecycleAuditEntry {
                            plugin_id: plugin_id.to_string(),
                            action: "unload".to_string(),
                            success: false,
                            detail: Some(e.to_string()),
                        });
                    }
                    return Err(e);
                }
            }
        };

        self.event_bus.remove_plugin_listeners(plugin_id);
        if let Some(ref t) = self.tools {
            t.unregister_plugin_tools(plugin_id);
        }
        if let Some(wasm) = instance.wasm_instance {
            wasm.destroy();
        }
        if let Some(ref a) = self.audit {
            a.record_plugin_lifecycle(PluginLifecycleAuditEntry {
                plugin_id: plugin_id.to_string(),
                action: "unload".to_string(),
                success: true,
                detail: None,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::DefaultEventBus;
    use std::path::Path;

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
}
