use super::types::{
    ConfirmPermissionsFn, PluginInfo, PluginInstance, PluginManifest, PluginStatus,
};
use crate::core::ToolRegistry;
use crate::infra::audit::{AuditRecorder, PluginLifecycleAuditEntry};
use crate::infra::error::AppError;
use crate::infra::event_bus::EventBus;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use crate::ext::runtime_manager::{SharedRuntimeManager, VmRuntimeKey};
use crate::ext::ts_compiler::transpile_pi_plugin_for_quickjs;
use crate::ext::vm_actor::{EventEnvelope, VmActor, VmActorHandle, VmCommand};
use crate::ext::{invoke_host_func_with, HostApiDispatcher, WasmEngine};

use super::types::parse_manifest;

/// 插件管理器：加载/卸载/启用/禁用，卸载时调用 EventBus.remove_plugin_listeners、ToolRegistry.unregister_plugin_tools。
pub struct PluginManager {
    event_bus: Arc<dyn EventBus>,
    tools: Option<Arc<dyn ToolRegistry>>,
    plugins: RwLock<HashMap<String, PluginInstance>>,
    wasm_engine: Option<Arc<WasmEngine>>,
    host_dispatcher: Option<Arc<HostApiDispatcher>>,
    confirm_permissions: Option<Arc<ConfirmPermissionsFn>>,
    audit: Option<Arc<dyn AuditRecorder>>,
    runtime_manager: Option<SharedRuntimeManager>,
    event_channel_capacity: usize,
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
            runtime_manager: None,
            event_channel_capacity: 64,
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

    /// 注入长生命周期 VM 运行时管理器。
    pub fn set_runtime_manager(&mut self, rm: SharedRuntimeManager) {
        self.runtime_manager = Some(rm);
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
        let key = VmRuntimeKey::new(session_id, plugin_id);
        let rm = self
            .runtime_manager
            .as_ref()
            .ok_or_else(|| AppError::Plugin("runtime_manager not set".into()))?;

        if let Some(existing) = rm.get(&key) {
            return Ok(existing);
        }

        let _plugin_info = self
            .get_plugin(plugin_id)
            .ok_or_else(|| AppError::Plugin(format!("plugin '{plugin_id}' not loaded")))?;

        let engine = self
            .wasm_engine
            .as_ref()
            .ok_or_else(|| AppError::Plugin("wasm_engine not set".into()))?;

        let instance_id = key.to_string();
        let mut wasm_instance = engine.create_instance(&instance_id)?;

        let dispatcher_opt = self.host_dispatcher.clone();
        let iid = instance_id.clone();
        let invoke_fn = move |request_json: &str| {
            let resp = invoke_host_func_with(dispatcher_opt.as_deref(), &iid, request_json)?;
            serde_json::to_string(&resp).map_err(AppError::from)
        };
        wasm_instance.register_host_binding(invoke_fn)?;

        if let Some(ref dispatcher) = self.host_dispatcher {
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

        let (handle, _event_tx) =
            VmActor::spawn(wasm_instance, plugin_root, self.event_channel_capacity);

        handle.dispatch(VmCommand::Init).await?;

        rm.insert(key, handle.clone());
        Ok(handle)
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
        let key = VmRuntimeKey::new(session_id, plugin_id);

        let dispatcher = self
            .host_dispatcher
            .as_ref()
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
        let rm = self
            .runtime_manager
            .as_ref()
            .ok_or_else(|| AppError::Plugin("runtime_manager not set".into()))?;

        let handles = rm.remove_session(session_id);
        tracing::debug!(
            "[end_session] removed {} handles elapsed_ms={}",
            handles.len(),
            t0.elapsed().as_millis()
        );
        for h in &handles {
            let _ = h.shutdown().await;
        }
        tracing::debug!(
            "[end_session] shutdown commands sent elapsed_ms={}",
            t0.elapsed().as_millis()
        );

        if let Some(ref dispatcher) = self.host_dispatcher {
            let map = self.plugins.read();
            for pid in map.keys() {
                let instance_id = format!("{session_id}/{pid}");
                tracing::debug!("[end_session] cleanup_instance {instance_id}");
                dispatcher.cleanup_instance(&instance_id);
            }
        }

        tracing::debug!(
            "[end_session] session={session_id} complete elapsed_ms={}",
            t0.elapsed().as_millis()
        );
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
