//! # 插件运行时层 (ext)
//!
//! 全局 Engine、独立 VM 实例、宿主导入绑定骨架。
//! 当前真实实现基于 `rquickjs`。

pub mod dispatcher;
pub mod host_binding;
pub mod plugin;
mod plugin_tool_executor;
mod runtime;
pub mod runtime_manager;
pub mod ts_compiler;
pub mod vm_actor;

pub use dispatcher::{AsyncCallStatus, HostApiDispatcher};
pub use host_binding::{invoke_host_func, invoke_host_func_with, HostRequest, HostResponse};
pub use runtime::{
    PluginEngine, PluginEngineConfig, PluginVmInstance, DEFAULT_PLUGIN_CALL_TIMEOUT_MS,
    DEFAULT_PLUGIN_IDLE_TTL_MS, DEFAULT_PLUGIN_INTERRUPT_BUDGET, DEFAULT_QUICKJS_HEAP_MB,
};
pub use ts_compiler::{transpile_pi_plugin_for_quickjs, transpile_typescript};

pub use plugin::{
    parse_manifest, CatalogEntry, ManifestTool, PluginActivation, PluginCatalog,
    PluginCatalogDiagnostic, PluginInfo, PluginInstance, PluginManager, PluginManifest,
    PluginSource, PluginStatus,
};
pub use plugin_tool_executor::PluginToolExecutor;
pub use runtime_manager::{PluginRuntimeKey, PluginRuntimeManager, SharedPluginRuntimeManager};
pub use vm_actor::{EventEnvelope, VmActorHandle, VmActorState, VmCommand};

#[cfg(test)]
mod tests;
