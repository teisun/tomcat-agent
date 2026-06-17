//! # PluginManager 插件生命周期管理
//!
//! 插件的全生命周期管理器：从磁盘加载 manifest + 主脚本（TS→QuickJS），
//! 注册到内存表，按需启用/禁用，会话级启动专属 VmActor 并桥接 hostcall，
//! 卸载时清理所有副作用（EventBus listeners / ToolRegistry / VM / async 票据）。
//!
//! ## 状态机
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                                                                          │
//! │  ① 磁盘                                                                  │
//! │   plugin/                                                                │
//! │   ├─ plugin.json       ─┐                                                │
//! │   └─ <main>.{ts,js}   ────► load_plugin(path)                           │
//! │                                  │ resolve_manifest_and_root             │
//! │                                  │ read_main_script + transpile_ts      │
//! │                                  ▼                                      │
//! │  ② Loaded   ──────► register_plugin(PluginInstance) ──┐                  │
//! │   plugins[id] = { Loaded, manifest, ... }              │                 │
//! │                                                         ▼                │
//! │  ③ Enabled  ◄── enable_plugin(id) ──── 设 status=Enabled，未起 VM        │
//! │                                                         │                │
//! │                       start_session_vm(plugin_id, sid)  │                │
//! │                                                         ▼                │
//! │  ④ Active   VmActor + VmActorHandle 注册到 plugins[id].sessions[sid]    │
//! │                ├─ plugin_engine 创建 instance                            │
//! │                ├─ host_dispatcher 注册 EventChannel                       │
//! │                └─ confirm_permissions 可选确认（默认放行）                 │
//! │                                                         │                │
//! │              dispatch_session_event ─► VmCommand ─► VmActor ─► JS 钩子   │
//! │                                                         │                │
//! │                       end_session(sid)                  │                │
//! │                                                         ▼                │
//! │  ③ Enabled  会话 VM 退出，plugin 仍 Enabled 待下个 session                │
//! │                                                                          │
//! │  ⑤ Disabled ◄── disable_plugin(id) ──┐                                   │
//! │                                       │                                   │
//! │                  unload_plugin(id) ──┴──► 移除 plugins[id] + 全副作用清理 │
//! │                  ├─ event_bus.remove_plugin_listeners(id)                │
//! │                  ├─ tools.unregister_plugin_tools(id)                    │
//! │                  ├─ host_dispatcher.cleanup_instance(id)                 │
//! │                  └─ plugin_runtime_manager.evict(PluginRuntimeKey)       │
//! │                                                                          │
//! └─────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## 6 个可注入依赖（构造后 set_*）
//!
//! | 字段                  | 用途                                      |
//! | --------------------- | ----------------------------------------- |
//! | `tools`               | `ToolRegistry`：注册/注销插件工具         |
//! | `audit`               | `AuditRecorder`：插件生命周期审计         |
//! | `plugin_engine`       | `PluginEngine`：插件 VM 实例化与 host_func 注册 |
//! | `host_dispatcher`     | `HostApiDispatcher`：插件→宿主 hostcall    |
//! | `confirm_permissions` | 加载期权限确认扩展点（默认可不注入/默认放行） |
//! | `plugin_runtime_manager` | 复用插件 VM 实例池                    |
//!
//! ## 与同族子模块的边界
//!
//! - **本文件**：生命周期 + 实例表 + 事件分发入口。
//! - `types.rs`：`PluginInstance` / `PluginManifest` / `PluginStatus` / `PluginInfo`。
//! - 跨 actor：`vm_actor::{VmActor, VmCommand, EventEnvelope}` 提供单插件单 VM 的
//!   消息隔离；`runtime_manager` 跨插件共享插件 VM 实例。

use super::types::{
    ConfirmPermissionsFn, PluginInfo, PluginInstance, PluginManifest, PluginStatus,
};
use super::FunctionRegistry;
use crate::core::ToolRegistry;
use crate::infra::audit::{AuditRecorder, PluginLifecycleAuditEntry};
use crate::infra::error::AppError;
use crate::infra::event_bus::EventBus;
use parking_lot::RwLock;
use std::collections::{hash_map::Entry, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use crate::ext::runtime_manager::{PluginRuntimeKey, SharedPluginRuntimeManager};
use crate::ext::ts_compiler::transpile_pi_plugin_for_quickjs;
use crate::ext::vm_actor::{EventEnvelope, VmActor, VmActorHandle, VmActorState, VmCommand};
use crate::ext::{invoke_host_func_with, HostApiDispatcher, PluginEngine};

use super::types::parse_manifest;

/// 插件管理器：加载/卸载/启用/禁用，卸载时调用 EventBus.remove_plugin_listeners、ToolRegistry.unregister_plugin_tools。
pub struct PluginManager {
    event_bus: Arc<dyn EventBus>,
    tools: RwLock<Option<Arc<dyn ToolRegistry>>>,
    functions: RwLock<Option<Arc<FunctionRegistry>>>,
    plugins: RwLock<HashMap<String, PluginInstance>>,
    plugin_engine: Option<Arc<PluginEngine>>,
    host_dispatcher: RwLock<Option<Arc<HostApiDispatcher>>>,
    confirm_permissions: Option<Arc<ConfirmPermissionsFn>>,
    audit: Option<Arc<dyn AuditRecorder>>,
    plugin_runtime_manager: Option<SharedPluginRuntimeManager>,
    event_channel_capacity: usize,
}

impl PluginManager {
    pub fn new(event_bus: Arc<dyn EventBus>) -> Self {
        Self {
            event_bus,
            tools: RwLock::new(None),
            functions: RwLock::new(None),
            plugins: RwLock::new(HashMap::new()),
            plugin_engine: None,
            host_dispatcher: RwLock::new(None),
            confirm_permissions: None,
            audit: None,
            plugin_runtime_manager: None,
            event_channel_capacity: 64,
        }
    }

    /// 注入 ToolRegistry（006 就绪后调用）；卸载时用于 unregister_plugin_tools。
    pub fn set_tool_registry(&self, t: Arc<dyn ToolRegistry>) {
        *self.tools.write() = Some(t);
    }

    pub fn set_function_registry(&self, registry: Arc<FunctionRegistry>) {
        *self.functions.write() = Some(registry);
    }

    /// 注入审计记录器；未设置时 load/enable/disable/unload 不写审计。
    pub fn set_audit_recorder(&mut self, a: Arc<dyn AuditRecorder>) {
        self.audit = Some(a);
    }

    /// 注入 PluginEngine；load_plugin 前必须设置，否则加载返回错误。
    pub fn set_plugin_engine(&mut self, engine: Arc<PluginEngine>) {
        self.plugin_engine = Some(engine);
    }

    /// 注入 HostApiDispatcher；未设置时 load_plugin 仍可执行，插件内 host 调用走桩响应。
    pub fn set_host_dispatcher(&self, dispatcher: Arc<HostApiDispatcher>) {
        *self.host_dispatcher.write() = Some(dispatcher);
    }

    /// 注入权限确认回调；未设置时 load_plugin 不调用确认、视为同意。
    pub fn set_confirm_permissions(&mut self, f: Arc<ConfirmPermissionsFn>) {
        self.confirm_permissions = Some(f);
    }

    /// 从磁盘路径完整加载插件：读清单与 main → 权限校验（可选确认） → 创建插件 VM 实例 → 注册宿主 API → 执行初始化代码 → 注册到管理器。
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

        let engine = self.plugin_engine.as_ref().ok_or_else(|| {
            AppError::Plugin("load_plugin 需要先调用 set_plugin_engine 注入引擎".to_string())
        })?;

        let mut instance = match engine.create_instance(&manifest.id) {
            Ok(i) => i,
            Err(e) => {
                let err = AppError::Plugin(format!("创建插件 VM 实例失败: {}", e));
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
        let dispatcher_opt = self.host_dispatcher.read().clone();
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
        let manifest_tool_names = manifest
            .tools
            .iter()
            .map(|tool| tool.name.clone())
            .collect::<Vec<_>>();
        let manifest_functions = manifest.functions.clone();
        let plugin_instance = PluginInstance {
            id: manifest.id.clone(),
            manifest: manifest.clone(),
            plugin_vm_instance: Some(instance),
            status: PluginStatus::Loaded,
            registered_tools: manifest_tool_names,
            registered_functions: manifest_functions,
            registered_commands: vec![],
            event_listener_ids: vec![],
            config: serde_json::Value::Null,
            created_at: now,
            loaded_at: now,
            plugin_root: plugin_root.clone(),
        };
        if let Err(e) = self.register_loaded_plugin(plugin_instance) {
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
                .map_err(|_| AppError::Plugin("插件目录下未找到 plugin.json".to_string()))?;
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
        let raw = std::fs::read_to_string(&main_path)
            .map_err(|e| AppError::Plugin(format!("读取 main 脚本失败: {}", e)))?;
        let ext = main_path
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_ascii_lowercase());
        match ext.as_deref() {
            Some("ts") | Some("tsx") => transpile_pi_plugin_for_quickjs(&raw, &manifest.main),
            _ => Ok(raw),
        }
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

    /// 仅基于 manifest 编目插件，不执行主脚本；供静态 tools[]/懒加载路径使用。
    pub fn register_catalog_plugin(
        &self,
        plugin_root: impl AsRef<Path>,
        manifest: PluginManifest,
    ) -> Result<(), AppError> {
        let plugin_root = plugin_root
            .as_ref()
            .canonicalize()
            .unwrap_or_else(|_| plugin_root.as_ref().to_path_buf());
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let manifest_tool_names = manifest
            .tools
            .iter()
            .map(|tool| tool.name.clone())
            .collect::<Vec<_>>();
        let manifest_functions = manifest.functions.clone();
        let instance = PluginInstance {
            id: manifest.id.clone(),
            manifest,
            plugin_vm_instance: None,
            status: PluginStatus::Enabled,
            registered_tools: manifest_tool_names,
            registered_functions: manifest_functions,
            registered_commands: vec![],
            event_listener_ids: vec![],
            config: serde_json::Value::Null,
            created_at: now,
            loaded_at: 0,
            plugin_root,
        };
        self.register_or_refresh_catalog_stub(instance)
    }

    fn register_loaded_plugin(&self, instance: PluginInstance) -> Result<(), AppError> {
        let id = instance.id.clone();
        let mut map = self.plugins.write();
        match map.entry(id.clone()) {
            Entry::Vacant(slot) => {
                slot.insert(instance);
                Ok(())
            }
            Entry::Occupied(mut slot) => {
                let existing = slot.get();
                if existing.plugin_vm_instance.is_none()
                    && existing.plugin_root == instance.plugin_root
                {
                    slot.insert(instance);
                    Ok(())
                } else {
                    Err(AppError::Plugin(format!("plugin already loaded: {}", id)))
                }
            }
        }
    }

    fn register_or_refresh_catalog_stub(&self, instance: PluginInstance) -> Result<(), AppError> {
        let id = instance.id.clone();
        let mut map = self.plugins.write();
        match map.entry(id.clone()) {
            Entry::Vacant(slot) => {
                slot.insert(instance);
                Ok(())
            }
            Entry::Occupied(mut slot) => {
                if slot.get().plugin_vm_instance.is_none() {
                    slot.insert(instance);
                    Ok(())
                } else {
                    Err(AppError::Plugin(format!("plugin already loaded: {}", id)))
                }
            }
        }
    }

    /// 按 ID 获取插件信息（只读，不含插件 VM 实例）。
    pub fn get_plugin(&self, plugin_id: &str) -> Option<PluginInfo> {
        self.sync_registered_capabilities(plugin_id);
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

    /// 注入长生命周期插件 VM 运行时管理器。
    pub fn set_plugin_runtime_manager(&mut self, rm: SharedPluginRuntimeManager) {
        self.plugin_runtime_manager = Some(rm);
    }

    /// 设置事件 channel 容量（默认 64）。
    pub fn set_event_channel_capacity(&mut self, cap: usize) {
        self.event_channel_capacity = cap;
    }

    /// 为指定会话+插件启动长生命周期 VM actor。
    pub async fn start_session_vm(
        &self,
        session_id: &str,
        plugin_id: &str,
    ) -> Result<VmActorHandle, AppError> {
        let key = PluginRuntimeKey::new(session_id, plugin_id);
        let runtime_manager = self
            .plugin_runtime_manager
            .as_ref()
            .ok_or_else(|| AppError::Plugin("plugin_runtime_manager not set".into()))?;

        for (expired_key, expired_handle) in runtime_manager.reap_configured_idle() {
            let _ = expired_handle.shutdown().await;
            if let Some(dispatcher) = self.host_dispatcher.read().clone() {
                dispatcher.cleanup_instance(&expired_key.to_string());
            }
        }

        if let Some(existing) = runtime_manager.get(&key) {
            match existing.current_state() {
                VmActorState::Created | VmActorState::Running | VmActorState::Idle => {
                    return Ok(existing);
                }
                VmActorState::ShuttingDown | VmActorState::Stopped | VmActorState::Error => {
                    let _ = runtime_manager.remove(&key);
                    if let Some(dispatcher) = self.host_dispatcher.read().clone() {
                        dispatcher.cleanup_instance(&key.to_string());
                    }
                }
            }
        }

        let _plugin_info = self
            .get_plugin(plugin_id)
            .ok_or_else(|| AppError::Plugin(format!("plugin '{plugin_id}' not loaded")))?;

        let engine = self
            .plugin_engine
            .as_ref()
            .ok_or_else(|| AppError::Plugin("plugin_engine not set".into()))?;

        let instance_id = key.to_string();
        let mut plugin_vm_instance = engine.create_instance(&instance_id)?;

        let dispatcher_opt = self.host_dispatcher.read().clone();
        let iid = instance_id.clone();
        let invoke_fn = move |request_json: &str| {
            let resp = invoke_host_func_with(dispatcher_opt.as_deref(), &iid, request_json)?;
            serde_json::to_string(&resp).map_err(AppError::from)
        };
        plugin_vm_instance.register_host_binding(invoke_fn)?;

        if let Some(dispatcher) = self.host_dispatcher.read().clone() {
            dispatcher.register_event_channel(&instance_id, self.event_channel_capacity);
        }

        let plugin_root = {
            let map = self.plugins.read();
            map.get(plugin_id)
                .map(|inst| inst.main_script_path())
                .ok_or_else(|| {
                    AppError::Plugin(format!("plugin '{plugin_id}' not found in registry"))
                })?
        };

        let handle = VmActor::spawn(plugin_vm_instance, plugin_root, self.event_channel_capacity);

        handle.dispatch(VmCommand::Init).await?;
        self.sync_registered_capabilities(plugin_id);

        runtime_manager.insert(key, handle.clone());
        Ok(handle)
    }

    pub fn has_session_vm(&self, session_id: &str, plugin_id: &str) -> bool {
        let Some(runtime_manager) = &self.plugin_runtime_manager else {
            return false;
        };
        runtime_manager.contains(&PluginRuntimeKey::new(session_id, plugin_id))
    }

    /// 向指定会话的 VM actor 投递事件。
    pub fn dispatch_session_event(
        &self,
        session_id: &str,
        plugin_id: &str,
        event_type: &str,
        data: serde_json::Value,
        context: serde_json::Value,
    ) -> Result<(), AppError> {
        let key = PluginRuntimeKey::new(session_id, plugin_id);
        if let Some(runtime_manager) = &self.plugin_runtime_manager {
            if let Some(handle) = runtime_manager.get(&key) {
                match handle.current_state() {
                    VmActorState::Error | VmActorState::Stopped | VmActorState::ShuttingDown => {
                        let _ = runtime_manager.remove(&key);
                        if let Some(dispatcher) = self.host_dispatcher.read().clone() {
                            dispatcher.cleanup_instance(&key.to_string());
                        }
                        return Err(AppError::Plugin(format!(
                            "plugin runtime '{key}' is not healthy; call start_session_vm to rebuild"
                        )));
                    }
                    VmActorState::Created | VmActorState::Running | VmActorState::Idle => {
                        let _ = runtime_manager.touch(&key);
                    }
                }
            }
        }

        let dispatcher = self
            .host_dispatcher
            .read()
            .clone()
            .ok_or_else(|| AppError::Plugin("host_dispatcher not set".into()))?;

        let instance_id = key.to_string();
        dispatcher.deliver_event(
            &instance_id,
            EventEnvelope {
                event_type: event_type.to_string(),
                data,
                context,
            },
        )
    }

    /// 结束指定会话下所有 VM actor。
    pub async fn end_session(&self, session_id: &str) -> Result<(), AppError> {
        let t0 = Instant::now();
        tracing::debug!("[end_session] session={session_id} start");
        let runtime_manager = self
            .plugin_runtime_manager
            .as_ref()
            .ok_or_else(|| AppError::Plugin("plugin_runtime_manager not set".into()))?;

        let removed = runtime_manager.remove_session_entries(session_id);
        tracing::debug!(
            "[end_session] removed {} handles elapsed_ms={}",
            removed.len(),
            t0.elapsed().as_millis()
        );
        for (_, handle) in &removed {
            let _ = handle.shutdown().await;
        }

        let mut instance_ids: BTreeSet<String> = removed
            .iter()
            .map(|(key, _)| key.to_string())
            .collect();

        if let Some(dispatcher) = self.host_dispatcher.read().clone() {
            instance_ids.extend(dispatcher.session_instance_ids(session_id));
            for instance_id in instance_ids {
                tracing::debug!("[end_session] cleanup_instance {instance_id}");
                dispatcher.cleanup_instance(&instance_id);
            }
        }

        tracing::debug!(
            "[end_session] shutdown commands sent elapsed_ms={}",
            t0.elapsed().as_millis()
        );
        tracing::debug!(
            "[end_session] session cleanup candidates processed elapsed_ms={}",
            t0.elapsed().as_millis()
        );
        tracing::debug!(
            "[end_session] session={session_id} complete elapsed_ms={}",
            t0.elapsed().as_millis()
        );
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn configured_event_channel_capacity(&self) -> usize {
        self.event_channel_capacity
    }

    #[cfg(test)]
    pub(crate) fn configured_engine_config(&self) -> Option<crate::ext::PluginEngineConfig> {
        self.plugin_engine
            .as_ref()
            .map(|engine| engine.config().clone())
    }

    #[cfg(test)]
    pub(crate) fn configured_idle_ttl(&self) -> Option<std::time::Duration> {
        self.plugin_runtime_manager
            .as_ref()
            .map(|manager| manager.configured_idle_ttl())
    }

    /// 卸载：移除事件监听、注销工具、销毁插件 VM 实例、从 map 移除。
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
        if let Some(t) = self.tools.read().clone() {
            t.unregister_plugin_tools(plugin_id);
        }
        if let Some(functions) = self.functions.read().clone() {
            functions.remove_by_plugin(plugin_id);
        }
        if let Some(dispatcher) = self.host_dispatcher.read().clone() {
            dispatcher.cleanup_plugin_capabilities(plugin_id);
        }
        if let Some(runtime_manager) = &self.plugin_runtime_manager {
            let removed = runtime_manager.remove_plugin(plugin_id);
            if let Some(dispatcher) = self.host_dispatcher.read().clone() {
                for (key, _handle) in removed {
                    dispatcher.cleanup_instance(&key.to_string());
                }
            }
        }
        if let Some(plugin_vm_instance) = instance.plugin_vm_instance {
            plugin_vm_instance.destroy();
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

    fn sync_registered_capabilities(&self, plugin_id: &str) {
        let Some(dispatcher) = self.host_dispatcher.read().clone() else {
            return;
        };
        let mut registered_tools = {
            let map = self.plugins.read();
            map.get(plugin_id)
                .map(|instance| {
                    instance
                        .manifest
                        .tools
                        .iter()
                        .map(|tool| tool.name.clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        };
        let registered_functions = {
            let map = self.plugins.read();
            map.get(plugin_id)
                .map(|instance| instance.manifest.functions.clone())
                .unwrap_or_default()
        };
        for dynamic_tool in dispatcher.registered_plugin_tools(plugin_id) {
            if !registered_tools.iter().any(|tool| tool == &dynamic_tool) {
                registered_tools.push(dynamic_tool);
            }
        }
        let registered_commands = dispatcher
            .registered_plugin_commands(plugin_id)
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        let event_listener_ids = dispatcher.registered_plugin_listener_ids(plugin_id);
        if let Some(instance) = self.plugins.write().get_mut(plugin_id) {
            instance.registered_tools = registered_tools;
            instance.registered_functions = registered_functions;
            instance.registered_commands = registered_commands;
            instance.event_listener_ids = event_listener_ids;
        }
    }
}
