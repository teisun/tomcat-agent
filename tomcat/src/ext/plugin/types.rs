use crate::infra::error::AppError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

use crate::ext::PluginVmInstance;
use crate::infra::event_bus::EventListenerId;

/// 插件清单（与 design CODE_BLOCK_P1_008 一致）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum PluginActivation {
    #[default]
    Lazy,
    Session,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestTool {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_manifest_tool_parameters")]
    pub parameters: Value,
}

fn default_manifest_tool_parameters() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {},
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestFunction {
    pub point: String,
    pub function: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: String,
    pub main: String,
    #[serde(default)]
    pub required_permissions: Vec<String>,
    #[serde(default)]
    pub required_secrets: Vec<String>,
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
    pub required_api_version: String,
    pub tags: Vec<String>,
    #[serde(default)]
    pub tools: Vec<ManifestTool>,
    #[serde(default)]
    pub functions: Vec<ManifestFunction>,
    #[serde(default)]
    pub events: Vec<String>,
    #[serde(default)]
    pub activation: PluginActivation,
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

/// 单插件实例：持有插件 VM 实例、状态、注册的工具与事件监听 ID，卸载时清理。
#[derive(Debug)]
pub struct PluginInstance {
    pub id: String,
    pub manifest: PluginManifest,
    pub plugin_vm_instance: Option<PluginVmInstance>,
    pub status: PluginStatus,
    pub registered_tools: Vec<String>,
    pub registered_functions: Vec<ManifestFunction>,
    pub registered_commands: Vec<String>,
    pub event_listener_ids: Vec<EventListenerId>,
    pub config: serde_json::Value,
    pub created_at: i64,
    pub loaded_at: i64,
    /// 插件根目录路径，用于解析 main 入口脚本。
    pub plugin_root: PathBuf,
}

/// 只读插件信息（不含插件 VM 实例），用于 get_plugin 等查询。
#[derive(Debug, Clone)]
pub struct PluginInfo {
    pub id: String,
    pub manifest: PluginManifest,
    pub status: PluginStatus,
    pub registered_tools: Vec<String>,
    pub registered_functions: Vec<ManifestFunction>,
    pub registered_commands: Vec<String>,
    pub event_listener_ids: Vec<EventListenerId>,
    pub config: serde_json::Value,
    pub created_at: i64,
    pub loaded_at: i64,
    pub plugin_root: PathBuf,
}

impl PluginInstance {
    pub fn plugin_id(&self) -> &str {
        &self.id
    }

    /// 返回插件 main 入口脚本的绝对路径。
    pub fn main_script_path(&self) -> PathBuf {
        self.plugin_root.join(&self.manifest.main)
    }

    pub fn to_info(&self) -> PluginInfo {
        PluginInfo {
            id: self.id.clone(),
            manifest: self.manifest.clone(),
            status: self.status,
            registered_tools: self.registered_tools.clone(),
            registered_functions: self.registered_functions.clone(),
            registered_commands: self.registered_commands.clone(),
            event_listener_ids: self.event_listener_ids.clone(),
            config: self.config.clone(),
            created_at: self.created_at,
            loaded_at: self.loaded_at,
            plugin_root: self.plugin_root.clone(),
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
    for tool in &m.tools {
        if tool.name.trim().is_empty() {
            return Err(AppError::Plugin(
                "manifest.tools[].name is required".to_string(),
            ));
        }
        if !tool.parameters.is_object() {
            return Err(AppError::Plugin(format!(
                "manifest.tools[{}].parameters must be an object",
                tool.name
            )));
        }
    }
    for function in &m.functions {
        if function.point.trim().is_empty() {
            return Err(AppError::Plugin(
                "manifest.functions[].point is required".to_string(),
            ));
        }
        if function.function.trim().is_empty() {
            return Err(AppError::Plugin(
                "manifest.functions[].function is required".to_string(),
            ));
        }
    }
    if m.required_permissions
        .iter()
        .any(|perm| perm == "net:fetch")
        && m.allowed_hosts.is_empty()
    {
        return Err(AppError::Plugin(
            "manifest.allowedHosts is required when manifest.requiredPermissions contains net:fetch"
                .to_string(),
        ));
    }
    Ok(())
}

/// 用户确认插件权限的回调：传入清单，返回 Ok(true) 同意、Ok(false) 拒绝、Err 表示确认过程出错。
pub type ConfirmPermissionsFn = dyn Fn(&PluginManifest) -> Result<bool, AppError> + Send + Sync;
