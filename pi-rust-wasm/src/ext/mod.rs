//! # WasmEdge 运行时层 (ext)
//!
//! 全局 Engine、独立 Wasm 实例、宿主导入绑定骨架。
//! 默认构建即包含 WasmEdge 真实实现，需安装 WasmEdge C 库。

pub mod dispatcher;
#[allow(dead_code)]
mod engine_stub;
mod engine_wasmedge;
pub mod host_binding;
#[allow(dead_code)]
mod instance_stub;
mod instance_wasmedge;
pub mod plugin;
pub mod runtime_manager;
pub mod ts_compiler;
pub mod vm_actor;

pub use dispatcher::{AsyncCallStatus, HostApiDispatcher};
pub use engine_stub::{WasmEngineConfig, DEFAULT_QUICKJS_HEAP_MB, DEFAULT_WASM_MAX_PAGES};
pub use engine_wasmedge::WasmEngine;
pub use host_binding::{invoke_host_func, invoke_host_func_with, HostRequest, HostResponse};
pub use instance_wasmedge::WasmInstance;
pub use ts_compiler::{transpile_pi_plugin_for_quickjs, transpile_typescript};

pub use plugin::{
    parse_manifest, PluginInfo, PluginInstance, PluginManager, PluginManifest, PluginStatus,
};
pub use runtime_manager::{RuntimeManager, SharedRuntimeManager, VmRuntimeKey};
pub use vm_actor::{EventEnvelope, VmActorHandle, VmActorState, VmCommand};
