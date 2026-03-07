//! # WasmEdge 运行时层 (ext)
//!
//! 全局 Engine、独立 Wasm 实例、宿主导入绑定骨架。
//! 默认为桩实现；启用 feature "wasmedge" 且安装 WasmEdge 后为真实实现。

pub mod host_binding;
pub mod dispatcher;
pub mod plugin;
mod engine_stub;
mod instance_stub;

#[cfg(feature = "wasmedge")]
mod engine_wasmedge;
#[cfg(feature = "wasmedge")]
mod instance_wasmedge;

pub use dispatcher::HostApiDispatcher;
pub use host_binding::{HostRequest, HostResponse, invoke_host_func, invoke_host_func_with};
pub use engine_stub::{WasmEngineConfig, DEFAULT_WASM_MAX_PAGES, DEFAULT_QUICKJS_HEAP_MB};

#[cfg(not(feature = "wasmedge"))]
pub use engine_stub::WasmEngine;
#[cfg(feature = "wasmedge")]
pub use engine_wasmedge::WasmEngine;

#[cfg(not(feature = "wasmedge"))]
pub use instance_stub::WasmInstance;
#[cfg(feature = "wasmedge")]
pub use instance_wasmedge::WasmInstance;

pub use plugin::{PluginInfo, PluginInstance, PluginManager, PluginManifest, PluginStatus, parse_manifest};
