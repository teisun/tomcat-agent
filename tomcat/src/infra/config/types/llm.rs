use serde::{Deserialize, Deserializer, Serialize};

use super::core::default_true;

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

/// Thinking 在 CLI 中的显示档位。
///
/// - `minimal`：只打一行 `[thinking] ...` 占位，不流式正文；
/// - `summary`：流式显示 summary，隐藏 raw；
/// - `full`：流式显示 summary + raw。
///
/// 反序列化兼容历史 bool：
/// - `false` -> `summary`
/// - `true` -> `full`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[repr(u8)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingDisplay {
    Minimal = 0,
    #[default]
    Summary = 1,
    Full = 2,
}

impl ThinkingDisplay {
    pub fn from_legacy_bool(value: bool) -> Self {
        if value {
            Self::Full
        } else {
            Self::Summary
        }
    }

    pub fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::Minimal,
            1 => Self::Summary,
            2 => Self::Full,
            _ => Self::Summary,
        }
    }

    pub fn as_u8(self) -> u8 {
        self as u8
    }

    pub fn next_cycle(self) -> Self {
        match self {
            Self::Summary => Self::Full,
            Self::Full => Self::Minimal,
            Self::Minimal => Self::Summary,
        }
    }

    pub fn shows_summary(self) -> bool {
        matches!(self, Self::Summary | Self::Full)
    }

    pub fn shows_raw(self) -> bool {
        matches!(self, Self::Full)
    }
}

impl<'de> Deserialize<'de> for ThinkingDisplay {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Bool(bool),
            String(String),
        }

        match Repr::deserialize(deserializer)? {
            Repr::Bool(v) => Ok(ThinkingDisplay::from_legacy_bool(v)),
            Repr::String(v) => match v.trim().to_ascii_lowercase().as_str() {
                "minimal" => Ok(ThinkingDisplay::Minimal),
                "summary" => Ok(ThinkingDisplay::Summary),
                "full" => Ok(ThinkingDisplay::Full),
                other => Err(serde::de::Error::custom(format!(
                    "unknown thinking display `{other}`; expected minimal|summary|full"
                ))),
            },
        }
    }
}

/// LLM 接入配置：提供商、API 地址、密钥环境变量、默认模型、限流、重试与多层超时。
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
    /// 整次 HTTP 请求总超时（秒）；0 表示不限制。
    #[serde(default = "default_http_timeout_sec")]
    pub http_timeout_sec: u64,
    /// 流式 SSE chunk 空闲超时（秒）；0 表示关闭。
    #[serde(default = "default_stream_timeout_sec")]
    pub stream_timeout_sec: u64,
    /// 非流式请求无进展 watchdog（秒）；0 表示关闭。
    #[serde(default = "default_non_stream_stale_timeout_sec")]
    pub non_stream_stale_timeout_sec: u64,
    /// socket 级 read 超时（秒）；0 表示关闭。
    #[serde(default = "default_http_read_timeout_sec")]
    pub http_read_timeout_sec: u64,
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
/// **产品默认**：`enabled = true`、`show = "summary"`、`level = "high"`。
/// 新默认会流式显示 summary、隐藏 raw；若希望更安静的占位模式，设 `show = "minimal"`；
/// 若希望完全静默 thinking，需显式 `enabled = false`。详见 changelog 与架构 §3.1 G5。
/// 其它字段：
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
    /// CLI thinking 显示档位：`minimal | summary | full`。
    ///
    /// 历史兼容：
    /// - `show = false` -> `summary`
    /// - `show = true` -> `full`
    ///
    /// 运行时优先级：
    /// `PI_CHAT_SHOW_THINKING`（已设置，支持 `minimal|summary|full`，也兼容旧 `0/1`） >
    /// 本字段 > 代码默认。
    #[serde(default = "default_thinking_show")]
    pub show: ThinkingDisplay,
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

fn default_thinking_show() -> ThinkingDisplay {
    ThinkingDisplay::Summary
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
pub const DEFAULT_LLM_MODEL: &str = "gpt-5.4";

fn default_llm_model() -> String {
    DEFAULT_LLM_MODEL.to_string()
}

fn default_max_concurrent_requests() -> u32 {
    4
}

fn default_llm_retry_count() -> u32 {
    3
}

pub const DEFAULT_LLM_HTTP_TIMEOUT_SEC: u64 = 1_800;
pub const DEFAULT_LLM_STREAM_TIMEOUT_SEC: u64 = 180;
pub const DEFAULT_LLM_NON_STREAM_STALE_TIMEOUT_SEC: u64 = 300;
pub const DEFAULT_LLM_HTTP_READ_TIMEOUT_SEC: u64 = 120;

fn default_http_timeout_sec() -> u64 {
    DEFAULT_LLM_HTTP_TIMEOUT_SEC
}

fn default_stream_timeout_sec() -> u64 {
    DEFAULT_LLM_STREAM_TIMEOUT_SEC
}

fn default_non_stream_stale_timeout_sec() -> u64 {
    DEFAULT_LLM_NON_STREAM_STALE_TIMEOUT_SEC
}

fn default_http_read_timeout_sec() -> u64 {
    DEFAULT_LLM_HTTP_READ_TIMEOUT_SEC
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
            http_timeout_sec: default_http_timeout_sec(),
            stream_timeout_sec: default_stream_timeout_sec(),
            non_stream_stale_timeout_sec: default_non_stream_stale_timeout_sec(),
            http_read_timeout_sec: default_http_read_timeout_sec(),
            proxy: None,
            api_base_fallback: None,
            thinking: ThinkingConfig::default(),
            tool_cli_verbosity: ToolCliVerbosity::default(),
            files: LlmFilesConfig::default(),
        }
    }
}
