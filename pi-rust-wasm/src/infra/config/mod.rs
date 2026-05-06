//! # 配置模块 (Config)
//!
//! 配置结构体、加载与合并、合法性校验。多源合并顺序：默认值 → 配置文件 → 环境变量（前缀 `PI_WASM__`，分隔符 `__`）。
//! 内嵌资源管理：wasmedge_quickjs.wasm + assets/modules/ 在启动时自动释放到 work_dir。

pub mod append;
mod assets;
mod load;
pub mod lock;
mod types;

#[cfg(test)]
mod tests;

#[allow(unused_imports)]
pub use append::append_workspace_entry_to_disk;
pub use append::{append_path_rule_to_disk, append_workspace_root_to_disk};
pub use assets::ensure_embedded_assets;
pub use load::{
    ensure_work_dir_structure, get_work_dir, load_config, load_config_toml_file,
    resolve_agent_definition_dir, resolve_agent_dir, resolve_agent_trail_dir, resolve_assets_dir,
    resolve_audit_dir, resolve_log_dir, resolve_memory_dir, resolve_plugins_dir,
    resolve_quickjs_path, resolve_sessions_dir, resolve_tmp_dir, resolve_workspace_dir,
    resolve_workspace_roots_paths, validate_config,
};
pub use lock::with_config_lock;
#[allow(unused_imports)]
pub use types::WorkspaceEntry;
pub use types::{
    compute_context_budget_chars, AgentConfig, AppConfig, ContextConfig, LlmConfig, LogConfig,
    PreflightConfig, PrimitiveConfig, SecurityConfig, WasmConfig, WorkspaceConfig,
    DEFAULT_LLM_MODEL,
};
#[allow(unused_imports)]
pub use types::{
    ToolsBashConfig, ToolsConfig, ToolsReadConfig, ToolsWriteConfig,
    DEFAULT_TOOLS_BASH_MAX_OUTPUT_CHARS, DEFAULT_TOOLS_BASH_TIMEOUT_MS,
    DEFAULT_TOOLS_READ_MAX_BYTES, DEFAULT_TOOLS_WRITE_NORMALIZE_CRLF,
    MAX_TOOLS_BASH_MAX_OUTPUT_CHARS, MAX_TOOLS_BASH_TIMEOUT_MS,
};
