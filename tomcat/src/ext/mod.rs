//! # 插件运行时层 (ext)
//!
//! 全局 Engine、独立 VM 实例、宿主导入绑定骨架。
//! 当前真实实现基于 `rquickjs`；历史 WasmEdge 文件保留作迁移参考。

mod crypto_native;
pub mod dispatcher;
mod engine_rquickjs;
#[allow(dead_code)]
mod engine_stub;
pub mod host_binding;
mod instance_rquickjs;
#[allow(dead_code)]
mod instance_stub;
pub mod plugin;
mod plugin_tool_executor;
pub mod runtime_manager;
pub mod ts_compiler;
pub mod vm_actor;

pub use dispatcher::{AsyncCallStatus, HostApiDispatcher};
pub use engine_stub::{
    WasmEngineConfig, DEFAULT_PLUGIN_CALL_TIMEOUT_MS, DEFAULT_PLUGIN_IDLE_TTL_MS,
    DEFAULT_PLUGIN_INTERRUPT_BUDGET, DEFAULT_QUICKJS_HEAP_MB, DEFAULT_WASM_MAX_PAGES,
};
pub use host_binding::{invoke_host_func, invoke_host_func_with, HostRequest, HostResponse};
pub use ts_compiler::{transpile_pi_plugin_for_quickjs, transpile_typescript};

pub use engine_rquickjs::WasmEngine;

pub use instance_rquickjs::WasmInstance;

pub use plugin::{
    parse_manifest, CatalogEntry, ManifestTool, PluginActivation, PluginCatalog,
    PluginCatalogDiagnostic, PluginInfo, PluginInstance, PluginManager, PluginManifest,
    PluginSource, PluginStatus,
};
pub use plugin_tool_executor::PluginToolExecutor;
pub use runtime_manager::{RuntimeManager, SharedRuntimeManager, VmRuntimeKey};
pub use vm_actor::{EventEnvelope, VmActorHandle, VmActorState, VmCommand};

#[cfg(test)]
mod tests;
