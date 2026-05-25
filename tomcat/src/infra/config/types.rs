//! 配置类型定义：PermissionLevel 枚举、各 *Config 结构体、Default 实现、默认值辅助函数。

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
}

impl Default for PreflightConfig {
    fn default() -> Self {
        Self {
            auto_install_search_tools: true,
            auto_install_git: true,
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

/// CLI 工具执行行的输出档位（与 `show_thinking` 解耦）。
///
/// - `off`：不打印 `[tool]` 开始/结束行；
/// - `brief`：仅打印结束行（成功/失败摘要 + 耗时）；
/// - `full`：打印开始 + 结束，失败时附加前 3 行 stderr（当前默认行为）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ToolCliVerbosity {
    Off,
    Brief,
    #[default]
    Full,
}

/// LLM 接入配置：提供商、API 地址、密钥环境变量、默认模型、限流与重试。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LlmConfig {
    #[serde(default = "default_llm_provider")]
    pub provider: String,
    #[serde(default)]
    pub api_base: Option<String>,
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default = "default_llm_model")]
    pub default_model: String,
    /// 最大并发 LLM 请求数，0 表示不限制（不推荐）。
    #[serde(default = "default_max_concurrent_requests")]
    pub max_concurrent_requests: u32,
    /// 非流式请求失败时的重试次数（仅对可重试错误如 429/5xx）。
    #[serde(default = "default_llm_retry_count")]
    pub retry_count: u32,
    /// 流式请求单次读取超时秒数。
    #[serde(default = "default_stream_timeout_sec")]
    pub stream_timeout_sec: u64,
    /// 显式 HTTP 代理 URL（如 `http://127.0.0.1:7890`）。设置后所有 LLM 请求经该代理；未设置时仍使用环境变量 HTTPS_PROXY/HTTP_PROXY（若存在）。
    #[serde(default)]
    pub proxy: Option<String>,
    /// 当对主 api_base 请求不通（连接失败、超时等）时，自动用该 URL 重试；示例 `https://api.chatanywhere.tech`。留空则关闭自动降级。
    #[serde(default)]
    pub api_base_fallback: Option<String>,
    /// Thinking / Reasoning 协议接入子配置（T2-P0-006 P5）。
    #[serde(default)]
    pub thinking: ThinkingConfig,
    /// CLI `[tool]` 行输出档位（与 `show_thinking` 独立）。
    #[serde(default)]
    pub tool_cli_verbosity: ToolCliVerbosity,
    /// OpenAI Files 上传子配置（T2-P0-015）。
    #[serde(default)]
    pub files: LlmFilesConfig,
}

/// OpenAI Files 子配置（T2-P0-015）。
///
/// 仅保留最小可配置项：`expires_after_seconds`。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LlmFilesConfig {
    /// 上传时 `expires_after[seconds]`：
    /// - `86400`（默认）= 24h 后服务端自动回收；
    /// - `0` = 不传 `expires_after` 字段，回退 OpenAI 默认策略。
    #[serde(default = "default_llm_files_expires_after_seconds")]
    pub expires_after_seconds: u64,
}

/// Thinking / Reasoning 协议子配置。
///
/// **产品默认（方案 B）**：`enabled = true`、`show = true`、`level = "high"`。
/// 库集成方若希望「装好就和旧行为一致」需显式 `enabled = false`。详见 changelog 与
/// 架构 §3.1 G5。其它字段：
///
/// - `level`: `off | minimal | low | medium | high | xhigh`，由
///   [`crate::core::llm::thinking_policy::ThinkingLevel`] 解析。
/// - `format`: `openai | openrouter | deepseek | zai | qwen | doubao` 等；`None`
///   表示按 provider 名称自动推断。
/// - `max_tokens`: 仅豆包 / Moonshot 等走 `thinking: { type, max_tokens? }` 时生效；
///   `openai-responses` / OpenAI 路径用 `reasoning.effort`，**不写**该字段。
///
/// **`strip_on_resend` 不再暴露给用户 toml**（`#[serde(skip)]`）：是否剥离重放历史
/// 中的 thinking 由 provider / 出站层根据各家 API 规则决定，避免用户开关与网关行为
/// 错配。字段保留供内部 / provider 实现 / 单测使用。
///
/// 详细策略见 `docs/architecture/llm-stream-events-cli-pipeline.md` §4.2.2。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ThinkingConfig {
    /// 全局 thinking 总开关；关闭则其它字段失效（也不发请求）。
    #[serde(default = "default_thinking_enabled")]
    pub enabled: bool,
    /// 强度档位：`off | minimal | low | medium | high | xhigh`。
    /// 字符串形式由 `core/llm/thinking_policy::ThinkingLevel` 解析。
    #[serde(default = "default_thinking_level")]
    pub level: String,
    /// 厂商请求格式：`openai | openrouter | deepseek | zai | qwen | doubao` 等；
    /// `None` 表示按 provider 名称自动推断。
    #[serde(default)]
    pub format: Option<String>,
    /// 仅豆包 / Moonshot 等走 `thinking: { type, max_tokens? }` 时进入请求体；
    /// OpenAI / openai-responses 路径忽略本字段（用 `reasoning.effort`）。
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// CLI 默认是否展示 thinking。运行时优先级：
    /// `PI_CHAT_SHOW_THINKING`（已设置）> 本字段 > 代码默认。
    #[serde(default = "default_thinking_show")]
    pub show: bool,
    /// 是否把 thinking 以独立结构化条目落 transcript（默认 false：仅展示，不持久化）。
    #[serde(default)]
    pub persist: bool,
    /// **不暴露给用户 toml**（`serde(skip)`）：序列化 / 反序列化都跳过；由 provider /
    /// 出站层赋值或在测试里显式构造。语义：多轮重发是否剥离上下文中的 thinking 块。
    #[serde(skip, default = "default_true")]
    pub strip_on_resend: bool,
    /// thinking 是否打到 stderr（默认 false：走 stdout 与正文同流）。`true` 时
    /// `CliTurnRenderer` 把 `[thinking]` 区块改写到 stderr，便于 prompt 抢行场景。
    #[serde(default)]
    pub print_to_stderr: bool,
}

fn default_thinking_level() -> String {
    "high".to_string()
}

fn default_thinking_enabled() -> bool {
    true
}

fn default_thinking_show() -> bool {
    true
}

impl Default for ThinkingConfig {
    fn default() -> Self {
        Self {
            enabled: default_thinking_enabled(),
            level: default_thinking_level(),
            format: None,
            max_tokens: None,
            show: default_thinking_show(),
            persist: false,
            strip_on_resend: true,
            print_to_stderr: false,
        }
    }
}

/// 默认 LLM 后端 id；与 [`crate::core::llm::registered_provider_ids`] 对齐。
/// `"openai-responses"` 走 OpenAI Responses API（`POST /v1/responses`）；
/// 改 `"openai"` 切回 Chat Completions（`POST /v1/chat/completions`）。
fn default_llm_provider() -> String {
    "openai-responses".to_string()
}

/// 全局默认 LLM 模型 id（`LlmConfig` 默认值、`tomcat init` 首次写入与文档一致）。
/// 可通过 `tomcat.config.toml` 中 `[llm] default_model` 或环境变量 `TOMCAT__LLM__DEFAULT_MODEL` 覆盖（后者优先级更高，见 [`load_config`]）。
pub const DEFAULT_LLM_MODEL: &str = "gpt-5.2";

fn default_llm_model() -> String {
    DEFAULT_LLM_MODEL.to_string()
}
fn default_max_concurrent_requests() -> u32 {
    4
}
fn default_llm_retry_count() -> u32 {
    3
}
fn default_stream_timeout_sec() -> u64 {
    60
}
pub const DEFAULT_LLM_FILES_EXPIRES_AFTER_SECONDS: u64 = 86_400;
fn default_llm_files_expires_after_seconds() -> u64 {
    DEFAULT_LLM_FILES_EXPIRES_AFTER_SECONDS
}

impl Default for LlmFilesConfig {
    fn default() -> Self {
        Self {
            expires_after_seconds: default_llm_files_expires_after_seconds(),
        }
    }
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: default_llm_provider(),
            api_base: None,
            api_key_env: Some("OPENAI_API_KEY".to_string()),
            default_model: default_llm_model(),
            max_concurrent_requests: default_max_concurrent_requests(),
            retry_count: default_llm_retry_count(),
            stream_timeout_sec: default_stream_timeout_sec(),
            proxy: None,
            api_base_fallback: None,
            thinking: ThinkingConfig::default(),
            tool_cli_verbosity: ToolCliVerbosity::default(),
            files: LlmFilesConfig::default(),
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

/// 4 原语配置：bash 两档列表 + path_rules 结构化规则。
///
/// **schema 升级（plan §5）**：
/// - 删除 `path_blacklist`（被 `path_rules` 替代，模式更明确）
/// - 删除 `require_approval_for_all_write` / `require_approval_for_all_bash`
///   （`workspace-in-default-allow, workspace-out-confirm` 模型已让它们冗余）
/// - 新增 `path_rules`: `Vec<PathRule>`（结构化路径规则，模式 `deny` / `readonly`）
/// - `bash_forbidden` / `bash_approval_required` 默认转为 regex 字符串列表
///   （编译由 `permission::gate` 在构造时完成）
///
/// 删除 legacy whitelist 配置后，路径允许根只由 `workspace.workspace_roots` 表达；
/// bash 只保留 forbidden / approval_required 两类策略。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PrimitiveConfig {
    /// 结构化路径规则。每条 `path` + `mode`（`deny` / `readonly`）。
    /// 在 gate 模式下与 builtin 规则合并；仅生效，不可弱化 builtin。
    #[serde(default)]
    pub path_rules: Vec<crate::core::permission::PathRule>,
    /// bash 高危但可允许：regex 列表，命中后弹 confirm；与 builtin 合并。
    #[serde(default)]
    pub bash_approval_required: Vec<String>,
    /// bash 禁止：regex 列表，命中即拒绝；与 builtin 合并。
    #[serde(default)]
    pub bash_forbidden: Vec<String>,
    #[serde(default)]
    pub auto_confirm: bool,
    /// `bash` 在 Unix 上 `sh -c` 前可选 source 的 env 脚本路径；`None` 时默认 `$HOME/.wasmedge/env`。
    #[serde(default)]
    pub wasmedge_env_path: Option<String>,
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

fn default_true() -> bool {
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

/// 上下文管理配置：token-aware 滑窗与 Compaction 参数。
/// 详见 `docs/architecture/context-management.md`。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ContextConfig {
    /// LLM 上下文窗口大小（token 数），默认 400,000（GPT-5.2）。
    #[serde(default = "default_context_window")]
    pub context_window: usize,
    /// LLM 最大输出 token 数，默认 128,000。
    #[serde(default = "default_max_output_tokens")]
    pub max_output_tokens: usize,
    /// 每批 Compaction 的最大 user turn 数，默认 10。
    #[serde(default = "default_compaction_turns")]
    pub compaction_turns: usize,
    /// 受保护的最近 user turn 数（不参与任何压缩），默认 3。
    #[serde(default = "default_keep_recent_turns")]
    pub keep_recent_turns: usize,
    /// Compaction 摘要使用的 LLM 模型（可配低成本模型），默认与主模型相同。
    #[serde(default = "default_compaction_model")]
    pub compaction_model: String,
    /// Layer 0 落盘阈值：单条 tool_result 字符数上限，默认 50,000。
    #[serde(default = "default_layer0_single_result_max_chars")]
    pub layer0_single_result_max_chars: usize,
    /// Layer 0 占位符替换阈值：compactable zone 内 > 此值的 tool_result 被替换为占位符，默认 10,000。
    #[serde(default = "default_layer0_placeholder_threshold_chars")]
    pub layer0_placeholder_threshold_chars: usize,
    /// Compaction 摘要最大 token 数（LLM max_tokens 参数），默认 10,000。
    #[serde(default = "default_compaction_max_tokens")]
    pub compaction_max_tokens: usize,
}

fn default_context_window() -> usize {
    400_000
}
fn default_max_output_tokens() -> usize {
    128_000
}
fn default_compaction_turns() -> usize {
    10
}
fn default_keep_recent_turns() -> usize {
    3
}
fn default_compaction_model() -> String {
    DEFAULT_LLM_MODEL.to_string()
}
fn default_layer0_single_result_max_chars() -> usize {
    50_000
}
fn default_layer0_placeholder_threshold_chars() -> usize {
    10_000
}
fn default_compaction_max_tokens() -> usize {
    10_000
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            context_window: default_context_window(),
            max_output_tokens: default_max_output_tokens(),
            compaction_turns: default_compaction_turns(),
            keep_recent_turns: default_keep_recent_turns(),
            compaction_model: default_compaction_model(),
            layer0_single_result_max_chars: default_layer0_single_result_max_chars(),
            layer0_placeholder_threshold_chars: default_layer0_placeholder_threshold_chars(),
            compaction_max_tokens: default_compaction_max_tokens(),
        }
    }
}

/// 计算上下文预算（字符数）。
/// 公式：`(context_window - max_output_tokens) * 4`
/// 其中 `*4` 将 token 转为近似字符数（chars/4 启发式）。
/// 对齐 context-management.md §4.6。
pub fn compute_context_budget_chars(config: &ContextConfig) -> usize {
    let available_tokens = config
        .context_window
        .saturating_sub(config.max_output_tokens);
    available_tokens * 4
}

/// 工具子系统配置：每个内建工具的可调上限聚合在此表，避免 `LlmConfig` / `PrimitiveConfig`
/// 等已有结构再被工具相关字段污染（与 `docs/architecture/tools/read.md` §3.4 对齐）。
///
/// **设计口径**（与 `read.md` §3.4 一致）：
/// - 仅放「磁盘资源 / 安全相关」的硬上限；
/// - **不**放可由 LLM 通过 schema 字段直接控制的开关（如 `line_numbers` / `hashline`），
///   避免管理员侧静默改变模型上下文。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ToolsConfig {
    #[serde(default)]
    pub read: ToolsReadConfig,
    #[serde(default)]
    pub write: ToolsWriteConfig,
    #[serde(default)]
    pub bash: ToolsBashConfig,
}

/// `[tools.read]` 子表：当前仅含 `max_bytes`。
///
/// `max_bytes` 是 **read 工具文本路径的「裸读字节上限」**：
/// - 当模型**未传** `offset` / `limit` 时，先在 `std::fs::metadata().len()` 阶段
///   与该值比对，超限直接返结构化错误，**不**触发任何 `read_to_*`；
/// - 当模型传入 `offset` / `limit`（即明确分窗）时，**不**触发该上限——
///   合理 dump / 大日志可被分窗读取（详见 `read.md` §2.5 决策图）。
///
/// 默认 25 MiB（介于 cc-fork 的 256 KiB 与 pi_agent_rust 的 100 MiB 之间，
/// 兼顾「合理 dump 文件」与「防爆 ctx」），可通过
/// `tomcat.config.toml [tools.read] max_bytes = ...` 或环境变量
/// `TOMCAT__TOOLS__READ__MAX_BYTES` 覆盖。图片 / PDF inline 上限由
/// `core::llm::types` 集中管理，**不**进 config。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolsReadConfig {
    #[serde(default = "default_tools_read_max_bytes")]
    pub max_bytes: u64,
}

/// 25 MiB；read.md §2.5 决策表 R6 #2「自设」入选值。
pub const DEFAULT_TOOLS_READ_MAX_BYTES: u64 = 25 * 1024 * 1024;

fn default_tools_read_max_bytes() -> u64 {
    DEFAULT_TOOLS_READ_MAX_BYTES
}

impl Default for ToolsReadConfig {
    fn default() -> Self {
        Self {
            max_bytes: default_tools_read_max_bytes(),
        }
    }
}

/// `[tools.write]` 子表：当前仅含 `normalize_crlf`（PR-G）。
///
/// 与 `read.md` § 工具子系统配置一致的设计口径：仅放「磁盘 / 安全相关」全局开关；
/// `normalize_crlf` 控制 [`crate::core::tools::primitive::executor::write_edit::write_file_impl`]
/// 写入字节前是否将 `\r\n` 折叠为 `\n`（与 [write.md](../../../docs/architecture/tools/write.md)
/// §3.3 / §8 一致）。**默认 `true`**：跨平台仓库统一收 `\n`，行为与
/// pi-mono / cc-fork-01 同档。
///
/// **schema 决策（write.md §4.1）**：**不**新增 per-call `normalize_line_endings?` 字段，
/// 避免 schema 多一维让 LLM 混淆；用户可通过 `tomcat.config.toml [tools.write] normalize_crlf = false`
/// 或环境变量 `TOMCAT__TOOLS__WRITE__NORMALIZE_CRLF=false` 关掉。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolsWriteConfig {
    #[serde(default = "default_tools_write_normalize_crlf")]
    pub normalize_crlf: bool,
}

/// 默认开启 LF 规范化（write.md §3.3 / §8 决策表）。
pub const DEFAULT_TOOLS_WRITE_NORMALIZE_CRLF: bool = true;

fn default_tools_write_normalize_crlf() -> bool {
    DEFAULT_TOOLS_WRITE_NORMALIZE_CRLF
}

impl Default for ToolsWriteConfig {
    fn default() -> Self {
        Self {
            normalize_crlf: default_tools_write_normalize_crlf(),
        }
    }
}

/// `[tools.bash]` 子表（T2-P0-016 PR-E / `bash.md` §8）。
///
/// 与 `read` / `write` 同口径，仅放「磁盘资源 / 安全相关」全局开关，**不**放可由 LLM
/// 直接通过 schema 字段控制的开关。
///
/// - `timeout_ms`：默认墙钟超时（毫秒），由 [`crate::core::tools::primitive::executor::bash`]
///   传给 `tokio::time::timeout(..., child.wait())`。配置默认 [`DEFAULT_TOOLS_BASH_TIMEOUT_MS`]
///   = 120_000（2 分钟，对齐 bash.md §2.4.3 / §6.2）；模型可在 schema `timeout_ms` 字段
///   显式上调，但 [`crate::core::agent_loop::tool_exec`] 会以
///   [`MAX_TOOLS_BASH_TIMEOUT_MS`] = 600_000（10 分钟）封顶。
/// - `max_output_chars`：单次 bash 调用 stdout / stderr **合并后字符数**上限（与
///   bash.md §8 / cc-fork-01 `BASH_MAX_OUTPUT_DEFAULT=30_000` 同档）。超限走
///   `EndTruncatingAccumulator` 风格头尾保留 + 整段落盘
///   `~/.tomcat/agents/<id>/tool-results/...`，调用回执带 `truncated=true` /
///   `persisted_output_path`（详见 bash.md §2.4.3）。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolsBashConfig {
    #[serde(default = "default_tools_bash_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_tools_bash_max_output_chars")]
    pub max_output_chars: usize,
}

/// 默认 bash 墙钟超时：120_000 ms（2 分钟）。bash.md §2.4.3 / §6.2 / §9.2 钉死。
pub const DEFAULT_TOOLS_BASH_TIMEOUT_MS: u64 = 120_000;

/// bash 墙钟超时上限：600_000 ms（10 分钟）。schema `timeout_ms` 大于此值会被 `tool_exec`
/// 在解析阶段 clamp（避免把任何模型可见上限漂移到 catalog schema 之外）。
///
/// **Phase-E.1**：仅声明；**Phase-E.2** 在 [`crate::core::agent_loop::tool_exec`] 与
/// [`crate::core::tools::primitive::executor::bash`] 中作为 clamp 使用。
#[allow(dead_code)]
pub const MAX_TOOLS_BASH_TIMEOUT_MS: u64 = 600_000;

/// 默认 bash 输出字符上限：30_000（cc-fork-01 `BASH_MAX_OUTPUT_DEFAULT` 同档）。
pub const DEFAULT_TOOLS_BASH_MAX_OUTPUT_CHARS: usize = 30_000;

/// bash 输出字符上限的硬上限：150_000（cc-fork-01 `BASH_MAX_OUTPUT_UPPER_LIMIT` 同档），
/// 用于 [`ToolsBashConfig`] 反序列化后的越界保护（可在 [`crate::infra::config::load`] 校验）。
///
/// **Phase-E.1**：仅声明；**Phase-E.3** 由 `output_accum` 在合并流时作为硬上限引用。
#[allow(dead_code)]
pub const MAX_TOOLS_BASH_MAX_OUTPUT_CHARS: usize = 150_000;

fn default_tools_bash_timeout_ms() -> u64 {
    DEFAULT_TOOLS_BASH_TIMEOUT_MS
}

fn default_tools_bash_max_output_chars() -> usize {
    DEFAULT_TOOLS_BASH_MAX_OUTPUT_CHARS
}

impl Default for ToolsBashConfig {
    fn default() -> Self {
        Self {
            timeout_ms: default_tools_bash_timeout_ms(),
            max_output_chars: default_tools_bash_max_output_chars(),
        }
    }
}

/// Wasm 运行时配置（feature "wasmedge" 时使用）。
/// quickjs wasm 路径由 [`resolve_quickjs_path`] 从 work_dir 推导，回退到环境变量。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct WasmConfig {}

/// `[plan]` 子表（T2-P1-002 PR-PLA/PLB）：PLAN 模式运行时全局参数。
///
/// 设计口径：仅放「锁等待 / 自动 checkpoint 开关」这类**运行时与磁盘资源**相关的全局
/// 参数；reviewer 子流程参数独立到 [`ReviewerConfig`]，避免单表字段膨胀。
///
/// 详见 `docs/architecture/plan-runtime.md`（PR-PLB / PR-PLE）。
///
/// env 覆盖：
/// - `TOMCAT_PLAN_FILE_LOCK_TIMEOUT_MS` → `lock_timeout_ms`
/// - `TOMCAT_PLAN_AUTO_CHECKPOINT_ON_BUILD` → `auto_checkpoint_on_build`
/// - `TOMCAT_PLAN_MAX_REVIEW_ROUNDS` → `max_review_rounds`
///
/// `verify_gate` 暂不提供 env 覆盖：只走 `[plan].verify_gate = "soft" | "gate"`。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PlanConfig {
    /// `~/.tomcat/plans/*.plan.md` advisory lock 等待上限（毫秒）。默认 2000。
    #[serde(default = "default_plan_lock_timeout_ms")]
    pub lock_timeout_ms: u64,
    /// `/plan build` 时是否自动打 `CheckpointKind::Manual{label:"plan_build:<id>"}`。默认 false。
    #[serde(default)]
    pub auto_checkpoint_on_build: bool,
    /// 单 plan 累计 review 软上限：超过时仅 transcript warning，**不**阻 `create_plan` / `/plan build`。
    /// 默认 1（仅首次 create_plan 必跑；后续 update_plan 不再触发，由调用方控制）。
    #[serde(default = "default_plan_max_review_rounds")]
    pub max_review_rounds: u32,
    /// EXEC 完成前 code reviewer 的最大尝试轮次。默认 1；0 表示直接跳过 code review。
    #[serde(default = "default_plan_max_code_review_rounds")]
    pub max_code_review_rounds: u32,
    /// Verifier gate 模式：`soft`（默认，FAIL 仅 advisory）或 `gate`（FAIL 阻止 completed）。
    #[serde(default = "default_plan_verify_gate")]
    pub verify_gate: String,
}

fn default_plan_lock_timeout_ms() -> u64 {
    2000
}

fn default_plan_max_review_rounds() -> u32 {
    1
}

fn default_plan_max_code_review_rounds() -> u32 {
    1
}

fn default_plan_verify_gate() -> String {
    "soft".to_string()
}

impl Default for PlanConfig {
    fn default() -> Self {
        Self {
            lock_timeout_ms: default_plan_lock_timeout_ms(),
            auto_checkpoint_on_build: false,
            max_review_rounds: default_plan_max_review_rounds(),
            max_code_review_rounds: default_plan_max_code_review_rounds(),
            verify_gate: default_plan_verify_gate(),
        }
    }
}

/// `[ask_question]` 子表：`ask_question` 工具运行时参数。
///
/// env 覆盖：
/// - `TOMCAT_ASK_QUESTION_TIMEOUT_MS` → `timeout_ms`
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AskQuestionConfig {
    /// 一次 ask_question 调用等待用户输入的墙钟超时（毫秒）。0 表示无超时。默认 300_000 ms = 5 min。
    #[serde(default = "default_ask_question_timeout_ms")]
    pub timeout_ms: u64,
}

fn default_ask_question_timeout_ms() -> u64 {
    300_000
}

impl Default for AskQuestionConfig {
    fn default() -> Self {
        Self {
            timeout_ms: default_ask_question_timeout_ms(),
        }
    }
}

/// `[todos]` 子表：session-local todos 持久化与生命周期参数（GAP-N12 / G3）。
///
/// env 覆盖：
/// - `TOMCAT_TODOS_PURGE_INACTIVE_ON_NEW_TODOS` → `purge_inactive_on_new_todos`
/// - `TOMCAT_TODOS_AUTO_NEW_TODOS_ON_REPLACE_AFTER_TERMINAL` → `auto_new_todos_on_replace_after_terminal`
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TodosConfig {
    /// 新建 todos 时是否清理 session 下「无 active 项 / 历史 inactive」的旧 todos 文件。默认 true。
    #[serde(default = "default_true")]
    pub purge_inactive_on_new_todos: bool,
    /// 当上一个 active todos 进入 terminal（全 completed / cancelled）后，下一次 todos replace 调用是否自动
    /// 视为「new todos」开启新文件（而不是 in-place 覆盖原文件）。默认 true。
    #[serde(default = "default_true")]
    pub auto_new_todos_on_replace_after_terminal: bool,
}

impl Default for TodosConfig {
    fn default() -> Self {
        Self {
            purge_inactive_on_new_todos: true,
            auto_new_todos_on_replace_after_terminal: true,
        }
    }
}

/// `[reviewer]` 子表（T2-P1-004 RV-A/B/E）：reviewer 内联子 Agent 派发参数。
///
/// reviewer 子 Agent 走 `AgentRegistry::spawn_subagent_internal`，**不**进 LLM catalog；
/// 本表仅控制：子 loop 最大轮次、（可选）覆盖父 model。
///
/// **改稿权 (`allow_review_edit`) 已固定为 `true`**——实现层硬编码，不再提供配置项。
/// 历史变更：移除 `TOMCAT_REVIEWER_DEFAULT_ALLOW_EDIT` / `[reviewer].default_allow_edit`。
///
/// env 覆盖（plan §P0.5 / reviewer §11）：
/// - `TOMCAT_REVIEWER_MAX_TURNS` → `max_turns`（默认 64）
/// - `TOMCAT_REVIEWER_MODEL` → `model_override`
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ReviewerConfig {
    /// reviewer 子 loop 最大 LLM reasoning 轮次（映射到 `AgentLoopConfig.max_tool_rounds`）。
    /// 默认 64；transcript 落 `reviewer_turns_used/limit/stop_reason` 便于调参。
    #[serde(default = "default_reviewer_max_turns")]
    pub max_turns: u32,
    /// 显式覆盖 reviewer 子 Agent 使用的 LLM 模型；`None` 时继承父 Agent。
    #[serde(default)]
    pub model_override: Option<String>,
}

fn default_reviewer_max_turns() -> u32 {
    64
}

impl Default for ReviewerConfig {
    fn default() -> Self {
        Self {
            max_turns: default_reviewer_max_turns(),
            model_override: None,
        }
    }
}

/// 应用顶层配置，聚合 log / llm / storage / agent / plugin / security / primitive / wasm 子配置。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct AppConfig {
    #[serde(default)]
    pub log: LogConfig,
    #[serde(default)]
    pub preflight: PreflightConfig,
    #[serde(default)]
    pub checkpoint: CheckpointConfig,
    #[serde(default)]
    pub llm: LlmConfig,
    #[serde(default)]
    pub storage: StorageConfig,
    #[serde(default)]
    pub agent: AgentConfig,
    #[serde(default)]
    pub plugin: PluginConfig,
    #[serde(default)]
    pub workspace: WorkspaceConfig,
    #[serde(default)]
    pub security: SecurityConfig,
    #[serde(default)]
    pub primitive: PrimitiveConfig,
    #[serde(default)]
    pub context: ContextConfig,
    #[serde(default)]
    pub tools: ToolsConfig,
    #[serde(default)]
    pub wasm: WasmConfig,
    /// PLAN 模式运行时全局参数（T2-P1-002 PR-PLA/PLB）。
    #[serde(default)]
    pub plan: PlanConfig,
    /// reviewer 内联子 Agent 派发参数（T2-P1-004 RV-A/B/E）。
    #[serde(default)]
    pub reviewer: ReviewerConfig,
    /// `ask_question` 工具参数（GAP-N12）。
    #[serde(default)]
    pub ask_question: AskQuestionConfig,
    /// session-local todos 工具参数（GAP-N12 / G3）。
    #[serde(default)]
    pub todos: TodosConfig,
}
