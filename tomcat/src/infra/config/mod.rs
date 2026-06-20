//! # 配置模块 (Config)
//!
//! 配置结构体、加载与合并、合法性校验。多源合并顺序：默认值 → 配置文件 → 环境变量（前缀 `TOMCAT__`，分隔符 `__`）。
//! 资产目录初始化：`assets/` 在启动时确保存在。

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
    ensure_work_dir_structure, get_work_dir, load_config, load_config_for_init,
    load_config_toml_file,
    resolve_agent_definition_dir, resolve_agent_dir, resolve_agent_trail_dir, resolve_assets_dir,
    resolve_audit_dir, resolve_checkpoints_dir, resolve_dot_tomcat_temp_dir, resolve_log_dir,
    resolve_memory_dir, resolve_plans_dir, resolve_plugins_dir, resolve_sessions_dir,
    resolve_tmp_dir, resolve_workspace_dir, resolve_workspace_roots_paths, validate_config,
};
pub use lock::with_config_lock;
#[allow(unused_imports)]
pub use types::WorkspaceEntry;
#[allow(unused_imports)]
pub use types::{
    compute_context_budget_chars, AgentConfig, AppConfig, CheckpointConfig, ContextConfig,
    LlmConfig, LlmFilesConfig, LlmRuntimeConfig, LogConfig, OpenAiResponsesConfig, PreflightConfig,
    PrimitiveConfig, ReasoningContinuityConfig, ResumeHydrationMode, SecurityConfig, SessionConfig,
    SkillsConfig, SplashConfig, ThinkingConfig, ThinkingDisplay, ToolCliVerbosity, WorkspaceConfig,
    DEFAULT_AGENT_MAX_ATTEMPTS, DEFAULT_AGENT_RETRY_BASE_DELAY_MS, DEFAULT_LLM_MODEL,
};
// PlanConfig / ReviewerConfig 由 PlanRuntime / reviewer 分别消费（P1/P4 起），
// 这里先 re-export 保证 `tomcat::infra::config::PlanConfig` 可用而不污染默认 import。
#[allow(unused_imports)]
pub use types::{AskQuestionConfig, PlanConfig, ReviewerConfig, TodosConfig};
#[allow(unused_imports)]
pub use types::{
    ServeConfig, ServeTransport, ToolsBashConfig, ToolsConfig, ToolsReadConfig,
    ToolsWebFetchConfig, ToolsWebSearchConfig, ToolsWriteConfig,
    DEFAULT_SKILLS_MAX_DESCRIPTION_CHARS, DEFAULT_SKILLS_MAX_SKILLS,
    DEFAULT_SKILLS_PROMPT_BUDGET_FLOOR_CHARS, DEFAULT_SKILLS_PROMPT_BUDGET_PCT,
    DEFAULT_SKILLS_SYSTEM_ENABLED, DEFAULT_TOOLS_BASH_MAX_OUTPUT_CHARS,
    DEFAULT_TOOLS_BASH_TIMEOUT_MS, DEFAULT_TOOLS_READ_MAX_BYTES,
    DEFAULT_TOOLS_WEB_FETCH_CACHE_CAPACITY_BYTES, DEFAULT_TOOLS_WEB_FETCH_CACHE_TTL_SECS,
    DEFAULT_TOOLS_WEB_FETCH_MARKDOWN_HEAD_CHARS, DEFAULT_TOOLS_WEB_FETCH_MAX_HTTP_CONTENT_BYTES,
    DEFAULT_TOOLS_WEB_FETCH_MAX_MARKDOWN_CHARS, DEFAULT_TOOLS_WEB_FETCH_MAX_REDIRECTS,
    DEFAULT_TOOLS_WEB_FETCH_TIMEOUT_MS, DEFAULT_TOOLS_WEB_SEARCH_BACKEND,
    DEFAULT_TOOLS_WEB_SEARCH_BRAVE_BASE_URL, DEFAULT_TOOLS_WEB_SEARCH_CACHE_CAPACITY,
    DEFAULT_TOOLS_WEB_SEARCH_CACHE_TTL_SECS, DEFAULT_TOOLS_WEB_SEARCH_COUNT,
    DEFAULT_TOOLS_WEB_SEARCH_SERPER_BASE_URL, DEFAULT_TOOLS_WEB_SEARCH_TAVILY_BASE_URL,
    DEFAULT_TOOLS_WEB_SEARCH_TIMEOUT_MS, DEFAULT_TOOLS_WRITE_NORMALIZE_CRLF,
    MAX_TOOLS_BASH_MAX_OUTPUT_CHARS, MAX_TOOLS_BASH_TIMEOUT_MS,
};
