//! # 插件运行时后端边界
//!
//! 对外统一暴露 `PluginEngine` / `PluginVmInstance` / `PluginEngineConfig`，
//! 当前具体实现为 `rquickjs`，后续可在此边界内切换其它运行时后端。

mod crypto_native;
mod engine;
mod engine_config;
mod instance;

pub use engine::PluginEngine;
pub use engine_config::{
    PluginEngineConfig, DEFAULT_PLUGIN_CALL_TIMEOUT_MS, DEFAULT_PLUGIN_IDLE_TTL_MS,
    DEFAULT_PLUGIN_INTERRUPT_BUDGET, DEFAULT_QUICKJS_HEAP_MB,
};
pub use instance::PluginVmInstance;
