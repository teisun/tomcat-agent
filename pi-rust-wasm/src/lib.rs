//! # pi_awsm 库
//!
//! 基础设施层与事件总线、WasmEdge 运行时层（ext），供 session_cli / llm / wasm_plugin 等模块依赖。
//! 对外 API 通过 `infra` / `ext` 层统一暴露，符合编码与分层架构规范。

pub mod infra;
pub mod core;
pub mod ext;

pub use infra::{
    init_logging, load_config, normalize_path, read_file_utf8, validate_config, write_file_atomic,
    AgentEvent, AppConfig, AppError, DefaultEventBus, EventBus, EventContext, EventListenerId,
    ExtensionEvent, LogConfig, PrimitiveConfig, SecurityConfig,
};
pub use ext::{
    HostApiDispatcher, HostRequest, HostResponse, PluginInfo, PluginInstance, PluginManager,
    PluginManifest, PluginStatus, WasmEngine, WasmEngineConfig, WasmInstance, invoke_host_func,
    parse_manifest,
};
