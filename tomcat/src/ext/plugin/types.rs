use crate::infra::error::AppError;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::ext::WasmInstance;
use crate::infra::event_bus::EventListenerId;

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
    /// 插件根目录路径，用于解析 main 入口脚本。
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
