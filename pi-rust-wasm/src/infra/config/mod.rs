//! # 配置模块 (Config)
//!
//! 配置结构体、加载与合并、合法性校验。多源合并顺序：默认值 → 配置文件 → 环境变量（前缀 `PI_WASM__`，分隔符 `__`）。
//! 内嵌资源管理：wasmedge_quickjs.wasm + assets/modules/ 在启动时自动释放到 work_dir。

mod assets;
mod load;
mod types;

#[cfg(test)]
mod tests;

pub use assets::ensure_embedded_assets;
pub use load::{
    ensure_work_dir_structure, get_work_dir, load_config, load_config_toml_file, resolve_agent_dir,
    resolve_assets_dir, resolve_audit_dir, resolve_extra_roots_paths, resolve_log_dir,
    resolve_memory_dir, resolve_plugins_dir, resolve_quickjs_path, resolve_sessions_dir,
    resolve_tmp_dir, resolve_workspace_dir, validate_config,
};
pub use types::{
    compute_context_budget_chars, AgentConfig, AppConfig, ContextConfig, LlmConfig, LogConfig,
    PrimitiveConfig, SecurityConfig, WasmConfig, WorkspaceConfig, DEFAULT_LLM_MODEL,
};
