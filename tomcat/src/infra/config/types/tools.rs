use serde::{Deserialize, Serialize};

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
    #[serde(default)]
    pub web_search: ToolsWebSearchConfig,
    #[serde(default)]
    pub web_fetch: ToolsWebFetchConfig,
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

/// `[tools.bash]` 子表：前台观察窗口与输出资源上限。
///
/// - `foreground_wait_ms`：一次 `bash` 调用在前台观察 tracked process 的时间，合法范围
///   为 [`MIN_TOOLS_BASH_FOREGROUND_WAIT_MS`]..=[`MAX_TOOLS_BASH_FOREGROUND_WAIT_MS`]，
///   默认 16 秒。等待窗口到期后进程继续作为 tracked background task 运行，不会被终止；
/// - `max_output_chars`：stdout / stderr 各自的内存字符上限。超限采用头尾保留，
///   完整输出持久化到 agent trail 的 `tool-results` 目录。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ToolsBashConfig {
    #[serde(default = "default_tools_bash_foreground_wait_ms")]
    pub foreground_wait_ms: u64,
    #[serde(default = "default_tools_bash_max_output_chars")]
    pub max_output_chars: usize,
}

/// 默认前台观察 16 秒；所有入口统一限制在 8–16 秒。
pub const DEFAULT_TOOLS_BASH_FOREGROUND_WAIT_MS: u64 = 16_000;
/// 前台观察窗口下界：8 秒。
pub const MIN_TOOLS_BASH_FOREGROUND_WAIT_MS: u64 = 8_000;
/// 前台观察窗口上界：16 秒。
pub const MAX_TOOLS_BASH_FOREGROUND_WAIT_MS: u64 = 16_000;

/// 默认 bash 输出字符上限：30_000（cc-fork-01 `BASH_MAX_OUTPUT_DEFAULT` 同档）。
pub const DEFAULT_TOOLS_BASH_MAX_OUTPUT_CHARS: usize = 30_000;

/// bash 输出字符上限的硬上限：150_000（cc-fork-01 `BASH_MAX_OUTPUT_UPPER_LIMIT` 同档），
/// 由配置校验拒绝越界值，执行层仍保留硬上限防御。
pub const MAX_TOOLS_BASH_MAX_OUTPUT_CHARS: usize = 150_000;

fn default_tools_bash_foreground_wait_ms() -> u64 {
    DEFAULT_TOOLS_BASH_FOREGROUND_WAIT_MS
}

fn default_tools_bash_max_output_chars() -> usize {
    DEFAULT_TOOLS_BASH_MAX_OUTPUT_CHARS
}

impl Default for ToolsBashConfig {
    fn default() -> Self {
        Self {
            foreground_wait_ms: default_tools_bash_foreground_wait_ms(),
            max_output_chars: default_tools_bash_max_output_chars(),
        }
    }
}

/// `[tools.web_search]` 子表（T2-P1-012 PR-WS-S）。
///
/// 仅承载 `web_search` runtime 的**默认 backend / filter / cache / timeout / base URL**
/// 配置；provider credentials 继续通过 `TAVILY_API_KEY` / `BRAVE_API_KEY` /
/// `SERPER_API_KEY` 等环境变量读取，不进 TOML。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolsWebSearchConfig {
    #[serde(default = "default_tools_web_search_backend")]
    pub backend: String,
    #[serde(default = "default_tools_web_search_count")]
    pub count: u32,
    #[serde(default)]
    pub freshness: Option<String>,
    #[serde(default)]
    pub country: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub domain_filter: Vec<String>,
    #[serde(default)]
    pub blocked_domains: Vec<String>,
    #[serde(default)]
    pub allowed_domains: Vec<String>,
    #[serde(default = "default_tools_web_search_cache_ttl_secs")]
    pub cache_ttl_secs: u64,
    #[serde(default = "default_tools_web_search_cache_capacity")]
    pub cache_capacity: u64,
    #[serde(default = "default_tools_web_search_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_tools_web_search_tavily_base_url")]
    pub tavily_base_url: String,
    #[serde(default = "default_tools_web_search_brave_base_url")]
    pub brave_base_url: String,
    #[serde(default = "default_tools_web_search_serper_base_url")]
    pub serper_base_url: String,
}

pub const DEFAULT_TOOLS_WEB_SEARCH_BACKEND: &str = "auto";
pub const DEFAULT_TOOLS_WEB_SEARCH_COUNT: u32 = 5;
pub const DEFAULT_TOOLS_WEB_SEARCH_CACHE_TTL_SECS: u64 = 300;
pub const DEFAULT_TOOLS_WEB_SEARCH_CACHE_CAPACITY: u64 = 50;
pub const DEFAULT_TOOLS_WEB_SEARCH_TIMEOUT_MS: u64 = 12_000;
pub const DEFAULT_TOOLS_WEB_SEARCH_TAVILY_BASE_URL: &str = "https://api.tavily.com";
pub const DEFAULT_TOOLS_WEB_SEARCH_BRAVE_BASE_URL: &str = "https://api.search.brave.com";
pub const DEFAULT_TOOLS_WEB_SEARCH_SERPER_BASE_URL: &str = "https://google.serper.dev";

fn default_tools_web_search_backend() -> String {
    DEFAULT_TOOLS_WEB_SEARCH_BACKEND.to_string()
}

fn default_tools_web_search_count() -> u32 {
    DEFAULT_TOOLS_WEB_SEARCH_COUNT
}

fn default_tools_web_search_cache_ttl_secs() -> u64 {
    DEFAULT_TOOLS_WEB_SEARCH_CACHE_TTL_SECS
}

fn default_tools_web_search_cache_capacity() -> u64 {
    DEFAULT_TOOLS_WEB_SEARCH_CACHE_CAPACITY
}

fn default_tools_web_search_timeout_ms() -> u64 {
    DEFAULT_TOOLS_WEB_SEARCH_TIMEOUT_MS
}

fn default_tools_web_search_tavily_base_url() -> String {
    DEFAULT_TOOLS_WEB_SEARCH_TAVILY_BASE_URL.to_string()
}

fn default_tools_web_search_brave_base_url() -> String {
    DEFAULT_TOOLS_WEB_SEARCH_BRAVE_BASE_URL.to_string()
}

fn default_tools_web_search_serper_base_url() -> String {
    DEFAULT_TOOLS_WEB_SEARCH_SERPER_BASE_URL.to_string()
}

impl Default for ToolsWebSearchConfig {
    fn default() -> Self {
        Self {
            backend: default_tools_web_search_backend(),
            count: default_tools_web_search_count(),
            freshness: None,
            country: None,
            language: None,
            domain_filter: Vec::new(),
            blocked_domains: Vec::new(),
            allowed_domains: Vec::new(),
            cache_ttl_secs: default_tools_web_search_cache_ttl_secs(),
            cache_capacity: default_tools_web_search_cache_capacity(),
            timeout_ms: default_tools_web_search_timeout_ms(),
            tavily_base_url: default_tools_web_search_tavily_base_url(),
            brave_base_url: default_tools_web_search_brave_base_url(),
            serper_base_url: default_tools_web_search_serper_base_url(),
        }
    }
}

/// `[tools.web_fetch]` 子表（T2-P1-013 PR-WF-A/S/B）。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolsWebFetchConfig {
    #[serde(default = "default_tools_web_fetch_max_redirects")]
    pub max_redirects: usize,
    #[serde(default = "default_tools_web_fetch_timeout_ms")]
    pub fetch_timeout_ms: u64,
    #[serde(default = "default_tools_web_fetch_max_http_content_bytes")]
    pub max_http_content_bytes: usize,
    #[serde(default = "default_tools_web_fetch_max_markdown_chars")]
    pub max_markdown_chars: usize,
    #[serde(default = "default_tools_web_fetch_markdown_head_chars")]
    pub markdown_head_chars: usize,
    #[serde(default = "default_tools_web_fetch_cache_ttl_secs")]
    pub cache_ttl_secs: u64,
    #[serde(default = "default_tools_web_fetch_cache_capacity_bytes")]
    pub cache_capacity_bytes: u64,
    #[serde(default)]
    pub use_llm_processing: bool,
}

pub const DEFAULT_TOOLS_WEB_FETCH_MAX_REDIRECTS: usize = 10;
pub const DEFAULT_TOOLS_WEB_FETCH_TIMEOUT_MS: u64 = 60_000;
pub const DEFAULT_TOOLS_WEB_FETCH_MAX_HTTP_CONTENT_BYTES: usize = 10 * 1024 * 1024;
pub const DEFAULT_TOOLS_WEB_FETCH_MAX_MARKDOWN_CHARS: usize = 100_000;
pub const DEFAULT_TOOLS_WEB_FETCH_MARKDOWN_HEAD_CHARS: usize = 2_000;
pub const DEFAULT_TOOLS_WEB_FETCH_CACHE_TTL_SECS: u64 = 900;
pub const DEFAULT_TOOLS_WEB_FETCH_CACHE_CAPACITY_BYTES: u64 = 50 * 1024 * 1024;

fn default_tools_web_fetch_max_redirects() -> usize {
    DEFAULT_TOOLS_WEB_FETCH_MAX_REDIRECTS
}

fn default_tools_web_fetch_timeout_ms() -> u64 {
    DEFAULT_TOOLS_WEB_FETCH_TIMEOUT_MS
}

fn default_tools_web_fetch_max_http_content_bytes() -> usize {
    DEFAULT_TOOLS_WEB_FETCH_MAX_HTTP_CONTENT_BYTES
}

fn default_tools_web_fetch_max_markdown_chars() -> usize {
    DEFAULT_TOOLS_WEB_FETCH_MAX_MARKDOWN_CHARS
}

fn default_tools_web_fetch_markdown_head_chars() -> usize {
    DEFAULT_TOOLS_WEB_FETCH_MARKDOWN_HEAD_CHARS
}

fn default_tools_web_fetch_cache_ttl_secs() -> u64 {
    DEFAULT_TOOLS_WEB_FETCH_CACHE_TTL_SECS
}

fn default_tools_web_fetch_cache_capacity_bytes() -> u64 {
    DEFAULT_TOOLS_WEB_FETCH_CACHE_CAPACITY_BYTES
}

impl Default for ToolsWebFetchConfig {
    fn default() -> Self {
        Self {
            max_redirects: default_tools_web_fetch_max_redirects(),
            fetch_timeout_ms: default_tools_web_fetch_timeout_ms(),
            max_http_content_bytes: default_tools_web_fetch_max_http_content_bytes(),
            max_markdown_chars: default_tools_web_fetch_max_markdown_chars(),
            markdown_head_chars: default_tools_web_fetch_markdown_head_chars(),
            cache_ttl_secs: default_tools_web_fetch_cache_ttl_secs(),
            cache_capacity_bytes: default_tools_web_fetch_cache_capacity_bytes(),
            use_llm_processing: false,
        }
    }
}
