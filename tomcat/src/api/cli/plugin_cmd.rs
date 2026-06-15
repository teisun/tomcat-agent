//! `tomcat plugin` 子命令实现：list / load / unload / enable / disable / info。

use std::path::Path;

pub(crate) use crate::core::package::{
    load_plugin_registry, save_plugin_registry, PluginRegistryEntry, PluginRegistryFile,
};
use crate::core::package::{resolve_runtime_layer_paths, PackageVisibility};
use crate::ext::parse_manifest;
use crate::{
    resolve_plugins_dir, AppConfig, AppError, AuditStore, DefaultEventBus, DefaultToolRegistry,
    EventBus, FileAuditRecorder, PluginEngine, PluginManager, Tool, ToolExecutor, ToolRegistry,
    TracingAuditRecorder,
};
use std::fmt::Write as _;

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
    let registered_functions = info
        .registered_functions
        .iter()
        .map(|function| format!("{} -> {}", function.point, function.function))
        .collect::<Vec<_>>();
    println!("  ID:        {}", info.id);
    println!("  名称:      {}", info.manifest.name);
    println!("  版本:      {}", info.manifest.version);
    println!("  描述:      {}", info.manifest.description);
    println!("  作者:      {}", info.manifest.author);
    println!("  状态:      {:?}", info.status);
    println!("  权限:      {:?}", info.manifest.required_permissions);
    println!("  API 版本:  {}", info.manifest.required_api_version);
    println!("  注册工具:  {:?}", info.registered_tools);
    println!("  注册函数:  {:?}", registered_functions);
    println!("  注册命令:  {:?}", info.registered_commands);
    println!("  事件监听:  {:?}", info.event_listener_ids);
    println!("  加载时间:  {}", info.loaded_at);
}

// ─── Layered Plugin Registry (registry.json) ───────────────────────────────

#[derive(Debug, Clone)]
struct LayeredRegistry {
    visibility: PackageVisibility,
    path: std::path::PathBuf,
    registry: PluginRegistryFile,
}

#[derive(Debug, Clone)]
struct LocatedRegistryEntry {
    visibility: PackageVisibility,
    path: std::path::PathBuf,
    entry: PluginRegistryEntry,
}

fn registry_path(cfg: &AppConfig) -> Result<std::path::PathBuf, AppError> {
    Ok(resolve_plugins_dir(cfg)?.join("registry.json"))
}

fn scope_context_root() -> Option<std::path::PathBuf> {
    std::env::current_dir().ok()
}

fn layered_registries(cfg: &AppConfig) -> Result<Vec<LayeredRegistry>, AppError> {
    let scope_root = scope_context_root();
    let mut out = Vec::new();
    for layer in resolve_runtime_layer_paths(cfg, scope_root.as_deref())? {
        out.push(LayeredRegistry {
            visibility: layer.visibility,
            path: layer.plugin_registry_path.clone(),
            registry: load_plugin_registry(&layer.plugin_registry_path),
        });
    }
    Ok(out)
}

fn locate_registry_entry(
    cfg: &AppConfig,
    id: &str,
) -> Result<Option<LocatedRegistryEntry>, AppError> {
    for layer in layered_registries(cfg)? {
        if let Some(entry) = layer.registry.plugins.iter().find(|entry| entry.id == id) {
            return Ok(Some(LocatedRegistryEntry {
                visibility: layer.visibility,
                path: layer.path.clone(),
                entry: entry.clone(),
            }));
        }
    }
    Ok(None)
}

fn visible_and_shadowed_entries(
    registries: &[LayeredRegistry],
) -> (Vec<LocatedRegistryEntry>, Vec<LocatedRegistryEntry>) {
    let mut visible = Vec::new();
    let mut shadowed = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for layer in registries {
        for entry in &layer.registry.plugins {
            let located = LocatedRegistryEntry {
                visibility: layer.visibility,
                path: layer.path.clone(),
                entry: entry.clone(),
            };
            if seen.insert(entry.id.clone()) {
                visible.push(located);
            } else {
                shadowed.push(located);
            }
        }
    }
    (visible, shadowed)
}

pub(crate) fn render_plugin_list_output(cfg: &AppConfig) -> Result<String, AppError> {
    let registries = layered_registries(cfg)?;
    Ok(render_plugin_list_from_registries(
        &registries,
        &cfg.plugin.auto_load,
    ))
}

fn render_plugin_list_from_registries(
    registries: &[LayeredRegistry],
    auto_load: &[String],
) -> String {
    let (visible, shadowed) = visible_and_shadowed_entries(registries);
    let mut out = String::new();

    if visible.is_empty() && shadowed.is_empty() {
        out.push_str("当前无已注册插件。\n");
        if !auto_load.is_empty() {
            let _ = writeln!(
                out,
                "  提示: auto_load 中的插件将在对话模式启动时自动加载: {:?}",
                auto_load
            );
        }
        return out;
    }

    let _ = writeln!(
        out,
        "{:<24} {:<10} {:<8} {:<12}",
        "ID", "层", "启用", "状态"
    );
    let _ = writeln!(out, "{}", "-".repeat(72));
    for item in &visible {
        let _ = writeln!(
            out,
            "{:<24} {:<10} {:<8} {:<12}",
            item.entry.id,
            item.visibility.as_str(),
            if item.entry.enabled { "是" } else { "否" },
            "visible"
        );
    }
    if !shadowed.is_empty() {
        out.push_str("\nshadowed:\n");
        for item in &shadowed {
            let _ = writeln!(
                out,
                "  - {} @ {} ({})",
                item.entry.id,
                item.visibility.as_str(),
                item.entry.path
            );
        }
    }

    out
}

pub(crate) fn run_plugin(sub: PluginSub, cfg: &AppConfig) -> Result<(), AppError> {
    let ctx = build_plugin_context(cfg)?;
    let pm = &ctx.plugin_manager;
    let reg_path = registry_path(cfg)?;

    match sub {
        PluginSub::List => {
            print!("{}", render_plugin_list_output(&ctx.config)?);
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
                        let mut registry_path_value = path.clone();
                        if let Some(info) = pm.get_plugin(id) {
                            registry_path_value = info.plugin_root.display().to_string();
                            format_plugin_info(&info);
                        }
                        let mut registry = load_plugin_registry(&reg_path);
                        registry.plugins.retain(|e| e.id != *id);
                        registry.plugins.push(PluginRegistryEntry {
                            id: id.clone(),
                            path: registry_path_value,
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
        PluginSub::Unload { id } => match locate_registry_entry(cfg, &id)? {
            Some(located) => {
                let mut registry = load_plugin_registry(&located.path);
                registry.plugins.retain(|entry| entry.id != id);
                save_plugin_registry(&located.path, &registry)?;
                let _ = pm.unload_plugin(&id);
                println!("已卸载插件: {} ({})", id, located.visibility.as_str());
            }
            None => println!("卸载失败: 插件未找到: {}", id),
        },
        PluginSub::Enable { id } => match locate_registry_entry(cfg, &id)? {
            Some(located) => {
                let mut registry = load_plugin_registry(&located.path);
                if let Some(entry) = registry.plugins.iter_mut().find(|entry| entry.id == id) {
                    entry.enabled = true;
                    save_plugin_registry(&located.path, &registry)?;
                }
                let _ = pm.enable_plugin(&id);
                println!("已启用插件: {} ({})", id, located.visibility.as_str());
            }
            None => println!("启用失败: 插件未找到: {}", id),
        },
        PluginSub::Disable { id } => match locate_registry_entry(cfg, &id)? {
            Some(located) => {
                let mut registry = load_plugin_registry(&located.path);
                if let Some(entry) = registry.plugins.iter_mut().find(|entry| entry.id == id) {
                    entry.enabled = false;
                    save_plugin_registry(&located.path, &registry)?;
                }
                let _ = pm.disable_plugin(&id);
                println!("已禁用插件: {} ({})", id, located.visibility.as_str());
            }
            None => println!("禁用失败: 插件未找到: {}", id),
        },
        PluginSub::Info { id } => match locate_registry_entry(cfg, &id)? {
            Some(located) => {
                println!("  层:        {}", located.visibility.as_str());
                println!("  路径:      {}", located.entry.path);
                println!(
                    "  启用:      {}",
                    if located.entry.enabled { "是" } else { "否" }
                );
                let manifest_path = Path::new(&located.entry.path).join("plugin.json");
                match std::fs::read_to_string(&manifest_path) {
                    Ok(raw) => match parse_manifest(&raw) {
                        Ok(manifest) => {
                            println!("  ID:        {}", manifest.id);
                            println!("  名称:      {}", manifest.name);
                            println!("  版本:      {}", manifest.version);
                            println!("  描述:      {}", manifest.description);
                            println!("  作者:      {}", manifest.author);
                            println!("  API 版本:  {}", manifest.required_api_version);
                            println!("  权限:      {:?}", manifest.required_permissions);
                        }
                        Err(error) => println!("  清单解析失败: {}", error),
                    },
                    Err(error) => println!("  读取清单失败: {}", error),
                }
            }
            None => println!("插件未找到: {}", id),
        },
    }
    Ok(())
}
