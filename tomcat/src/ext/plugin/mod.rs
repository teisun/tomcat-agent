//! # 插件生命周期管理（与 design CODE_BLOCK_P1_008 / P1_009 一致）
//!
//! PluginManifest、PluginInstance、PluginStatus、加载/启用/禁用/卸载及与 EventBus、ToolRegistry 的清理对接。

mod catalog;
mod manager;
mod source_scan;
mod types;

#[cfg(test)]
mod tests;

pub use catalog::{CatalogEntry, PluginCatalog, PluginCatalogDiagnostic, PluginSource};
pub use manager::PluginManager;
pub use types::{
    parse_manifest, ConfirmPermissionsFn, ManifestTool, PluginActivation, PluginInfo,
    PluginInstance, PluginManifest, PluginStatus,
};
