//! # Thinking / Reasoning 请求字段策略
//!
//! 把「逻辑档位 `ThinkingLevel`」+「厂商格式 `ThinkingFormat`」映射到具体请求体字段。
//! 详见 `docs/architecture/llm-stream-events-cli-pipeline.md` §4.2.2。
//!
//! 设计要点：
//!
//! - **集中策略**：provider 层只调用 [`resolve_request_fields`]，不再各自写 if/else，
//!   避免后续厂商分化扩散到 N 个文件。
//! - **集中策略**：优先在这里吸收厂商差异；大多数 format 只会写一个字段，DeepSeek
//!   OpenAI 兼容格式例外，会同时写 `reasoning_effort` 与 `thinking`。
//! - **向后兼容**：`ThinkingLevel::Off` 总是返回 `(None, None)`，`enabled=false` 时
//!   provider 应直接跳过本函数，请求体保持与历史一致。

use serde::{Deserialize, Serialize};

use crate::infra::config::ThinkingConfig;

/// 逻辑档位。顺序即全序：`off < minimal < low < medium < high < xhigh < max`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingLevel {
    Off,
    Minimal,
    Low,
    #[default]
    Medium,
    High,
    Xhigh,
    Max,
}

impl ThinkingLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" | "none" => Some(Self::Off),
            "minimal" => Some(Self::Minimal),
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            "xhigh" | "x-high" => Some(Self::Xhigh),
            "max" => Some(Self::Max),
            _ => None,
        }
    }

    /// 容错解析；未知字符串退化为 `Medium` 并返回 `false` 让 caller 决定是否报告。
    pub fn parse_or_medium(s: &str) -> (Self, bool) {
        Self::parse(s)
            .map(|level| (level, true))
            .unwrap_or((Self::Medium, false))
    }

    pub fn clamp_to_supported(self, supported: &[Self]) -> Self {
        if supported.is_empty() {
            return self;
        }
        if supported.contains(&self) {
            return self;
        }
        supported
            .iter()
            .copied()
            .filter(|candidate| *candidate <= self)
            .max()
            .unwrap_or_else(|| supported.iter().copied().min().unwrap_or(self))
    }
}

/// 厂商请求格式。`Auto` 表示按 wire `api` 推断；其它显式指定。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThinkingFormat {
    /// 按 wire `api` 推断；未知 wire 保守回落到 `Openai`。
    #[default]
    Auto,
    /// OpenAI Chat Completions / Responses：`reasoning_effort` 字符串档位。
    Openai,
    /// OpenRouter / 兼容 OpenAI 网关：与 `Openai` 同形态，单独枚举便于扩展。
    Openrouter,
    /// DeepSeek：OpenAI 兼容格式下，请求侧同时带 `reasoning_effort` + `thinking.enabled`。
    Deepseek,
    /// 智谱 / Z.AI：与 OpenAI Responses 同形态；占位。
    Zai,
    /// Qwen：占位（不在本期接通）。
    Qwen,
    /// 豆包 / Moonshot：`thinking: {"type":"enabled", "max_tokens": ?}` 对象。
    Doubao,
    /// Anthropic Messages：`thinking: {"type":"enabled","budget_tokens": ...}`。
    Anthropic,
    /// 新版 Anthropic：`thinking: {"type":"adaptive"}` + `output_config.effort`。
    AnthropicAdaptive,
}

impl ThinkingFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Openai => "openai",
            Self::Openrouter => "openrouter",
            Self::Deepseek => "deepseek",
            Self::Zai => "zai",
            Self::Qwen => "qwen",
            Self::Doubao => "doubao",
            Self::Anthropic => "anthropic",
            Self::AnthropicAdaptive => "anthropic-adaptive",
        }
    }

    pub fn parse_or_auto(s: Option<&str>) -> Self {
        match s.map(|v| v.trim().to_ascii_lowercase()) {
            None => Self::Auto,
            Some(v) => match v.as_str() {
                "openai" => Self::Openai,
                "openrouter" => Self::Openrouter,
                "deepseek" => Self::Deepseek,
                "zai" => Self::Zai,
                "qwen" => Self::Qwen,
                "doubao" | "moonshot" => Self::Doubao,
                "anthropic" => Self::Anthropic,
                "anthropic-adaptive" => Self::AnthropicAdaptive,
                _ => Self::Auto,
            },
        }
    }

    /// 当 format=Auto 时按 wire `api`（与 [`crate::core::llm::registry`] 注册名一致）推断。
    /// 兼容旧调用名；新路径优先走 [`resolve_for_api`]。
    pub fn resolve(&self, provider_id: &str) -> Self {
        self.resolve_for_api(provider_id)
    }

    /// 当 format=Auto 时按 wire `api` 推断。
    pub fn resolve_for_api(&self, api: &str) -> Self {
        if !matches!(self, Self::Auto) {
            return *self;
        }
        thinking_format_for_api(api)
    }

    /// 旧的按 model 名启发式推断，仅保留给工具函数/单测。
    /// 运行时解析不再调用该路径，避免名字骗过 wire。
    pub fn resolve_for_model(&self, model: &str) -> Self {
        if !matches!(self, Self::Auto) {
            return *self;
        }
        thinking_format_for_model(model)
    }
}

/// 按 wire `api` 归一到默认 thinking 请求格式。
///
/// 设计目标：
/// - 解析期完全不看 model 名，避免中转站把 `claude-*` 这类名字误导成 Anthropic wire；
/// - `openai` 与 `openai-responses` 共用「effort 档位」语义，但由各自 provider 负责编码成
///   顶层 `reasoning_effort` 或嵌套 `reasoning: { effort }`；
/// - 显式 `thinking_format` 仍可覆盖这一路径，用于极少数 relay 方言。
pub fn default_thinking_format_for_api(api: &str) -> &'static str {
    match api.trim() {
        "openai" | "openai-responses" => "openai",
        "deepseek" => "deepseek",
        "zai" => "zai",
        "qwen" => "qwen",
        "doubao" | "moonshot" => "doubao",
        "anthropic" | "anthropic-messages" => "anthropic",
        _ => "openai",
    }
}

pub fn thinking_format_for_api(api: &str) -> ThinkingFormat {
    ThinkingFormat::parse_or_auto(Some(default_thinking_format_for_api(api)))
}

/// 按 model 归一到 thinking 请求格式。
///
/// 设计目标：
/// - 单输入（model 字符串）即可决定默认格式；
/// - 同厂商多个 model 可以复用同一种 format；
/// - 未来某个特殊 model 也可以单独映射到独立 format。
///
/// 注意：该函数仅保留给测试/工具代码；运行时解析已改为 [`thinking_format_for_api`]。
pub fn thinking_format_for_model(model: &str) -> ThinkingFormat {
    let lower = model.trim().to_ascii_lowercase();
    if lower.starts_with("deepseek-") {
        ThinkingFormat::Deepseek
    } else if lower.starts_with("qwen") {
        ThinkingFormat::Qwen
    } else if lower.starts_with("doubao")
        || lower.starts_with("moonshot")
        || lower.starts_with("kimi")
        || lower.starts_with("mimo-")
    {
        ThinkingFormat::Doubao
    } else if matches!(
        lower.as_str(),
        "claude-opus-4-6" | "claude-opus-4-7" | "claude-opus-4-8"
    ) {
        ThinkingFormat::AnthropicAdaptive
    } else if lower.starts_with("claude-") {
        ThinkingFormat::Anthropic
    } else {
        ThinkingFormat::Openai
    }
}

/// `resolve_request_fields` 的输出：大多数 format 最多只写一个字段；DeepSeek 例外。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThinkingRequestFields {
    /// OpenAI / DeepSeek：`reasoning_effort: "low"|"medium"|"high"|...`
    pub reasoning_effort: Option<String>,
    /// DeepSeek / 豆包 / Moonshot：`thinking: {"type":"enabled", ...}`
    pub thinking: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnthropicThinkingRequest {
    pub thinking: Option<serde_json::Value>,
    pub effort: Option<String>,
    pub max_tokens: u32,
}

pub fn normalize_supported_reasoning_levels(levels: &[String]) -> Vec<String> {
    let mut normalized = Vec::new();
    for level in levels {
        let Some(level) = ThinkingLevel::parse(level) else {
            continue;
        };
        let token = level.as_str().to_string();
        if !normalized.iter().any(|existing| existing == &token) {
            normalized.push(token);
        }
    }
    normalized
}

pub fn safe_supported_reasoning_levels_for(
    api: &str,
    thinking_format: Option<&str>,
) -> Vec<String> {
    let fmt = ThinkingFormat::parse_or_auto(thinking_format).resolve_for_api(api);
    let defaults: &[ThinkingLevel] = match fmt {
        ThinkingFormat::Openai | ThinkingFormat::Openrouter => &[
            ThinkingLevel::Low,
            ThinkingLevel::Medium,
            ThinkingLevel::High,
            ThinkingLevel::Xhigh,
        ],
        ThinkingFormat::Deepseek | ThinkingFormat::Zai => {
            &[ThinkingLevel::High, ThinkingLevel::Max]
        }
        ThinkingFormat::Doubao => &[],
        ThinkingFormat::Anthropic | ThinkingFormat::AnthropicAdaptive => &[
            ThinkingLevel::Low,
            ThinkingLevel::Medium,
            ThinkingLevel::High,
            ThinkingLevel::Xhigh,
            ThinkingLevel::Max,
        ],
        ThinkingFormat::Qwen | ThinkingFormat::Auto => &[
            ThinkingLevel::Low,
            ThinkingLevel::Medium,
            ThinkingLevel::High,
            ThinkingLevel::Xhigh,
        ],
    };
    defaults
        .iter()
        .map(|level| level.as_str().to_string())
        .collect()
}

pub fn clamp_reasoning_level(
    level: ThinkingLevel,
    supported_reasoning_levels: &[String],
) -> ThinkingLevel {
    let supported = normalize_supported_reasoning_levels(supported_reasoning_levels)
        .into_iter()
        .filter_map(|token| ThinkingLevel::parse(&token))
        .collect::<Vec<_>>();
    if supported.is_empty() {
        return if level == ThinkingLevel::Off {
            ThinkingLevel::Off
        } else {
            ThinkingLevel::High
        };
    }
    level.clamp_to_supported(&supported)
}

/// 把 `ThinkingConfig` + provider 推断出的 `ThinkingFormat` 翻译为具体请求字段。
///
/// 行为：
/// - `enabled=false` 或 `level=off` → 全 None；
/// - OpenAI 系：`reasoning_effort` 为 level 原样字符串；是否降级由 caller 的 clamp 决定；
/// - DeepSeek：按官方 thinking mode，同时写 `reasoning_effort + thinking={"type":"enabled"}`；
///   本函数不再折叠档位，caller 应保证传入 level 已经在模型支持集内；
/// - 豆包/Moonshot：`thinking={"type":"enabled"}`，带 `max_tokens` 时附带；
/// - Qwen：当前无显式请求字段；响应解析仍走 reasoning_content 三路兜底。
pub fn resolve_request_fields(cfg: &ThinkingConfig, fmt: ThinkingFormat) -> ThinkingRequestFields {
    if !cfg.enabled {
        return ThinkingRequestFields::default();
    }
    let (level, _ok) = ThinkingLevel::parse_or_medium(&cfg.level);
    if matches!(level, ThinkingLevel::Off) {
        return ThinkingRequestFields::default();
    }
    match fmt {
        ThinkingFormat::Openai | ThinkingFormat::Openrouter | ThinkingFormat::Zai => {
            let v = openai_reasoning_effort(level)
                .expect("ThinkingLevel::Off should have returned early");
            ThinkingRequestFields {
                reasoning_effort: Some(v.to_string()),
                thinking: None,
            }
        }
        ThinkingFormat::Deepseek => ThinkingRequestFields {
            reasoning_effort: Some(
                openai_reasoning_effort(level)
                    .expect("ThinkingLevel::Off should have returned early")
                    .to_string(),
            ),
            thinking: Some(serde_json::json!({
                "type": "enabled"
            })),
        },
        ThinkingFormat::Doubao => {
            let mut obj = serde_json::Map::new();
            obj.insert(
                "type".to_string(),
                serde_json::Value::String("enabled".to_string()),
            );
            if let Some(mx) = cfg.max_tokens {
                obj.insert(
                    "max_tokens".to_string(),
                    serde_json::Value::Number(mx.into()),
                );
            }
            ThinkingRequestFields {
                reasoning_effort: None,
                thinking: Some(serde_json::Value::Object(obj)),
            }
        }
        // Qwen：当前无显式请求字段；Anthropic 走 `resolve_anthropic_request`。
        ThinkingFormat::Qwen | ThinkingFormat::Anthropic | ThinkingFormat::AnthropicAdaptive => {
            ThinkingRequestFields::default()
        }
        // Auto 应该已经被 caller resolve 掉；保险起见兜底。
        ThinkingFormat::Auto => ThinkingRequestFields::default(),
    }
}

pub fn resolve_anthropic_request(
    cfg: &ThinkingConfig,
    fmt: ThinkingFormat,
    request_max_tokens: Option<u32>,
) -> AnthropicThinkingRequest {
    let (level, _ok) = ThinkingLevel::parse_or_medium(&cfg.level);
    let default_budget = anthropic_default_budget(level);
    let mut max_tokens = request_max_tokens.unwrap_or_else(|| {
        if cfg.enabled && default_budget > 0 {
            (default_budget + 1024).max(2048)
        } else {
            2048
        }
    });
    if !cfg.enabled || default_budget == 0 {
        return AnthropicThinkingRequest {
            thinking: None,
            effort: None,
            max_tokens: max_tokens.max(256),
        };
    }
    if matches!(fmt, ThinkingFormat::AnthropicAdaptive) {
        return AnthropicThinkingRequest {
            thinking: Some(serde_json::json!({
                "type": "adaptive",
            })),
            effort: Some(level.as_str().to_string()),
            max_tokens: max_tokens.max(256),
        };
    }
    if max_tokens <= 512 {
        return AnthropicThinkingRequest {
            thinking: None,
            effort: None,
            max_tokens: max_tokens.max(256),
        };
    }
    let configured_budget = cfg.max_tokens.unwrap_or(default_budget);
    let budget_tokens = configured_budget
        .min(max_tokens.saturating_sub(256).max(256))
        .max(256);
    if max_tokens <= budget_tokens {
        max_tokens = budget_tokens + 256;
    }
    AnthropicThinkingRequest {
        thinking: Some(serde_json::json!({
            "type": "enabled",
            "budget_tokens": budget_tokens,
        })),
        effort: None,
        max_tokens,
    }
}

fn openai_reasoning_effort(level: ThinkingLevel) -> Option<&'static str> {
    match level {
        ThinkingLevel::Off => None,
        ThinkingLevel::Minimal => Some("minimal"),
        ThinkingLevel::Low => Some("low"),
        ThinkingLevel::Medium => Some("medium"),
        ThinkingLevel::High => Some("high"),
        ThinkingLevel::Xhigh => Some("xhigh"),
        ThinkingLevel::Max => Some("max"),
    }
}

fn anthropic_default_budget(level: ThinkingLevel) -> u32 {
    match level {
        ThinkingLevel::Off => 0,
        ThinkingLevel::Minimal | ThinkingLevel::Low => 1024,
        ThinkingLevel::Medium => 2048,
        ThinkingLevel::High => 4096,
        ThinkingLevel::Xhigh => 8192,
        ThinkingLevel::Max => 16384,
    }
}

// ─── P6 / P7：多轮重发剥离 + 持久化策略（API 地基） ────────────────────────────

/// `strip_on_resend` + `format` 两个条件的总判定：决定多轮重发时是否剥离思考。
///
/// 规则：
/// - `strip_on_resend=false` → 不剥离；
/// - `strip_on_resend=true` 时按 `format` 分支：
///   - `Deepseek`：必须剥离（否则 400 cls error），返回 true；
///   - `Anthropic`（未来）：必须保留（带 signature），返回 false；
///   - 其它（OpenAI/OpenRouter/Doubao/Qwen 等）：默认剥离（true）。
pub fn should_strip_on_resend(cfg: &ThinkingConfig, fmt: ThinkingFormat) -> bool {
    if !cfg.strip_on_resend {
        return false;
    }
    !matches!(
        fmt,
        ThinkingFormat::Auto | ThinkingFormat::Anthropic | ThinkingFormat::AnthropicAdaptive
    )
}

/// `persist=true` 时上层应把 Thinking 事件落 transcript；默认 false（仅展示不落盘）。
pub fn should_persist_thinking(cfg: &ThinkingConfig) -> bool {
    cfg.enabled && cfg.persist
}

/// Anthropic 风格的 assistant 消息 content 是数组 `[{type: "thinking", ...}, {type: "text", ...}]`；
/// 在多轮重发时对该结构剥离 `type=thinking` 的块。**不针对** OpenAI 协议（OpenAI 内
/// 部 ChatMessage 从未把 thinking 写进 content，自然无需剥）。
///
/// 行为：仅当 `value` 为 `Array` 时遍历过滤；其它形状返回 0；返回剥离条数。
pub fn strip_anthropic_thinking_blocks(value: &mut serde_json::Value) -> usize {
    let arr = match value.as_array_mut() {
        Some(a) => a,
        None => return 0,
    };
    let before = arr.len();
    arr.retain(|item| {
        item.get("type")
            .and_then(|t| t.as_str())
            .map(|s| s != "thinking")
            .unwrap_or(true)
    });
    before - arr.len()
}
