//! # WasmEdge 运行时层 (ext)
//!
//! 全局 Engine、独立 Wasm 实例、宿主导入绑定骨架。
//! 默认为桩实现；启用 feature "wasmedge" 且安装 WasmEdge 后为真实实现。

pub mod dispatcher;
mod engine_stub;
pub mod host_binding;
mod instance_stub;
pub mod plugin;

#[cfg(feature = "wasmedge")]
mod engine_wasmedge;
#[cfg(feature = "wasmedge")]
mod instance_wasmedge;

pub use dispatcher::HostApiDispatcher;
pub use engine_stub::{WasmEngineConfig, DEFAULT_QUICKJS_HEAP_MB, DEFAULT_WASM_MAX_PAGES};
pub use host_binding::{invoke_host_func, invoke_host_func_with, HostRequest, HostResponse};

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
