//! # WasmEdge 运行时层 (ext)
//!
//! 全局 Engine、独立 Wasm 实例、宿主导入绑定骨架。
//! 默认为桩实现；启用 feature "wasmedge" 且安装 WasmEdge 后为真实实现。

pub mod dispatcher;
#[allow(dead_code)]
mod engine_stub;
#[cfg(feature = "wasmedge")]
mod engine_wasmedge;
pub mod host_binding;
#[allow(dead_code)]
mod instance_stub;
#[cfg(feature = "wasmedge")]
mod instance_wasmedge;
pub mod plugin;
pub mod runtime_manager;
pub mod ts_compiler;
pub mod vm_actor;

pub use dispatcher::{AsyncCallStatus, HostApiDispatcher};
pub use engine_stub::{WasmEngineConfig, DEFAULT_QUICKJS_HEAP_MB, DEFAULT_WASM_MAX_PAGES};
pub use host_binding::{invoke_host_func, invoke_host_func_with, HostRequest, HostResponse};
pub use ts_compiler::{transpile_pi_plugin_for_quickjs, transpile_typescript};

#[cfg(not(feature = "wasmedge"))]
pub use engine_stub::WasmEngine;
#[cfg(feature = "wasmedge")]
pub use engine_wasmedge::WasmEngine;

#[cfg(not(feature = "wasmedge"))]
pub use instance_stub::WasmInstance;
#[cfg(feature = "wasmedge")]
pub use instance_wasmedge::WasmInstance;

pub use plugin::{
    parse_manifest, PluginInfo, PluginInstance, PluginManager, PluginManifest, PluginStatus,
};
pub use runtime_manager::{RuntimeManager, SharedRuntimeManager, VmRuntimeKey};
pub use vm_actor::{EventEnvelope, VmActorHandle, VmActorState, VmCommand};

#[cfg(test)]
mod tests;
