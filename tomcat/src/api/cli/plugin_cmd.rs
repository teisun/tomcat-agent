//! `tomcat plugin` 子命令实现：list / load / unload / enable / disable / info。

use std::path::Path;

use crate::{
    resolve_plugins_dir, write_file_atomic, AppConfig, AppError, AuditStore, DefaultEventBus,
    DefaultToolRegistry, EventBus, FileAuditRecorder, PluginEngine, PluginManager, Tool,
    ToolExecutor, ToolRegistry, TracingAuditRecorder,
};

use super::PluginSub;

struct PluginContext {
    plugin_manager: PluginManager,
    #[allow(dead_code)]
    config: AppConfig,
}

struct NoopToolExecutor;

#[async_trait::async_trait]
impl ToolExecutor for NoopToolExecutor {
    async fn execute(
        &self,
        tool: &Tool,
        _params: serde_json::Value,
        _caller_plugin_id: &str,
        _session_id: Option<&str>,
    ) -> Result<serde_json::Value, AppError> {
        Err(AppError::Config(format!(
            "CLI 模式下不支持工具执行: {}",
            tool.name
        )))
    }
}

fn build_plugin_context(cfg: &AppConfig) -> Result<PluginContext, AppError> {
    let event_bus: std::sync::Arc<dyn EventBus> = std::sync::Arc::new(DefaultEventBus::new());
    let executor: std::sync::Arc<dyn ToolExecutor> = std::sync::Arc::new(NoopToolExecutor);
    let audit: std::sync::Arc<dyn crate::infra::AuditRecorder> =
        match AuditStore::open_if_enabled(cfg)? {
            Some(store) => std::sync::Arc::new(FileAuditRecorder::new(std::sync::Arc::new(store))),
            None => std::sync::Arc::new(TracingAuditRecorder),
        };
    let tool_registry: std::sync::Arc<dyn ToolRegistry> =
        std::sync::Arc::new(DefaultToolRegistry::new(executor, audit.clone()));
    let mut pm = PluginManager::new(event_bus);
    pm.set_tool_registry(tool_registry);
    pm.set_audit_recorder(audit);

    if let Ok(engine) = PluginEngine::global(None) {
        pm.set_plugin_engine(engine);
    }

    pm.set_confirm_permissions(std::sync::Arc::new(|_| Ok(true)));

    Ok(PluginContext {
        plugin_manager: pm,
        config: cfg.clone(),
    })
}

fn format_plugin_info(info: &crate::PluginInfo) {
    println!("  ID:        {}", info.id);
    println!("  名称:      {}", info.manifest.name);
    println!("  版本:      {}", info.manifest.version);
    println!("  描述:      {}", info.manifest.description);
    println!("  作者:      {}", info.manifest.author);
    println!("  状态:      {:?}", info.status);
    println!("  权限:      {:?}", info.manifest.required_permissions);
    println!("  API 版本:  {}", info.manifest.required_api_version);
    println!("  注册工具:  {:?}", info.registered_tools);
    println!("  注册命令:  {:?}", info.registered_commands);
    println!("  事件监听:  {:?}", info.event_listener_ids);
    println!("  加载时间:  {}", info.loaded_at);
}

// ─── Plugin Registry (registry.json) ──────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct PluginRegistryEntry {
    pub(crate) id: String,
    pub(crate) path: String,
    pub(crate) enabled: bool,
    pub(crate) loaded_at: String,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct PluginRegistryFile {
    #[serde(default)]
    pub(crate) plugins: Vec<PluginRegistryEntry>,
}

fn registry_path(cfg: &AppConfig) -> Result<std::path::PathBuf, AppError> {
    Ok(resolve_plugins_dir(cfg)?.join("registry.json"))
}

pub(crate) fn load_plugin_registry(path: &Path) -> PluginRegistryFile {
    if !path.exists() {
        return PluginRegistryFile::default();
    }
    match std::fs::read_to_string(path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_else(|_| {
            eprintln!("⚠ registry.json 格式损坏，返回空注册表");
            PluginRegistryFile::default()
        }),
        Err(_) => PluginRegistryFile::default(),
    }
}

pub(crate) fn save_plugin_registry(path: &Path, reg: &PluginRegistryFile) -> Result<(), AppError> {
    let json = serde_json::to_string_pretty(reg).map_err(|e| AppError::Config(e.to_string()))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(AppError::Io)?;
    }
    write_file_atomic(path, json.as_bytes())
}

pub(crate) fn run_plugin(sub: PluginSub, cfg: &AppConfig) -> Result<(), AppError> {
    let ctx = build_plugin_context(cfg)?;
    let pm = &ctx.plugin_manager;
    let reg_path = registry_path(cfg)?;

    match sub {
        PluginSub::List => {
            let ids = pm.list_loaded();
            let registry = load_plugin_registry(&reg_path);

            if ids.is_empty() && registry.plugins.is_empty() {
                println!("当前无已加载或已注册插件。");
                if !ctx.config.plugin.auto_load.is_empty() {
                    println!(
                        "  提示: auto_load 中的插件将在对话模式启动时自动加载: {:?}",
                        ctx.config.plugin.auto_load
                    );
                }
            } else {
                println!(
                    "{:<20} {:<15} {:<10} {:<10}",
                    "ID", "路径/名称", "启用", "状态"
                );
                println!("{}", "-".repeat(60));
                for id in &ids {
                    if let Some(info) = pm.get_plugin(id) {
                        println!(
                            "{:<20} {:<15} {:<10} {:?}",
                            info.id, info.manifest.name, "loaded", info.status
                        );
                    }
                }
                for entry in &registry.plugins {
                    if !ids.contains(&entry.id) {
                        let status = "registered";
                        println!(
                            "{:<20} {:<15} {:<10} {}",
                            entry.id,
                            entry.path,
                            if entry.enabled { "是" } else { "否" },
                            status
                        );
                    }
                }
            }
        }
        PluginSub::Load { path } => {
            let p = std::path::Path::new(&path);
            if !p.exists() {
                println!("插件路径不存在: {}", path);
                return Ok(());
            }
            match pm.load_plugin(p) {
                Ok(()) => {
                    println!("插件加载成功: {}", path);
                    let ids = pm.list_loaded();
                    if let Some(id) = ids.last() {
                        if let Some(info) = pm.get_plugin(id) {
                            format_plugin_info(&info);
                        }
                        let mut registry = load_plugin_registry(&reg_path);
                        registry.plugins.retain(|e| e.id != *id);
                        registry.plugins.push(PluginRegistryEntry {
                            id: id.clone(),
                            path: path.clone(),
                            enabled: true,
                            loaded_at: chrono::Utc::now().to_rfc3339(),
                        });
                        save_plugin_registry(&reg_path, &registry)?;
                    }
                }
                Err(e) => {
                    let msg = e.to_string();
                    println!("插件加载失败: {}", msg);
                    if msg.contains("plugin_engine") || msg.contains("rquickjs") {
                        println!("  提示: 请先运行 tomcat doctor 检查运行环境");
                    }
                }
            }
        }
        PluginSub::Unload { id } => {
            let mut registry = load_plugin_registry(&reg_path);
            let had_registry_entry = registry.plugins.iter().any(|entry| entry.id == id);
            match pm.unload_plugin(&id) {
                Ok(()) => {
                    println!("已卸载插件: {}", id);
                    registry.plugins.retain(|e| e.id != id);
                    save_plugin_registry(&reg_path, &registry)?;
                }
                Err(e) if had_registry_entry => {
                    registry.plugins.retain(|entry| entry.id != id);
                    save_plugin_registry(&reg_path, &registry)?;
                    println!("已卸载插件: {}", id);
                }
                Err(e) => println!("卸载失败: {}", e),
            }
        }
        PluginSub::Enable { id } => match pm.enable_plugin(&id) {
            Ok(()) => {
                println!("已启用插件: {}", id);
                let mut registry = load_plugin_registry(&reg_path);
                if let Some(entry) = registry.plugins.iter_mut().find(|e| e.id == id) {
                    entry.enabled = true;
                    save_plugin_registry(&reg_path, &registry)?;
                }
            }
            Err(e) => println!("启用失败: {}", e),
        },
        PluginSub::Disable { id } => match pm.disable_plugin(&id) {
            Ok(()) => {
                println!("已禁用插件: {}", id);
                let mut registry = load_plugin_registry(&reg_path);
                if let Some(entry) = registry.plugins.iter_mut().find(|e| e.id == id) {
                    entry.enabled = false;
                    save_plugin_registry(&reg_path, &registry)?;
                }
            }
            Err(e) => println!("禁用失败: {}", e),
        },
        PluginSub::Info { id } => match pm.get_plugin(&id) {
            Some(info) => format_plugin_info(&info),
            None => println!("插件未找到: {}", id),
        },
    }
    Ok(())
}
