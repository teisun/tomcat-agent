//! # WasmEdge 运行时层 (ext)
//!
//! 全局 Engine、独立 Wasm 实例、宿主导入绑定骨架。
//! 当前为桩实现，保证主库无 WasmEdge 依赖即可编译；真实 WasmEdge+QuickJS 集成可后续通过 feature 或独立 crate 接入。

pub mod host_binding;
pub mod dispatcher;
pub mod plugin;
mod engine_stub;
mod instance_stub;

pub use dispatcher::HostApiDispatcher;
pub use host_binding::{HostRequest, HostResponse, invoke_host_func};
pub use engine_stub::{WasmEngine, WasmEngineConfig, DEFAULT_WASM_MAX_PAGES, DEFAULT_QUICKJS_HEAP_MB};
pub use instance_stub::WasmInstance;
pub use plugin::{PluginInfo, PluginInstance, PluginManager, PluginManifest, PluginStatus, parse_manifest};
