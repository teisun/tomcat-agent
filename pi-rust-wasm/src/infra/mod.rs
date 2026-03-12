//! # 基础设施层 (Infrastructure)
//!
//! 提供配置、统一错误、日志、跨平台路径与文件操作、事件总线及事件类型。
//! 上层（core / ext）仅依赖本层对外暴露的契约；子模块使用 `pub(crate)` 限定在 Crate 内可见，
//! 通过本文件选择性 `pub use` 暴露对外 API，遵循分层架构与最小暴露原则。

pub(crate) mod audit;
pub(crate) mod config;
pub(crate) mod error;
pub(crate) mod event_bus;
pub(crate) mod events;
pub(crate) mod logging;
pub(crate) mod platform;

pub use audit::{
    AuditPrimitiveOp, AuditRecorder, HostcallAuditEntry, PrimitiveAuditEntry, ToolAuditEntry,
    TracingAuditRecorder,
};
pub use config::{
    ensure_work_dir_structure, get_work_dir, load_config, resolve_log_dir, resolve_plugins_dir,
    resolve_quickjs_path, resolve_sessions_dir, resolve_tmp_dir, resolve_workspace_dir,
    validate_config, AppConfig, LlmConfig, LogConfig, PrimitiveConfig, SecurityConfig, WasmConfig,
};
pub use error::AppError;
pub use event_bus::{DefaultEventBus, EventBus, EventContext, EventListenerId};
pub use events::{AgentEvent, ExtensionEvent};
pub use logging::init_logging;
pub use platform::{normalize_path, read_file_utf8, write_file_atomic};
