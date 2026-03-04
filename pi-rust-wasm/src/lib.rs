//! # pi_awsm 库
//!
//! 基础设施层与事件总线，供 session_cli / llm / wasm_plugin 等模块依赖。下列 `pub use` 为对外统一入口。

pub mod config;
pub mod error;
pub mod event_bus;
pub mod events;
pub mod logging;
pub mod platform;

/// 配置与校验入口、以及各子配置类型。
pub use config::{load_config, validate_config, AppConfig, PrimitiveConfig, SecurityConfig};
pub use error::AppError;
pub use event_bus::{DefaultEventBus, EventBus, EventContext, EventListenerId};
pub use events::{AgentEvent, ExtensionEvent};
pub use logging::init_logging;
pub use platform::{normalize_path, read_file_utf8, write_file_atomic};
