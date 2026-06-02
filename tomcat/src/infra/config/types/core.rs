use serde::{Deserialize, Serialize};

/// 插件或操作的权限等级，用于 4 原语与工具访问控制。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PermissionLevel {
    /// 受限，仅允许白名单内操作。
    #[default]
    Restricted,
    /// 普通，需审批的操作按配置处理。
    Normal,
    /// 可信，放宽审批要求。
    Trusted,
}

/// 日志配置：级别、是否写文件。文件目录由 [`resolve_log_dir`] 推导；按日滚动、文件名前缀 `tomcat`（见 [`crate::init_logging`]）。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LogConfig {
    #[serde(default = "default_log_level")]
    pub level: String,
    #[serde(default)]
    pub file_enabled: bool,
}

fn default_log_level() -> String {
    "warn".to_string()
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            file_enabled: false,
        }
    }
}

/// 运行前预检配置：控制 chat 入口是否后台尝试安装增强型外部工具。
///
/// 预检使用 [`std::process::Command::output`]，**无宿主侧超时**；与 Tier2 搜索环境变量
/// `PI_SEARCH_TIER2_DEADLINE_MS`（仅 `search_files` 兜底）无关。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PreflightConfig {
    /// 是否在 `tomcat chat` 入口后台探测并尝试安装 search_files 的 Tier1 依赖（rg/fd）。
    #[serde(default = "default_true")]
    pub auto_install_search_tools: bool,
    /// 是否在 `tomcat chat` 入口后台探测并尝试安装 git。
    #[serde(default = "default_true")]
    pub auto_install_git: bool,
    /// 是否在 chat CLI 中显示 search_tools preflight 的 `[tools]` 提示。
    #[serde(default)]
    pub show_search_tools_ui: bool,
    /// 是否在 chat CLI 中显示 git preflight 的 `[git]` 提示。
    #[serde(default)]
    pub show_git_ui: bool,
}

impl Default for PreflightConfig {
    fn default() -> Self {
        Self {
            auto_install_search_tools: true,
            auto_install_git: true,
            show_search_tools_ui: false,
            show_git_ui: false,
        }
    }
}

/// Checkpoint 配置：仅暴露 retention 策略。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CheckpointConfig {
    #[serde(default = "default_checkpoint_retention_max")]
    pub retention_max: usize,
    #[serde(default = "default_checkpoint_retention_days")]
    pub retention_days: u32,
}

fn default_checkpoint_retention_max() -> usize {
    50
}

fn default_checkpoint_retention_days() -> u32 {
    7
}

impl Default for CheckpointConfig {
    fn default() -> Self {
        Self {
            retention_max: default_checkpoint_retention_max(),
            retention_days: default_checkpoint_retention_days(),
        }
    }
}

/// 存储配置：仅 work_dir。agent 系统子目录由 resolve 函数从 work_dir 推导。
/// 详见 docs/architecture/work-dir-and-data-layout.md。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct StorageConfig {
    /// 工作根目录；默认 `~/.tomcat/`。支持 `~` 与相对路径。
    #[serde(default)]
    pub work_dir: Option<String>,
}

/// Agent 配置：标识、身份目录、工作区目录。
/// `agent_dir` 和 `workspace` 均为可选，未配置时由 resolve 函数从 work_dir 推导。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AgentConfig {
    /// agent 标识，影响运行时数据目录和 session key 命名。
    #[serde(default = "default_agent_id")]
    pub id: String,
    /// agent 身份与凭据目录（auth-profiles.json、models.json 等）。
    /// 未配置时从 work_dir 推导为 `{work_dir}/agents/{id}/agent`。
    #[serde(default)]
    pub agent_dir: Option<String>,
    /// agent 工作区目录（AGENTS.md、SOUL.md 等设计态文件）。
    /// 未配置时从 work_dir 推导为 `{work_dir}/workspace-{id}`。
    #[serde(default)]
    pub workspace: Option<String>,
}

fn default_agent_id() -> String {
    "main".to_string()
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            id: default_agent_id(),
            agent_dir: None,
            workspace: None,
        }
    }
}

/// 插件配置：启动时自动加载的插件列表。插件目录由 [`resolve_plugins_dir`] 从 work_dir 推导。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PluginConfig {
    #[serde(default)]
    pub auto_load: Vec<String>,
}

/// 全局工作区授权：额外可访问根路径列表，**所有 agent 共用**，与 `[agent]` 下的 `workspace`（设计态目录）不同。
///
/// 持久化在 `tomcat.config.toml` 的 `[workspace]` 表；由 `tomcat workspace add/list/remove` 或手编维护。
///
/// `entries` 是 v2 富格式（每项含 path / alias / description），与 `workspace_roots`（仅路径）
/// 同时支持；解析时合并去重。新代码请优先使用 `entries`。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct WorkspaceConfig {
    /// v1 兼容：每项为路径字符串（通常为绝对路径）；空串在解析时忽略。
    #[serde(default)]
    pub workspace_roots: Vec<String>,
    /// v2 富格式：每项含 path / alias / description（与 `workspace_roots` 合并）。
    #[serde(default)]
    pub entries: Vec<WorkspaceEntry>,
}

/// 富格式工作区条目（[`WorkspaceConfig::entries`] 元素）。
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct WorkspaceEntry {
    /// 路径（绝对，含 `~` 前缀）。
    pub path: String,
    /// 可选别名，便于 LLM 在对话中引用。
    #[serde(default)]
    pub alias: Option<String>,
    /// 可选说明，便于审计与回顾。
    #[serde(default)]
    pub description: Option<String>,
}

/// 安全与审计配置：默认插件权限、审计日志开关与保留天数、安全扫描等。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SecurityConfig {
    #[serde(default)]
    pub default_plugin_permission_level: PermissionLevel,
    #[serde(default = "default_true")]
    pub enable_audit_log: bool,
    #[serde(default = "default_audit_retention_days")]
    pub audit_log_retention_days: u32,
    #[serde(default)]
    pub enable_plugin_safety_scan: bool,
}

pub(super) fn default_true() -> bool {
    true
}

fn default_audit_retention_days() -> u32 {
    90
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            default_plugin_permission_level: PermissionLevel::Restricted,
            enable_audit_log: true,
            audit_log_retention_days: 90,
            enable_plugin_safety_scan: false,
        }
    }
}

/// Wasm 运行时配置（feature "wasmedge" 时使用）。
/// quickjs wasm 路径由 [`resolve_quickjs_path`] 从 work_dir 推导，回退到环境变量。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct WasmConfig {}
