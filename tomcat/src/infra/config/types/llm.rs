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
/// 关于 source 分类：只有 OpenAI Responses 才有独立的 summary / raw 双流；
/// chat-completions 类 provider（deepseek/mimo/doubao 等）只有单一 reasoning 流，
/// 统一标记为 `summary`，因此在默认 `summary` 档位即可显示其思考——它们没有
/// 任何会被 `raw` 过滤吞掉的内容。
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

/// LLM 配置：
/// - 选择层：`default_model` / `vision_model` / `title_model`
/// - 运行时层：并发 / 重试 / 超时 / proxy / files / continuity 等全局旋钮
///
/// 模型如何连接（`api` / `provider` / `api_key_env` / `base_url`）不再出现在这里，
/// 全部收敛到 `models.toml` 的 [`crate::core::llm::ModelEntry`]。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LlmConfig {
    #[serde(default = "default_llm_model")]
    pub default_model: String,
    /// Vision 场景模型；未设置时回落主模型，由 resolver 负责 capability guard。
    #[serde(default)]
    pub vision_model: Option<String>,
    /// 标题生成场景模型；未设置时回落 compaction_model / default_model。
    #[serde(default)]
    pub title_model: Option<String>,
    /// 仅供 `#[cfg(test)]` 单测维持最小迁移成本的隐藏覆写字段：
    /// - 不参与真实配置反序列化；
    /// - 生产代码与用户配置都不可见；
    /// - 仅测试构建下 resolver 会把它当作 fallback/base override。
    #[cfg(test)]
    #[serde(skip)]
    pub provider: String,
    #[cfg(test)]
    #[serde(skip)]
    pub api_base: Option<String>,
    #[cfg(test)]
    #[serde(skip)]
    pub api_key_env: Option<String>,
    /// 最大并发 LLM 请求数，0 表示不限制（不推荐）。
    #[serde(default = "default_max_concurrent_requests")]
    pub max_concurrent_requests: u32,
    /// 非流式请求失败时的重试次数（仅对可重试错误如 429/5xx）。
    #[serde(default = "default_llm_retry_count")]
    pub retry_count: u32,
    /// Agent loop 顶层重试次数（provider 内重试耗尽后仍可继续尝试整轮）。
    #[serde(default = "default_agent_max_attempts")]
    pub agent_max_attempts: u32,
    /// Agent loop 顶层指数退避基础延迟（毫秒）。
    #[serde(default = "default_agent_retry_base_delay_ms")]
    pub agent_retry_base_delay_ms: u64,
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
    /// transcript-first reasoning continuity 总开关（默认 true；可按需显式关闭）。
    #[serde(default)]
    pub reasoning_continuity: ReasoningContinuityConfig,
    /// OpenAI Responses 专属子配置。
    #[serde(default)]
    pub openai_responses: OpenAiResponsesConfig,
}

/// 供 provider 构造复用的全局运行时参数。
///
/// 与 [`LlmConfig`] 的区别：
/// - [`LlmConfig`] 还包含“选哪个模型”的字段；
/// - 本结构体只保留“无论选哪个模型都一样”的运行时旋钮。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LlmRuntimeConfig {
    pub max_concurrent_requests: u32,
    pub retry_count: u32,
    pub agent_max_attempts: u32,
    pub agent_retry_base_delay_ms: u64,
    pub stream_timeout_sec: u64,
    pub non_stream_stale_timeout_sec: u64,
    pub http_read_timeout_sec: u64,
    pub proxy: Option<String>,
    pub api_base_fallback: Option<String>,
    pub thinking: ThinkingConfig,
    pub tool_cli_verbosity: ToolCliVerbosity,
    pub files: LlmFilesConfig,
    pub reasoning_continuity: ReasoningContinuityConfig,
    pub openai_responses: OpenAiResponsesConfig,
}

impl From<&LlmConfig> for LlmRuntimeConfig {
    fn from(value: &LlmConfig) -> Self {
        Self {
            max_concurrent_requests: value.max_concurrent_requests,
            retry_count: value.retry_count,
            agent_max_attempts: value.agent_max_attempts,
            agent_retry_base_delay_ms: value.agent_retry_base_delay_ms,
            stream_timeout_sec: value.stream_timeout_sec,
            non_stream_stale_timeout_sec: value.non_stream_stale_timeout_sec,
            http_read_timeout_sec: value.http_read_timeout_sec,
            proxy: value.proxy.clone(),
            api_base_fallback: value.api_base_fallback.clone(),
            thinking: value.thinking.clone(),
            tool_cli_verbosity: value.tool_cli_verbosity,
            files: value.files.clone(),
            reasoning_continuity: value.reasoning_continuity.clone(),
            openai_responses: value.openai_responses.clone(),
        }
    }
}

impl Default for LlmRuntimeConfig {
    fn default() -> Self {
        LlmConfig::default().runtime()
    }
}

impl LlmConfig {
    pub fn runtime(&self) -> LlmRuntimeConfig {
        self.into()
    }
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

/// transcript-first reasoning continuity 子配置。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ReasoningContinuityConfig {
    #[serde(default = "default_reasoning_continuity_enabled")]
    pub enabled: bool,
}

/// OpenAI Responses continuity 专属开关。
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct OpenAiResponsesConfig {
    /// 是否启用 `previous_response_id` 快车道；开启后切到 `store=true` 分支。
    #[serde(default)]
    pub use_previous_response_id: bool,
}

/// Thinking / Reasoning 协议子配置。
///
/// **产品默认**：`enabled = true`、`show = "summary"`、`level = "high"`。
/// 新默认会流式显示 summary、隐藏 raw；若希望更安静的占位模式，设 `show = "minimal"`；
/// 若希望完全静默 thinking，需显式 `enabled = false`。详见 changelog 与架构 §3.1 G5。
/// 注意 `summary`/`raw` 双流仅 OpenAI Responses 有；chat-completions 类模型
/// （deepseek/mimo/doubao 等）单流统一归 `summary`，默认档即可见其思考。
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

fn default_reasoning_continuity_enabled() -> bool {
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

impl Default for ReasoningContinuityConfig {
    fn default() -> Self {
        Self {
            enabled: default_reasoning_continuity_enabled(),
        }
    }
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

pub const DEFAULT_AGENT_MAX_ATTEMPTS: u32 = 4;
pub const DEFAULT_AGENT_RETRY_BASE_DELAY_MS: u64 = 500;
pub const DEFAULT_LLM_STREAM_TIMEOUT_SEC: u64 = 180;
pub const DEFAULT_LLM_NON_STREAM_STALE_TIMEOUT_SEC: u64 = 300;
pub const DEFAULT_LLM_HTTP_READ_TIMEOUT_SEC: u64 = 120;

fn default_agent_max_attempts() -> u32 {
    DEFAULT_AGENT_MAX_ATTEMPTS
}

fn default_agent_retry_base_delay_ms() -> u64 {
    DEFAULT_AGENT_RETRY_BASE_DELAY_MS
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
            default_model: default_llm_model(),
            vision_model: None,
            title_model: Some("utility-flash".to_string()),
            #[cfg(test)]
            provider: "openai-responses".to_string(),
            #[cfg(test)]
            api_base: None,
            #[cfg(test)]
            api_key_env: None,
            max_concurrent_requests: default_max_concurrent_requests(),
            retry_count: default_llm_retry_count(),
            agent_max_attempts: default_agent_max_attempts(),
            agent_retry_base_delay_ms: default_agent_retry_base_delay_ms(),
            stream_timeout_sec: default_stream_timeout_sec(),
            non_stream_stale_timeout_sec: default_non_stream_stale_timeout_sec(),
            http_read_timeout_sec: default_http_read_timeout_sec(),
            proxy: None,
            api_base_fallback: None,
            thinking: ThinkingConfig::default(),
            tool_cli_verbosity: ToolCliVerbosity::default(),
            files: LlmFilesConfig::default(),
            reasoning_continuity: ReasoningContinuityConfig::default(),
            openai_responses: OpenAiResponsesConfig::default(),
        }
    }
}
